//! The delete generation: how many batches of index deletes have gone out for a
//! volume since the last time its drive was proved to still be there.
//!
//! Every delete gate asks whether the drive is listed AFTER the observation it
//! guards, which closes the window it can see. One window stays open: a path lookup
//! may be able to answer `ENOENT` while the mount is still in the table (unverified
//! — see `reconcile/DETAILS.md` § "The delete gates"). Deletes made in that window
//! pass every gate, so something has to remember that they happened. This counter
//! is that memory: a non-zero count plus a drive that then reads gone is what marks
//! the index for a rebuild.
//!
//! ❌ **A presence read alone never resets it.** Taken inside an unmount window a
//! `Some(true)` says nothing about batches already sent. The reset needs BOTH a
//! successful listing of the volume ROOT and a `Some(true)` read after it: together
//! they say the drive was really there after the last batch.
//!
//! A leaf beside `hold.rs` for the same reason it is one: the scanner, reconcile,
//! watch, and verifier all send deletes, and nothing below `lifecycle` may import
//! `lifecycle::state`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

use cmdr_fs::ignore_poison::IgnorePoison;

use crate::indexing::events::IndexEvent;
use crate::indexing::store::{INDEX_NEEDS_REBUILD_KEY, IndexStore, IndexStoreError};
use crate::indexing::volume::VolumeId;
use crate::indexing::writer::{IndexWriter, WriteMessage};

/// One volume's outstanding delete batches, and whether its root has been listed
/// since the last one.
#[derive(Default)]
struct Counts {
    /// Batches sent since the last reset.
    batches: u64,
    /// A successful listing of the volume ROOT that came after the last batch. Half
    /// of the reset condition; the presence read is the other half.
    root_listed_since_last_batch: bool,
}

/// Every volume's count. A volume that has sent none has no entry.
static DELETES: LazyLock<Mutex<HashMap<VolumeId, Counts>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Record that a batch of deletes went out for `volume_id`.
///
/// Clears the root-listing half of the reset condition: a listing that came BEFORE
/// this batch says nothing about whether the drive was there when it was sent.
pub(crate) fn batch_sent(volume_id: &str) {
    let mut table = DELETES.lock_ignore_poison();
    let counts = table.entry(volume_id.to_string()).or_default();
    counts.batches += 1;
    counts.root_listed_since_last_batch = false;
}

/// How many delete batches are outstanding for `volume_id`.
#[cfg(test)]
pub(crate) fn outstanding(volume_id: &str) -> u64 {
    DELETES
        .lock_ignore_poison()
        .get(volume_id)
        .map_or(0, |counts| counts.batches)
}

/// Take `volume_id`'s outstanding count, leaving none behind.
///
/// ⚠️ **Taking, ❌ never reading**, and that is what makes the marker fire ONCE per
/// vanish. Several gates can notice the same drive leaving within milliseconds of
/// each other; the first one to ask owns those batches and writes the marker, and
/// the rest see nothing owed. A later batch re-arms it, which is right: it is a new
/// delete against the same absent drive.
fn take_outstanding(volume_id: &str) -> u64 {
    DELETES
        .lock_ignore_poison()
        .get_mut(volume_id)
        .map_or(0, |counts| std::mem::take(&mut counts.batches))
}

/// Mark `volume_id`'s index for a rebuild if deletes went out that its drive can no
/// longer account for, and say so once.
///
/// **The one reader of the count**, called by every gate that finds the drive gone
/// AFTER an observation. A gate refuses the deletes it can still see coming; this is
/// for the ones already sent, whose rows are indistinguishable from files the user
/// really removed. A volume with nothing outstanding writes nothing, so the healthy
/// path costs one map lookup.
///
/// Through the WRITER, so the marker lands in order behind the deletes it speaks
/// for, and so it is on disk before the shutdown that usually follows (the channel
/// is in order and a drain processes what is queued). A stop that has already
/// drained its writer uses
/// [`note_the_drive_left_after_the_drain`] instead.
pub(crate) fn note_the_drive_left(volume_id: &str, writer: &IndexWriter) {
    if take_outstanding(volume_id) == 0 {
        return;
    }
    if let Err(e) = writer.send(WriteMessage::UpdateMeta {
        key: INDEX_NEEDS_REBUILD_KEY.to_string(),
        value: "1".to_string(),
    }) {
        log::warn!("'{volume_id}': couldn't mark the index for a rebuild after its drive left: {e}");
        return;
    }
    announce_the_rebuild(volume_id, writer.events().as_ref());
}

