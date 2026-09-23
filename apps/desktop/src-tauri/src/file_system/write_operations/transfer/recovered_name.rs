//! Getting committed data out of `.cmdr-tmp-*` space, and saying where it went.
//!
//! A staged write's temp stops being a partial the moment the bytes are all
//! there: from then on it holds the only complete copy of the new file, and the
//! landing rename is what gives it the user's name. When that rename fails after
//! the destination has already been cleared, the bytes are committed data
//! wearing a sweepable name, and both halves of this module answer that:
//! [`rescue_out_of_temp_space`] gives them a real filename, and
//! [`FinalizeFailure`] carries that name out to the user.
//!
//! The other half is the landing that fails with nothing cleared: the user's
//! file (or someone else's) still holds the name, the source still holds the new
//! bytes, and the temp is a complete-but-unplaced copy of ours that nobody
//! needs. [`discard_unplaced_temp`] takes it away at once, so the user's folder
//! doesn't carry a `.cmdr-tmp-*` until the hourly reap, and
//! [`what_a_refused_delete_left`] is how a landing whose delete answered an
//! error finds out which of the two situations it is in.
//!
//! It sits at `transfer/` level rather than inside `volume/` because BOTH
//! landings reach it: `volume::finalize::finalize_safe_replace` (a cross-volume
//! file→file Overwrite) and `staged_write::land` (any staged write).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::super::unique_name::{NameCandidates, RESCUE_NAME_ATTEMPTS, recovered_sibling};
use crate::file_system::volume::{Volume, VolumeError};

/// What a failed landing leaves behind, and where.
///
/// ❗ The path is TYPED data, not prose: it travels to the user through
/// `WriteOperationError::NewDataKeptAt` so the dialog can name the file they
/// have to go and look at. ❌ Never parse it back out of a message.
#[derive(Debug)]
pub(in crate::file_system::write_operations) struct FinalizeFailure {
    /// What the destination said.
    pub error: VolumeError,
    /// Where the only complete copy of the NEW bytes is now, when the name they
    /// were meant to take is already cleared: the ` (recovered)` name, or the
    /// `.cmdr-tmp-*` temp when even that rename couldn't happen.
    ///
    /// `None` when nothing was cleared, so nothing was rescued and nothing was
    /// lost.
    pub new_data_at: Option<PathBuf>,
}

impl From<VolumeError> for FinalizeFailure {
    /// An ordinary failure, with nothing stranded anywhere.
    fn from(error: VolumeError) -> Self {
        Self {
            error,
            new_data_at: None,
        }
    }
}

/// Gets the complete new bytes out of `.cmdr-tmp-*` space and into a real
/// filename next to where they were meant to land.
///
/// ❗ **This is what stops the hourly reap from eating them.**
/// `cleanup.rs::reap_stale_transfer_temps` matches on the `.cmdr-tmp-` marker
/// plus an age, and it runs at the start of every copy and every volume move
/// into a directory. A temp left here is committed data with no ledger entry
/// (`staged_write::commit` deregisters it before landing), so an hour later the
/// next transfer into the same folder would delete the user's only copy. A file
/// called `notes (recovered).txt` is one no sweep can match.
///
/// Answers where the bytes ARE, which is never nothing: if the rescue rename
/// fails too (the same dead connection that failed the finalize), the temp path
/// is the honest answer and the caller reports that instead.
///
/// ❗ **Only `AlreadyExists` earns another try.** The rename that brought us here
/// has already failed once, so a dead link, a read-only share, or a refused name
/// would fail identically under every candidate: walking eight of them would add
/// eight timeouts to an error path the user is waiting on. A name that is merely
/// TAKEN is the one answer a different name fixes.
pub(in crate::file_system::write_operations::transfer) async fn rescue_out_of_temp_space(
    dest_volume: &Arc<dyn Volume>,
    temp: &Path,
    orig: &Path,
) -> PathBuf {
    let recovered = recovered_sibling(orig);
    match dest_volume.rename(temp, &recovered, false).await {
        Ok(()) => return recovered,
        Err(VolumeError::AlreadyExists(_)) => {}
        Err(_) => return keeps_its_temp_name(temp),
    }
    // Taken (an earlier rescue of the same file, or the user's own): continue
    // the house ` (N)` series off the recovered name.
    let mut candidates = NameCandidates::for_file(&recovered);
    while candidates.attempts() < RESCUE_NAME_ATTEMPTS {
        let candidate = candidates.current();
        match dest_volume.rename(temp, &candidate, false).await {
            Ok(()) => return candidate,
            Err(VolumeError::AlreadyExists(_)) => candidates.advance(),
            Err(_) => return keeps_its_temp_name(temp),
        }
    }
    keeps_its_temp_name(temp)
}

