//! The privacy retro-delete, and the purges it still owes.
//!
//! Excluding a folder deletes its stored rows at once, across every reachable volume
//! (`../DETAILS.md` § Per-folder photo-search exclude). Split out of [`super`] because a
//! purge can fail to land and has to outlive the call that asked for it: SQLite refuses
//! the delete on a full disk or a locked database, a volume's writer won't open, or the
//! `VACUUM` that takes the text off the disk fails. Each of those leaves the purge OWED
//! in a per-volume ledger, and every pass on that volume settles what it owes before it
//! does anything else, until nothing is left. A launch rebuilds the ledger from the
//! persisted exclusions (`lifecycle::wire_volume` re-runs the retro-delete for each one),
//! so a restart doesn't forget a purge either. Nobody is told: the veto is already live,
//! and there's nothing a person could do that the retry doesn't.

use std::collections::HashMap;

use cmdr_fs::ignore_poison::IgnorePoison;

use crate::media_index::{coverage, network, store, vector};

use super::MediaScheduler;

/// What one volume still owes: the folder prefixes whose rows haven't left yet, and
/// whether a `VACUUM` is owed after a delete that did.
#[derive(Debug, Default)]
pub(super) struct OwedPurge {
    /// The excluded OS folder → the index-space prefix its rows sit under on this volume.
    /// Keyed by the folder, so un-excluding it drops the debt instead of deleting rows
    /// nobody wants gone any more. The prefix is mapped once, when the purge is owed, so a
    /// retry needs no mount root: a share remounted under a new name still stores the same
    /// index-relative paths.
    prefixes: HashMap<String, String>,
    /// Rows left the database but `VACUUM` didn't run, so their pages (and, after a
    /// retro-delete, the recognized text in them) are still in the file.
    vacuum: bool,
}

/// How a purge ended.
#[must_use = "a Pending purge is still owed; reading it as done is how a refused delete once looked like success"]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurgeOutcome {
    /// Nothing is owed: every targeted row is gone and its pages are reclaimed.
    Settled {
        /// The stored rows this call deleted.
        deleted_rows: u64,
    },
    /// Part of it didn't land. The volume keeps the debt, and its next pass (or the next
    /// launch) retries it.
    Pending {
        /// The stored rows this call deleted before the part that didn't land.
        deleted_rows: u64,
    },
}

impl MediaScheduler {
    /// Retro-delete every stored row at or under `folder` (an OS-mount path) across the
    /// reachable volumes in `mounts` (`volume_id`, `mount_root`) — the privacy
    /// complement to the veto, invoked when the user excludes a folder. USER-EXPLICIT
    /// deletion: it derives ONLY from settings state, never scan/bus/gate state, so it
    /// needs no completed-scan edge (unlike GC — see `../DETAILS.md` § The GC safety argument).
    ///
    /// Each volume maps the OS folder into its own index-path space
    /// ([`os_folder_to_index_prefix`](network::fetch::os_folder_to_index_prefix)): the
    /// folder passes through on a local volume, strips the mount root on a network one,
    /// and a volume the folder isn't under is skipped. The purge is owed BEFORE it runs
    /// and then settled ([`settle_owed_purges`](Self::settle_owed_purges)), so whatever
    /// doesn't land stays owed for the volume's next pass. [`PurgeOutcome::Pending`] says
    /// some volume still owes part of it.
    ///
    /// **Offline network volumes** aren't in `mounts` (no mount root while unmounted),
    /// so they're skipped here and the retro-delete re-fires on reconnect via
    /// `lifecycle::wire_volume`. Runs off the IPC thread (the caller uses `spawn_blocking`), so
    /// the blocking prunes are deadlock-safe.
    pub fn retro_delete_excluded_folder(&self, folder: &str, mounts: &[(String, String)]) -> PurgeOutcome {
        let mut deleted_rows = 0u64;
        let mut pending = false;
        for (volume_id, mount_root) in mounts {
            // Only volumes that were actually enriched have a `media.db`; don't create an
            // empty one just to prune nothing.
            if !store::media_db_path(&self.data_dir, volume_id).exists() {
                continue;
            }
            // Map the OS folder into this volume's index-path space; `None` ⇒ the folder
            // isn't under this mount, so this volume has no matching rows.
            let Some(index_prefix) = network::fetch::os_folder_to_index_prefix(folder, mount_root) else {
                continue;
            };
            // Owe the purge BEFORE paying it, so a refused delete, or a panic part-way
            // through, leaves it on the books for the next pass.
            self.owe_purge(volume_id, folder, index_prefix);
            match self.settle_owed_purges(volume_id) {
                PurgeOutcome::Settled { deleted_rows: n } => deleted_rows += n,
                PurgeOutcome::Pending { deleted_rows: n } => {
                    deleted_rows += n;
                    pending = true;
                }
            }
        }
        if pending {
            PurgeOutcome::Pending { deleted_rows }
        } else {
            PurgeOutcome::Settled { deleted_rows }
        }
    }