/// The same, for a stop that has already drained this volume's writer: the marker
/// goes through a short-lived connection, the way the per-drive intent markers do.
///
/// ⚠️ Only once no writer thread is live for the volume (`store/connection.rs`).
/// This is the durable half of the pair: an in-session write can still be lost to a
/// kill, while a drive that vanished and was stopped for it ends here, and the
/// marker is on disk before the app can quit.
pub(crate) fn note_the_drive_left_after_the_drain(volume_id: &str, db_path: &Path) {
    if take_outstanding(volume_id) == 0 {
        return;
    }
    if let Err(e) = IndexStore::mark_index_needs_rebuild(db_path) {
        log::warn!("'{volume_id}': couldn't mark the index for a rebuild after its drive left: {e}");
        return;
    }
    announce_the_rebuild(volume_id, crate::indexing::host::events::current().as_ref());
}

/// Whether the rebuild marker counts as SET, from a read of it that may have failed.
///
/// ❗ **A read that couldn't answer counts as SET, ❌ never as "no marker".** This one
/// row outranks every other cell of the launch-routing table
/// (`lifecycle/manager/launch_route.rs`), and an index a vanishing drive deleted from
/// looks perfectly finished to all of them, so folding the failure into `false` sends
/// the launch replaying or reconciling in place over holes nobody would ever notice.
/// A spurious rebuild costs one rescan; a skipped one carries the holes forward for
/// the life of the index.
pub(crate) fn marker_reads_as_set(read: Result<bool, IndexStoreError>, volume_id: &str) -> bool {
    match read {
        Ok(set) => set,
        Err(e) => {
            log::warn!(
                "'{volume_id}': the rebuild marker wouldn't read, so the launch rebuilds rather than trusting an index that may be missing rows: {e}"
            );
            true
        }
    }
}

/// One `warn` for the log and one event for the host, for a marker that just landed.
fn announce_the_rebuild(volume_id: &str, events: &dyn crate::indexing::events::EventSink) {
    log::warn!(
        "'{volume_id}': deletes had gone out when its drive read gone, so its index is marked for a rebuild and the next start walks it from scratch"
    );
    events.emit(IndexEvent::IndexNeedsFreshScan {
        volume_id: volume_id.to_string(),
    });
}

/// Record that `volume_id`'s ROOT listed successfully. Arms the reset; the next
/// `Some(true)` presence read completes it.
///
/// ⚠️ The volume root specifically. A sub-directory listing proves only that one
/// directory was readable, which an unmounted `/Volumes/X` whose mount-point folder
/// survives can still manage.
pub(crate) fn root_listed(volume_id: &str) {
    if let Some(counts) = DELETES.lock_ignore_poison().get_mut(volume_id) {
        counts.root_listed_since_last_batch = true;
    }
}

/// Record a `Some(true)` presence read for `volume_id`, resetting the count when a
/// root listing already came after the last batch.
pub(crate) fn drive_seen(volume_id: &str) {
    // ❌ Never reset on the presence read alone. Inside an unmount window a
    // `Some(true)` can arrive while the batches already sent went out against a drive
    // on its way out; only a root listing that came AFTER the last batch says
    // otherwise.
    if let Some(counts) = DELETES.lock_ignore_poison().get_mut(volume_id)
        && counts.root_listed_since_last_batch
    {
        counts.batches = 0;
        counts.root_listed_since_last_batch = false;
    }
}

