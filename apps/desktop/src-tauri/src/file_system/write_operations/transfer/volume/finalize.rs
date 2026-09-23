//! Landing a fully-written cross-volume temp at its final name.
//!
//! Both halves are one act: [`temp_sibling_path`] mints the `.cmdr-tmp-<uuid>`
//! sibling a staged write goes to, and [`finalize_safe_replace`] swaps it over
//! whatever is at the destination once the last byte is in. Kept out of
//! `conflict.rs` because the resolver only DECIDES that a replace should happen;
//! the five write sites (`copy_serial.rs`, `copy_concurrent_task.rs`,
//! `merge.rs`, `sequential_extract.rs`, `move_cross.rs`) are what call this,
//! after their stream succeeds and the resolver is long done.
//!
//! ❗ On a failure here the new bytes are committed data, not a partial. The
//! caller contract is on `finalize_safe_replace`, and the rescue that keeps them
//! out of reach of the hourly reap is `../recovered_name.rs`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::super::recovered_name::{
    AfterARefusedDelete, FinalizeFailure, discard_unplaced_temp, rescue_out_of_temp_space, what_a_refused_delete_left,
};
use crate::file_system::volume::{Volume, VolumeError};

/// Builds a temp sibling path next to `dest_path` for a staged write.
///
/// Uses the recognizable `.cmdr-tmp-<uuid>` marker (matches the project's temp
/// convention, so a leftover after a crash is identifiable and cleanup helpers
/// recognize it). The temp lives in the same parent directory as the original
/// so the finalize step's `rename` stays within one directory (no cross-dir
/// rename, which some backends refuse).
///
/// Shared with `staged_write.rs`, which stages EVERY cross-volume file write on
/// one of these, not only the conflict-driven safe-replace.
pub(super) fn temp_sibling_path(dest_path: &Path) -> PathBuf {
    let parent = dest_path.parent().unwrap_or(Path::new(""));
    let filename = dest_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    parent.join(format!("{filename}.cmdr-tmp-{}", uuid::Uuid::new_v4()))
}

/// Lands a fully-written temp at its final name: deletes whatever is at `orig`
/// (which survived the entire streaming write) and renames the temp into its
/// place.
///
/// Two callers, one shape: the conflict layer's file→file safe-replace, and
/// `staged_write.rs`'s landing of an ordinary staged write (where `orig` usually
/// doesn't exist yet and the delete is a tolerated `NotFound`).
///
/// Order matters and is the whole point of safe-replace: the temp holds the
/// COMPLETE new data the moment this is called, and `orig` still holds the
/// complete old data. We delete `orig` first, then `rename(temp, orig, false)`
/// into the now-absent slot. We do NOT use `rename(force=true)` to replace:
/// MTP's `rename(force=true)` does NOT delete an existing destination (it can
/// create a duplicate), so an explicit delete-then-rename is the only shape
/// that's correct and uniform across Local / SMB / MTP / InMemory.
///
/// There is a tiny window between the delete and the rename where neither name
/// resolves to a file on disk — but the complete new data lives in `temp`
/// throughout, so a crash in that window leaves a recoverable `.cmdr-tmp-*`
/// sibling rather than data loss. We tolerate `NotFound` on the delete (the
/// original may have vanished out from under us).
///
/// A delete that answers any OTHER error is checked with one stat, because the
/// answer isn't the act (`what_a_refused_delete_left`): the original still there
/// means the swap can't happen, so this takes its own temp away at once (the
/// source still holds those bytes) and reports the refusal; the original gone
/// after all means the way is clear, and the swap goes on; a stat that can't
/// answer leaves the temp alone.
///
/// CALLER CONTRACT: when this returns `Err` the caller must NOT clean up the
/// temp, and [`FinalizeFailure::new_data_at`] says where the new data is when it
/// matters. If the DELETE was refused, nothing moved: the destination still
/// holds the user's file and this already removed the temp (or left it, logged,
/// for the stale-temp sweep when even that delete failed). If the delete
/// SUCCEEDED and the rename failed, the temp holds the only complete copy at the
/// destination and the original is gone, so this rescues it out of temp space
/// (see [`rescue_out_of_temp_space`]) and reports where it went. The write sites
/// enforce the no-cleanup half by stopping their partial-cleanup tracking from
/// designating the temp the moment the streaming write succeeded, before this
/// function runs. See `transfer/CLAUDE.md` § "The post-write temp is committed
/// data" and the `*_preserves_new_data_on_finalize_failure` tests.
pub(super) async fn finalize_safe_replace(
    dest_volume: &Arc<dyn Volume>,
    temp: &Path,
    orig: &Path,
) -> Result<(), FinalizeFailure> {
    match dest_volume.delete(orig).await {
        Ok(()) => {}
        Err(VolumeError::NotFound(_)) => {
            // Already gone; the rename below will land the new data anyway.
        }
        Err(e) => match what_a_refused_delete_left(dest_volume, orig).await {
            // The delete went through despite its answer: the way is clear, so
            // the swap goes on. Stopping here would leave neither file.
            AfterARefusedDelete::Gone => {}
            AfterARefusedDelete::StillThere => {
                // The server kept the original, so the swap can't happen. The
                // temp is a complete copy of bytes the source still holds, and
                // it's ours: take it away now rather than leave it in the
                // user's folder until the hourly reap.
                let discarded = discard_unplaced_temp(dest_volume, temp).await;
                log::warn!(
                    "finalize_safe_replace: couldn't delete the original {}, so the destination still holds it (temp {} {}): {}",
                    orig.display(),
                    temp.display(),
                    if discarded {
                        "removed"
                    } else {
                        "left for the stale-temp sweep"
                    },
                    e
                );
                return Err(FinalizeFailure {
                    error: e,
                    new_data_at: None,
                });
            }
            AfterARefusedDelete::Unknown => {
                return Err(FinalizeFailure {
                    error: e,
                    new_data_at: None,
                });
            }
        },
    }
    match dest_volume.rename(temp, orig, false).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let new_data_at = rescue_out_of_temp_space(dest_volume, temp, orig).await;
            log::warn!(
                "finalize_safe_replace: the original {} is gone and the new data couldn't take its name, so it is at {} now: {}",
                orig.display(),
                new_data_at.display(),
                error
            );
            Err(FinalizeFailure {
                error,
                new_data_at: Some(new_data_at),
            })
        }
    }
}