/// What a landing's delete of the name in the way left there, after the delete
/// answered with an error.
///
/// The answer to a delete isn't the delete: over a network backend the server
/// can act and the response still get lost, so the error alone can't say
/// whether the name is still taken. One stat can, and it runs only on this
/// failure path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::file_system::write_operations::transfer) enum AfterARefusedDelete {
    /// The name is still taken: the delete really didn't happen, nothing was
    /// cleared, and the temp is a spare copy of bytes the source still holds.
    StillThere,
    /// The name is empty after all (the delete went through despite its answer,
    /// or someone else removed the file). The way is clear, so the landing goes
    /// ahead: leaving the temp unplaced would leave NEITHER file at the name.
    Gone,
    /// The destination couldn't say. ❗ The temp stays: if the name IS empty, it
    /// is the only copy at the destination, and this is no moment to guess.
    Unknown,
}

pub(in crate::file_system::write_operations::transfer) async fn what_a_refused_delete_left(
    dest_volume: &Arc<dyn Volume>,
    path: &Path,
) -> AfterARefusedDelete {
    match dest_volume.get_metadata(path).await {
        Ok(_) => AfterARefusedDelete::StillThere,
        Err(VolumeError::NotFound(_)) => AfterARefusedDelete::Gone,
        Err(e) => {
            log::warn!(
                target: "copy",
                "landing: couldn't tell whether {} survived a refused delete ({e}), so the temp stays",
                path.display()
            );
            AfterARefusedDelete::Unknown
        }
    }
}

/// Takes away a complete temp whose bytes will NOT take their name, because
/// nothing was cleared for them: the name still holds what it held, and the
/// source still holds the new bytes.
///
/// ❗ Only ever called with a temp THIS operation minted (a `.cmdr-tmp-<uuid>`
/// name nobody else picks), and ❌ never once a landing has cleared the name:
/// from then on the temp may be the only copy at the destination, and it goes
/// through [`rescue_out_of_temp_space`] instead.
///
/// Best effort, one delete, and answers whether the temp is gone. A delete that
/// fails too (the same dead link that failed the landing) is logged and left to
/// the sweeps: the operation's abandoned-write sweep if the temp is still
/// registered, and `cleanup.rs::reap_stale_transfer_temps` on the next transfer
/// into that folder once it is an hour old.
pub(in crate::file_system::write_operations::transfer) async fn discard_unplaced_temp(
    dest_volume: &Arc<dyn Volume>,
    temp: &Path,
) -> bool {
    match dest_volume.delete(temp).await {
        Ok(()) | Err(VolumeError::NotFound(_)) => true,
        Err(e) => {
            log::warn!(
                target: "copy",
                "landing: couldn't remove the unplaced temp {} ({e}); the stale-temp sweep will",
                temp.display()
            );
            false
        }
    }
}

/// The rescue couldn't happen, so the bytes stay where they are and the caller
/// reports THAT path. Says so loudly: this is the one shape in which committed
/// data still wears a name `cleanup.rs::reap_stale_transfer_temps` matches.
fn keeps_its_temp_name(temp: &Path) -> PathBuf {
    log::warn!(
        "rescue_out_of_temp_space: couldn't move {} to a real filename, so the new data stays under its temp name",
        temp.display()
    );
    temp.to_path_buf()
}
