//! How a full-rescan walk ends, and the bracket it runs under.
//!
//! Both halves exist so the network and local full-tree reconciles can't drift on
//! the ordering invariant: stamp every successfully-listed dir FIRST, then run a
//! SINGLE aggregate.

use crate::indexing::store::IndexStoreError;
use crate::indexing::writer::{AggSource, IndexWriter, WriteMessage};

/// Number of dir ids per `MarkDirsListed` message. Bounds each message's size;
/// the writer-side `IndexStore::mark_dirs_listed` chunks the SQL `UPDATE` further
/// (at 900, under SQLite's bound-parameter ceiling) inside a savepoint, so this is
/// only message-level batching — never load-bearing for SQL correctness.
const MARK_CHUNK: usize = 10_000;

/// Emit `MarkDirsListed` for every successfully-listed dir id, chunked. A no-op
/// when empty. The only failure is a writer send (the writer thread is gone); the
/// caller maps that into its own error type.
pub(crate) fn send_marks(listed_ids: &[i64], epoch: u64, writer: &IndexWriter) -> Result<(), IndexStoreError> {
    for chunk in listed_ids.chunks(MARK_CHUNK) {
        writer.send(WriteMessage::MarkDirsListed {
            ids: chunk.to_vec(),
            epoch,
        })?;
    }
    Ok(())
}

/// The full-rescan FINISH, in ONE place so the network and (future) local
/// full-tree reconcile can't drift on the ordering invariant: stamp every
/// successfully-listed dir's `listed_epoch` FIRST, then run a SINGLE
/// `ComputeAllAggregates`.
///
/// The mark-before-aggregate order is load-bearing: aggregating
/// before the marks rolls the whole tree to `min_subtree_epoch = 0` (incomplete);
/// a mark queued after the aggregate drags that dir's ancestors to incomplete. The
/// single in-order writer guarantees the order once sequenced here.
///
/// This is the single-aggregate coverage refresh the full-rescan path uses, NOT
/// the per-dir `PropagateMinSubtreeEpoch` propagation `reconcile_subtree` runs (a
/// ~2.4x regression at full scale; that stays the small-scope live path). A no-op
/// reconcile still runs the aggregate (cheap O(dirs) bulk SQL since no
/// `InsertEntriesV2` ran) so coverage re-stamps to the new epoch; it writes no
/// entry rows. The only failure is a writer send; the caller maps it.
pub(crate) fn finish_reconcile(listed_ids: &[i64], epoch: u64, writer: &IndexWriter) -> Result<(), IndexStoreError> {
    send_marks(listed_ids, epoch, writer)?;
    // `Sql`, not `Maps`: a reconcile writes via `UpsertEntryV2` (maps empty in the
    // happy case), but a verification subtree scan's `InsertEntriesV2` can leave
    // the shared writer's accumulator polluted with subtree-only data. Declaring
    // `Sql` recomputes from committed rows and can't be poisoned by that (Leak D).
    writer.send(WriteMessage::ComputeAllAggregates { source: AggSource::Sql })?;
    Ok(())
}

/// RAII bracket for a FULL reconcile's bulk walk: tells the writer to STOP
/// per-entry ancestor `dir_stats` propagation for the walk's duration, then
/// RESTORES it on EVERY scope exit (clean finish, cancel, empty-root, error, or
/// panic) so the shared, long-lived writer is never left non-propagating for the
/// LIVE event loop that runs afterwards.
///
/// Why suppress: the full reconcile emits thousands of `UpsertEntryV2` / `Delete*`;
/// letting each one walk the ancestor chain
/// (`propagate_delta_by_id` / `propagate_min_subtree_epoch` /
/// `propagate_recursive_has_symlinks`) is O(entries × tree-depth) and wedges the
/// writer for hours on a large delta. It's also pure waste: `finish_reconcile`'s
/// single `ComputeAllAggregates` recomputes every dir's stats from the entries
/// table, overwriting whatever per-entry propagation produced. The LIVE path
/// (`reconcile_subtree`, FSEvents) has NO final aggregate, so it MUST keep
/// propagating — which is exactly why this guard restores the default on exit.
///
/// Suppression is a DEBT, so the bracket also records it: `begin` marks the
/// `dir_stats` ledger unpaid (durably, via `MarkLedgerUnpaid`) and the exit pays
/// it (`PayLedgerIfUnpaid`, a no-op once the walk's own `ComputeAllAggregates`
/// disarmed the latch). Without that, a walk that never reaches its terminal
/// aggregate leaves every ancestor of a mid-walk-discovered directory claiming an
/// exact size over a descendant at `listed_epoch = 0` — measured in production as
/// 249 lying directories after a rescan the user quit 5 seconds in. The durable
/// half is what covers process death, where no `Drop` runs at all.
///
/// All sends are best-effort: on a hard writer-gone error the send fails and is
/// ignored, matching how the surrounding walk already treats writer sends.
pub(crate) struct BulkReconcileGuard {
    writer: IndexWriter,
}

impl BulkReconcileGuard {
    /// Begin the bracket: record the debt, then disable per-entry propagation.
    ///
    /// Order matters: the marker must be committed BEFORE the first suppressed
    /// write, or a death between the two would leave drift with a paid ledger.
    pub(crate) fn begin(writer: &IndexWriter) -> Self {
        let _ = writer.send(WriteMessage::MarkLedgerUnpaid);
        let _ = writer.send(WriteMessage::SetDeltaPropagation(false));
        Self { writer: writer.clone() }
    }
}

impl Drop for BulkReconcileGuard {
    fn drop(&mut self) {
        // Re-enable per-entry propagation for the subsequent live path, then pay
        // the ledger if this walk never ran its own aggregate.
        let _ = self.writer.send(WriteMessage::SetDeltaPropagation(true));
        let _ = self.writer.send(WriteMessage::PayLedgerIfUnpaid);
    }
}
