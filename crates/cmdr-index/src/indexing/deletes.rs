//! The delete generation: how many batches of index deletes have gone out for a
//! volume since the last time its drive was proved to still be there.
//!
//! Every delete gate asks whether the drive is listed AFTER the observation it
//! guards, which closes the window it can see. One window stays open: a path lookup
//! may be able to answer `ENOENT` while the mount is still in the table (unverified
//! — see the eject plan's M7 landmine). Deletes made in that window pass every gate,
//! so something has to remember that they happened. This counter is that memory: a
//! non-zero count plus a drive that then reads gone is what marks the index for a
//! rebuild.
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
use std::sync::{LazyLock, Mutex};

use cmdr_fs::ignore_poison::IgnorePoison;

use crate::indexing::volume::VolumeId;

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
///
/// The gates only ever ADD to this count; the reader that acts on it is the rebuild
/// marker, which pairs a non-zero count with a drive that reads gone. Until that
/// lands, only these tests read it.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the rebuild marker is the production reader; the gates only record"
    )
)]
pub(crate) fn outstanding(volume_id: &str) -> u64 {
    DELETES
        .lock_ignore_poison()
        .get(volume_id)
        .map_or(0, |counts| counts.batches)
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
    use super::*;

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
