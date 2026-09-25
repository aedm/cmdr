//! The cross-volume move's source sweep: removing a top-level source once its
//! copy has landed.
//!
//! The sweep deletes a LEDGER, never a tree, the same rule the local
//! cross-filesystem sweep follows (`move_op/source_sweep.rs`). The copy walk
//! records every source file it carried, with what the source listing said about
//! it at the time ([`SourceStamp`]), and every source folder it walked. The sweep
//! lists each walked folder once more and removes only the files that are in the
//! ledger AND still match their stamp; a folder goes only once it's empty
//! (`Volume::delete` is empty-only for a directory on every backend).
//!
//! Everything else stays, because the source holds the only copy of it:
//!
//! - **Appeared**: an entry the walk never saw, a download or a sync landing
//!   while the folder copied. Counted once per unknown subtree.
//! - **Changed**: a carried file whose listing no longer matches its stamp, an
//!   app saving over it after the walk listed it. The destination has the old
//!   bytes.
//! - **Skipped**: a child a merge conflict resolved to Skip. The user asked for
//!   that, so it's not counted as news.
//!
//! ❌ Never go back to a recursive delete of the source (`remove_tree`): it acts
//! on what is on disk NOW rather than on what this move carried.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::transfer_error::{AtPath, PathedVolumeError};
use crate::file_system::listing::FileEntry;
use crate::file_system::volume::{Volume, VolumeError};

/// What a source entry looked like to the move: its size, its mtime, and its
/// inode where the backend has one. Compared field by field, so a field a backend
/// never reports (an MTP device with no mtime, every non-local inode) drops out
/// of the comparison rather than failing it.
///
/// An mtime is sound here though the in-flight ledgers refuse one
/// (`../DETAILS.md` § "What the in-flight ledgers record"): both reads ask the
/// SAME backend about the SAME file the same way, so a coarse clock truncates
/// both alike and can't invent a change. What it can hide is a same-size save
/// inside one tick (the listing's clock is whole seconds); a local source's
/// inode covers the save-by-rename case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SourceStamp {
    size: Option<u64>,
    modified_at: Option<u64>,
    inode: Option<u64>,
}

impl SourceStamp {
    pub(super) fn of(entry: &FileEntry) -> Self {
        Self {
            size: entry.size,
            modified_at: entry.modified_at,
            inode: entry.inode,
        }
    }
}

/// What one top-level source's copy walk carried: the source files it copied
/// (with their stamps) and the source folders it listed.
// DEFAULT-OK: an empty ledger is a walk that has carried nothing yet, and a sweep
// over it removes nothing.
#[derive(Default)]
pub(super) struct SourceLedger {
    files: HashMap<PathBuf, SourceStamp>,
    walked_dirs: HashSet<PathBuf>,
}

impl SourceLedger {
    pub(super) fn record_walked_dir(&mut self, dir: PathBuf) {
        self.walked_dirs.insert(dir);
    }

    pub(super) fn record_carried_file(&mut self, file: PathBuf, stamp: SourceStamp) {
        self.files.insert(file, stamp);
    }
}

/// What the sweep left in one source folder.
// DEFAULT-OK: zero is "the sweep has left nothing here yet", true of a sweep
// that hasn't run and of one that took everything.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct FolderLeftovers {
    pub(super) appeared: u32,
    pub(super) changed: u32,
}

/// Removes a moved source folder from the ledger its copy walk kept, sparing
/// `skipped` (the merge's Skips) and whatever appeared or changed.
///
/// A child that refuses to go keeps its folder alive, and the FIRST such failure
/// comes out with the child's own path: the leaf is the diagnosis, the folder's
/// `ENOTEMPTY` would only be its symptom. The sweep keeps going past a failure,
/// so it clears what it can.
pub(super) async fn sweep_moved_folder(
    volume: &Arc<dyn Volume>,
    folder: &Path,
    ledger: &SourceLedger,
    skipped: &HashSet<PathBuf>,
) -> Result<FolderLeftovers, PathedVolumeError> {
    let mut left = FolderLeftovers::default();
    sweep_level(volume, folder, ledger, skipped, &mut left).await?;
    Ok(left)
}

/// One folder of the sweep. `Ok(true)` means something stays under it, so the
/// caller keeps it.
async fn sweep_level(
    volume: &Arc<dyn Volume>,
    dir: &Path,
    ledger: &SourceLedger,
    skipped: &HashSet<PathBuf>,
    left: &mut FolderLeftovers,
) -> Result<bool, PathedVolumeError> {
    let entries = match volume.list_directory(dir, None).await {
        Ok(entries) => entries,
        // Gone already: whoever removed it, nothing of it is left to protect.
        Err(VolumeError::NotFound(_)) => return Ok(false),
        Err(e) => return Err(e).at(dir),
    };

    let mut remains = false;
    let mut first_failure: Option<PathedVolumeError> = None;
    for entry in &entries {
        let path = PathBuf::from(&entry.path);
        let outcome = if skipped.contains(&path) {
            Ok(true)
        } else if entry.is_directory {
            if ledger.walked_dirs.contains(&path) {
                Box::pin(sweep_level(volume, &path, ledger, skipped, left)).await
            } else {
                left.appeared += 1;
                Ok(true)
            }
        } else {
            match ledger.files.get(&path) {
                None => {
                    left.appeared += 1;
                    Ok(true)
                }
                Some(stamp) if *stamp != SourceStamp::of(entry) => {
                    left.changed += 1;
                    Ok(true)
                }
                Some(_) => volume.delete(&path).await.at(&path).map(|()| false),
            }
        };
        match outcome {
            Ok(stays) => remains |= stays,
            Err(e) => {
                log::warn!(
                    target: "move",
                    "source sweep: couldn't remove {}: {:?}",
                    e.path.display(),
                    e.error
                );
                remains = true;
                first_failure.get_or_insert(e);
            }
        }
    }

    if remains {
        return match first_failure {
            Some(failure) => Err(failure),
            None => Ok(true),
        };
    }
    volume.delete(dir).await.at(dir).map(|()| false)
}

/// The stamp of one top-level source FILE, read right before its copy so a save
/// during the copy counts as a change too. `None` when the stat fails, which
/// the check below reads as unprovable.
pub(super) async fn stamp_file(volume: &Arc<dyn Volume>, file: &Path) -> Option<SourceStamp> {
    volume
        .get_metadata(file)
        .await
        .ok()
        .map(|entry| SourceStamp::of(&entry))
}

/// Whether a top-level source file still looks the way it did before its copy,
/// so the move may delete it. A stamp that couldn't be taken proves nothing and
/// keeps the file. A file already gone answers `true` and leaves the verdict to
/// the delete.
pub(super) async fn file_is_unchanged(
    volume: &Arc<dyn Volume>,
    file: &Path,
    before: Option<SourceStamp>,
) -> Result<bool, PathedVolumeError> {
    match volume.get_metadata(file).await {
        Ok(now) => Ok(before == Some(SourceStamp::of(&now))),
        Err(VolumeError::NotFound(_)) => Ok(true),
        Err(e) => Err(e).at(file),
    }
}