/// Drop `volume_id`'s count, so one test's batches can't reach another's.
#[cfg(test)]
pub(crate) fn forget(volume_id: &str) {
    DELETES.lock_ignore_poison().remove(volume_id);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::indexing::events::{IndexEventKind, RecordingSink};

    /// A volume with a real writer over a real database, so the marker can be read
    /// back from the `meta` table the production path writes it to.
    fn a_volume_with_a_writer(
        volume_id: &str,
    ) -> (IndexWriter, std::path::PathBuf, tempfile::TempDir, Arc<RecordingSink>) {
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir.path().join(format!("{volume_id}.db"));
        IndexStore::open(&db_path).expect("open the store");
        let events = Arc::new(RecordingSink::new());
        let writer = IndexWriter::spawn(
            &db_path,
            Arc::clone(&events) as Arc<dyn crate::indexing::events::EventSink>,
        )
        .expect("spawn the writer");
        (writer, db_path, dir, events)
    }

    /// Whether this index carries the rebuild marker.
    fn is_marked(db_path: &Path) -> bool {
        let conn = IndexStore::open_read_connection(db_path).expect("read connection");
        IndexStore::index_needs_rebuild(&conn).expect("read the marker")
    }

    /// ❗ A marker nobody could read is not "no marker". The launch-routing table asks
    /// this row first precisely because an index a vanishing drive deleted from looks
    /// finished to every other cell, so a failed read that answered `false` would send
    /// the launch replaying over holes.
    #[test]
    fn a_rebuild_marker_nobody_could_read_counts_as_set() {
        // A real failure, not a hand-made `Err`: a database with no schema at all can't
        // answer the marker, which is the shape a corrupt or half-open index takes.
        let conn = cmdr_fs::sqlite_util::open_in_memory().expect("an in-memory connection");
        let unreadable = IndexStore::index_needs_rebuild(&conn);
        assert!(unreadable.is_err(), "a schema-less database can't answer the marker");
        assert!(
            marker_reads_as_set(unreadable, "vol-unreadable"),
            "a marker that wouldn't read routes the launch to a rebuild"
        );

        // And a read that DID answer is still believed both ways.
        assert!(!marker_reads_as_set(Ok(false), "vol-clean"));
        assert!(marker_reads_as_set(Ok(true), "vol-marked"));
    }

    /// ❗ What the whole count is for: deletes went out, the drive then read gone,
    /// so the index says on disk that it may be missing rows.
    ///
    /// Through the writer, in order behind the deletes it speaks for, and announced
    /// exactly once — the host turns that into the one sentence a person who just
    /// pulled a drive reads.
    #[test]
    fn a_batch_then_a_drive_that_left_marks_the_index_for_a_rebuild() {
        let volume_id = "deletes-test-marker";
        forget(volume_id);
        let (writer, db_path, _dir, events) = a_volume_with_a_writer(volume_id);

        batch_sent(volume_id);
        note_the_drive_left(volume_id, &writer);
        writer.flush_blocking().expect("flush the writer");

        assert!(is_marked(&db_path), "the next start has to rebuild this index");
        assert_eq!(
            events.kinds_for(volume_id),
            vec![IndexEventKind::IndexNeedsFreshScan],
            "and the host hears about it once"
        );

        // A second gate noticing the same vanish owes nothing.
        note_the_drive_left(volume_id, &writer);
        writer.flush_blocking().expect("flush the writer");
        assert_eq!(
            events.kinds_for(volume_id).len(),
            1,
            "❌ one disconnection, one announcement, however many gates saw it"
        );
        writer.shutdown();
        forget(volume_id);
    }

    /// A drive that left with nothing outstanding owes no rebuild: the index lost
    /// no rows, so marking it would cost a full rescan for nothing.
    #[test]
    fn a_drive_that_left_with_no_deletes_outstanding_marks_nothing() {
        let volume_id = "deletes-test-no-marker";
        forget(volume_id);
        let (writer, db_path, _dir, events) = a_volume_with_a_writer(volume_id);

        note_the_drive_left(volume_id, &writer);
        writer.flush_blocking().expect("flush the writer");

        assert!(!is_marked(&db_path));
        assert!(events.kinds_for(volume_id).is_empty());
        writer.shutdown();
        forget(volume_id);
    }

    /// The durable half: a stop that has already drained its writer still gets the
    /// marker down, through a short-lived connection. This is the path a pulled
    /// drive actually takes, and the one that has to survive the quit that often
    /// follows.
    #[test]
    fn a_stop_that_drained_its_writer_still_marks_the_index() {
        let _serialized = crate::indexing::handle::test_lock();
        let volume_id = "deletes-test-marker-after-drain";
        forget(volume_id);
        let (writer, db_path, _dir, _events) = a_volume_with_a_writer(volume_id);
        writer.shutdown();

        let host = Arc::new(RecordingSink::new());
        let _installed = crate::indexing::host::events::install_for_test(
            Arc::clone(&host) as Arc<dyn crate::indexing::events::EventSink>
        );

        batch_sent(volume_id);
        note_the_drive_left_after_the_drain(volume_id, &db_path);

        assert!(
            is_marked(&db_path),
            "the marker outlives the writer that would have carried it"
        );
        assert_eq!(host.kinds_for(volume_id), vec![IndexEventKind::IndexNeedsFreshScan]);
        forget(volume_id);
    }

    /// A volume nothing has deleted from owes no rebuild.
    #[test]
    fn a_volume_with_no_deletes_has_none_outstanding() {
        assert_eq!(outstanding("deletes-test-untouched"), 0);
    }

    /// The count is of BATCHES, so a gate that sends one batch per directory or per
    /// event batch is what it measures.
    #[test]
    fn every_batch_counts() {
        let volume_id = "deletes-test-counts";
        forget(volume_id);
        batch_sent(volume_id);
        batch_sent(volume_id);
        assert_eq!(outstanding(volume_id), 2);
        forget(volume_id);
    }

    /// The full reset: the root listed after the last batch, and a presence read
    /// after that. Only then is the drive proved to have been there all along.
    #[test]
    fn a_root_listing_then_a_presence_read_resets_the_count() {
        let volume_id = "deletes-test-reset";
        forget(volume_id);
        batch_sent(volume_id);
        root_listed(volume_id);
        drive_seen(volume_id);
        assert_eq!(outstanding(volume_id), 0, "the drive was proved to still be there");
        forget(volume_id);
    }

    /// ❗ The one that matters: a presence read taken INSIDE an unmount window can
    /// answer `Some(true)` while the deletes already sent were made against a drive
    /// on its way out. Without a root listing after the batch, it proves nothing and
    /// must leave the count standing — this is the only thing that catches the
    /// unverified `ENOENT`-mid-unmount window.
    #[test]
    fn a_presence_read_alone_does_not_reset_the_count() {
        let volume_id = "deletes-test-presence-alone";
        forget(volume_id);
        batch_sent(volume_id);
        drive_seen(volume_id);
        assert_eq!(
            outstanding(volume_id),
            1,
            "a `Some(true)` with no root listing after the batch resets nothing"
        );
        forget(volume_id);
    }

    /// ❗ The marker fires ONCE per vanish, however many gates notice.
    ///
    /// A drive leaving trips every gate that touches it within milliseconds: the
    /// live loop's batch, a walk's directory, the verifier's pass. Each asks what is
    /// outstanding, and the first to ask owns it — otherwise one disconnection would
    /// write the marker a dozen times and announce itself a dozen times to the
    /// person who pulled one drive.
    #[test]
    fn only_the_first_asker_owes_the_rebuild() {
        let volume_id = "deletes-test-one-owner";
        forget(volume_id);
        batch_sent(volume_id);
        batch_sent(volume_id);

        assert_eq!(take_outstanding(volume_id), 2, "the first asker owns both batches");
        assert_eq!(take_outstanding(volume_id), 0, "and every later one owes nothing");

        // A new batch against the same absent drive is a new debt, so the next
        // gate to notice marks it again.
        batch_sent(volume_id);
        assert_eq!(take_outstanding(volume_id), 1);
        forget(volume_id);
    }

    /// A root listing that came BEFORE the last batch is stale: the batch that
    /// followed it is exactly what's in doubt.
    #[test]
    fn a_root_listing_before_the_last_batch_does_not_reset_the_count() {
        let volume_id = "deletes-test-stale-listing";
        forget(volume_id);
        root_listed(volume_id);
        batch_sent(volume_id);
        drive_seen(volume_id);
        assert_eq!(
            outstanding(volume_id),
            1,
            "the listing predates the batch, so it can't clear it"
        );
        forget(volume_id);
    }
}