    /// Settle whatever `volume_id` owes: prune each owed folder that is STILL excluded
    /// (dropping the ones that aren't), `VACUUM` when rows left or a `VACUUM` was owed, and
    /// put back whatever didn't land. Every pass calls this before its own work, which is
    /// what makes a refused purge retry until it lands. Answers
    /// `Settled { deleted_rows: 0 }` at the cost of one map lookup when nothing is owed.
    pub(super) fn settle_owed_purges(&self, volume_id: &str) -> PurgeOutcome {
        const NOTHING_OWED: PurgeOutcome = PurgeOutcome::Settled { deleted_rows: 0 };
        // Every pass asks, and almost every volume owes nothing: answer that without the
        // serializing lock below.
        if !self.owed_purges.lock_ignore_poison().contains_key(volume_id) {
            return NOTHING_OWED;
        }
        // One settle at a time, so a caller arriving mid-purge waits for it to finish
        // rather than finding an empty ledger while the rows are still being deleted.
        let _serial = self.purge_serial.lock_ignore_poison();
        let Some(mut owed) = self.owed_purges.lock_ignore_poison().remove(volume_id) else {
            return NOTHING_OWED;
        };
        let writer = match self.writers.writer_for(&self.data_dir, volume_id) {
            Ok(writer) => writer,
            Err(e) => {
                log::warn!(
                    target: "media_index",
                    "purge on '{volume_id}' can't open its writer ({e}); retrying on its next pass"
                );
                self.restore_owed_purge(volume_id, owed);
                return PurgeOutcome::Pending { deleted_rows: 0 };
            }
        };

        let excluded = network::config::snapshot().excluded_folders;
        let mut deleted_rows = 0u64;
        owed.prefixes.retain(|folder, prefix| {
            // Un-excluded since it was owed: nobody wants these rows gone any more.
            if !excluded.contains(folder) {
                return false;
            }
            // Double-tap through the ONE writer thread: the first (blocking) prune drains
            // the queue up to it; the second sweeps any straggler an in-flight upsert
            // re-added before its own pre-upsert veto re-check could stop it.
            let pruned = writer
                .prune_under_folder(prefix)
                .and_then(|first| writer.prune_under_folder(prefix).map(|second| first + second));
            match pruned {
                Ok(n) => {
                    deleted_rows += n as u64;
                    false
                }
                Err(e) => {
                    log::warn!(
                        target: "media_index",
                        "purge under '{folder}' on '{volume_id}' didn't land ({e}); retrying on its next pass"
                    );
                    true
                }
            }
        });

        if deleted_rows > 0 || owed.vacuum {
            // The ANN flush lands the buffered key removals first, so the purged images
            // stop being ANN-reachable at the same moment (best-effort: an unusable index
            // is wiped for a rebuild). Then reclaim the pages — privacy: the OCR text
            // leaves the disk, not just the table.
            let _ = writer.flush_ann_index();
            owed.vacuum = match writer.vacuum() {
                Ok(()) => false,
                Err(e) => {
                    log::warn!(
                        target: "media_index",
                        "VACUUM after a purge on '{volume_id}' failed ({e}); retrying on its next pass"
                    );
                    true
                }
            };
        }
        if deleted_rows > 0 {
            // Drop the derived caches so a later search / slider preview rebuilds honestly.
            vector::cache::invalidate(&store::media_db_path(&self.data_dir, volume_id));
            coverage::invalidate(volume_id);
            log::info!(
                target: "media_index",
                "purge on '{volume_id}': {} removed",
                cmdr_fs::pluralize::pluralize(deleted_rows, "row")
            );
        }

        if owed.prefixes.is_empty() && !owed.vacuum {
            PurgeOutcome::Settled { deleted_rows }
        } else {
            self.restore_owed_purge(volume_id, owed);
            PurgeOutcome::Pending { deleted_rows }
        }
    }

    /// Owe `volume_id` a purge of `folder`'s rows, which sit under `index_prefix` there.
    fn owe_purge(&self, volume_id: &str, folder: &str, index_prefix: String) {
        self.owed_purges
            .lock_ignore_poison()
            .entry(volume_id.to_string())
            .or_default()
            .prefixes
            .insert(folder.to_string(), index_prefix);
    }

    /// Put back what a settle couldn't finish, merged with anything owed while it ran.
    fn restore_owed_purge(&self, volume_id: &str, owed: OwedPurge) {
        let mut ledger = self.owed_purges.lock_ignore_poison();
        let entry = ledger.entry(volume_id.to_string()).or_default();
        entry.vacuum |= owed.vacuum;
        for (folder, prefix) in owed.prefixes {
            entry.prefixes.entry(folder).or_insert(prefix);
        }
    }
}
