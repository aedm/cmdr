//! The destination an Overwrite is replacing, held aside until whatever replaces
//! it has landed.
//!
//! Two shapes reach it. A same-volume move replaces by RENAMING, so the new
//! bytes need no temp and the only thing at risk is the entry being replaced;
//! each such aside settles the moment its one rename answers. A cross-type
//! Overwrite (a folder landing on a file, or a file on a folder) can't stage
//! the new side the way a file→file one does (`finalize.rs::finalize_safe_replace`),
//! because a folder lands leaf by leaf over the rest of the operation. Those
//! asides go to the operation's [`DisplacedLedger`] and settle when it ends.
//! The local-FS twin is `write_operations/overwrite.rs::DisplacedEntry`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::super::super::in_flight_temps::{self, ItemKind, RecordHome, TrackedRecord};
use super::super::super::state::WriteOperationState;
use super::super::super::types::{RecoveredOriginal, WriteOperationError};
use super::super::recovered_name::rescue_out_of_temp_space;
use super::cleanup::{TreeRemoval, remove_tree};
use super::transfer_error::{PathRole, map_volume_error};
use crate::file_system::staging::StagingTemp;
use crate::file_system::volume::{Volume, VolumeError};
use crate::ignore_poison::IgnorePoison;

/// The destination an Overwrite is about to replace, renamed out of the way
/// rather than deleted.
///
/// A same-volume move replaces by RENAMING the source onto the name, and a
/// rename can't clobber (`force = false`, and MTP's `force = true` doesn't
/// delete an existing dest either), so the name has to be free first. Deleting
/// to free it is what makes the gap fatal: the replacing rename is a separate
/// call, and an SMB `STATUS_SHARING_VIOLATION`, an MTP `MoveObject` refusal, or
/// a session blip fails it after the delete has already succeeded — the
/// destination gone, the source not moved. One rename aside costs one call on
/// every backend and turns that into a restore. A cross-type Overwrite has the
/// same gap, only wider: the folder replacing a file streams in over many calls,
/// any of which can fail or be cancelled.
///
/// The guard rides along so the aside stays hidden from the pane for exactly as
/// long as it is on disk. ❗ It wears the `.cmdr-temp-` marker, which
/// `cleanup.rs::reap_stale_transfer_temps` does NOT match (it reaps `.cmdr-tmp-`
/// only), so an aside nothing could put back survives for the user to find.
#[must_use = "an aside nobody settles stays under its scratch name until the next launch's sweep"]
pub(super) struct DisplacedDestination {
    aside: StagingTemp,
    original: PathBuf,
    /// Whether it's a FOLDER, which only a cross-type Overwrite sets aside and
    /// which [`DisplacedDestination::discard`] has to take down as a tree.
    is_directory: bool,
    /// The ledger's record of it, so a force-quit between the rename aside and
    /// the rename that replaces it doesn't leave the user's file wearing a
    /// scratch name with nothing that knows what it is. `None` for an operation
    /// that never named its destination volume, which production always does.
    record: Option<TrackedRecord>,
    state: Arc<WriteOperationState>,
}

/// Hand-rolled: the operation's state has no `Debug`, and the two paths are the
/// whole story.
impl std::fmt::Debug for DisplacedDestination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DisplacedDestination")
            .field("aside", &self.aside.path())
            .field("original", &self.original)
            .field("is_directory", &self.is_directory)
            .finish()
    }
}

/// Renames whatever is at `original` to a `.cmdr-temp-<uuid>` sibling, so the
/// name is free for whatever replaces it. `is_directory` is what the caller
/// knows sits there.
///
/// `Ok(None)` ⇒ nothing was there, so there is nothing to put back.
///
/// ❗ The record is VOLUME-SPACE homed, ❌ never worked out from the path: these
/// paths are the destination volume's own, and a direct SMB session's `/photos`
/// is not this Mac's. `RecordHome::volume_space` is what keeps the sweep's
/// `std::fs` off them.
pub(super) async fn displace_destination(
    state: &Arc<WriteOperationState>,
    volume: &Arc<dyn Volume>,
    original: &Path,
    is_directory: bool,
) -> Result<Option<DisplacedDestination>, WriteOperationError> {
    let aside = StagingTemp::mint_aside(original, uuid::Uuid::new_v4(), state.liveness_token());
    let record = state.dest_volume_id().map(|volume_id| {
        in_flight_temps::track_in(
            state,
            RecordHome::volume_space(volume_id),
            ItemKind::VolumeAside {
                destination: original.to_path_buf(),
            },
            aside.path(),
        )
    });
    match volume.rename(original, aside.path(), false).await {
        Ok(()) => Ok(Some(DisplacedDestination {
            aside,
            original: original.to_path_buf(),
            is_directory,
            record,
            state: Arc::clone(state),
        })),
        Err(VolumeError::NotFound(_)) => {
            retire(state, record.as_ref());
            Ok(None)
        }
        Err(e) => {
            retire(state, record.as_ref());
            Err(map_volume_error(
                &original.display().to_string(),
                PathRole::Destination,
                e,
            ))
        }
    }
}

/// Drops a record for a rename that never happened, so nothing is claimed to be
/// on disk that isn't.
fn retire(state: &Arc<WriteOperationState>, record: Option<&TrackedRecord>) {
    if let Some(record) = record {
        in_flight_temps::retire(state, record);
    }
}

impl DisplacedDestination {
    /// The replacement landed, so the entry it replaced goes. Best-effort: a
    /// leftover wears the recognizable `.cmdr-temp-<uuid>` name and becomes
    /// visible in the pane once the operation ends.
    ///
    /// A folder goes as a tree, and only here: the user answered Overwrite on a
    /// prompt naming both types, and what replaced it has fully landed.
    pub(super) async fn discard(self, volume: &Arc<dyn Volume>) {
        let removed = if self.is_directory {
            remove_tree(volume, self.aside.path(), TreeRemoval::UserChoseOverwriteAcrossTypes)
            .await
            .map_err(|e| e.error)
        } else {
            volume.delete(self.aside.path()).await
        };
        match removed {
            Ok(()) => retire(&self.state, self.record.as_ref()),
            Err(e) => {
                log::warn!(
                    target: "copy",
                    "couldn't remove the displaced destination at {}: {e}. It keeps its record.",
                    self.aside.path().display()
                );
                if let Some(record) = self.record {
                    in_flight_temps::keep_for_arrival(&self.state, record);
                }
            }
        }
    }

    /// The replacement never landed, so the user's file comes home.
    ///
    /// `None` when it did. `Some(kept_at)` when even THAT rename refused: the
    /// bytes then wear a ` (recovered)` name beside where they belong (or, if
    /// nothing landed at all, still the aside's), and the caller has to name
    /// that path in the failure the user reads, because it is the only place
    /// their file is.
    pub(super) async fn restore(self, volume: &Arc<dyn Volume>) -> Option<PathBuf> {
        match volume.rename(self.aside.path(), &self.original, false).await {
            Ok(()) => {
                retire(&self.state, self.record.as_ref());
                None
            }
            Err(e) => {
                log::warn!(
                    target: "copy",
                    "couldn't put {} back at {}: {e}",
                    self.aside.path().display(),
                    self.original.display()
                );
                let kept_at = rescue_out_of_temp_space(volume, self.aside.path(), &self.original).await;
                // The rescue either gave the bytes a real name or left them on
                // the aside. Still on the aside means the sweep is the next one
                // to try, so the record stays.
                if kept_at == self.aside.path() {
                    if let Some(record) = self.record {
                        in_flight_temps::keep_for_arrival(&self.state, record);
                    }
                } else {
                    retire(&self.state, self.record.as_ref());
                }
                Some(kept_at)
            }
        }
    }
}

/// The cross-type asides one cross-volume copy or move is holding, settled when
/// the operation ends.
///
/// A cross-type Overwrite isn't over when the resolver answers: a folder lands
/// leaf by leaf over the rest of the operation (and a deep clash can sit inside
/// any source), so the entry it replaced has to stay recoverable until the
/// operation knows how it ended. Op-wide rather than per source, because the
/// concurrent driver can ABANDON a source's future at its cancel deadline, and
/// an aside that lived in that future would go with it.
///
/// Each entry remembers whether its replacement LANDED (its whole top-level
/// source finished), which decides what an interrupted operation does with it
/// ([`DisplacedLedger::settle_interrupted`]).
// DEFAULT-OK: an empty ledger is the truth about an operation that has set
// nothing aside yet.
#[derive(Default)]
pub(super) struct DisplacedLedger {
    held: Mutex<Vec<Held>>,
}

struct Held {
    displaced: DisplacedDestination,
    landed: bool,
}

impl DisplacedLedger {
    /// Takes custody of an aside the resolver made.
    pub(super) fn hold(&self, displaced: DisplacedDestination) {
        self.held.lock_ignore_poison().push(Held {
            displaced,
            landed: false,
        });
    }

    /// The top-level source that landed at `dest_root` finished in full, so every
    /// aside at or under it was replaced by something complete.
    pub(super) fn landed_under(&self, dest_root: &Path) {
        for held in self.held.lock_ignore_poison().iter_mut() {
            if held.displaced.original.starts_with(dest_root) {
                held.landed = true;
            }
        }
    }

    /// The operation COMPLETED: every replacement is in, so every aside goes.
    pub(super) async fn discard_all(&self, volume: &Arc<dyn Volume>) {
        for held in self.take() {
            held.displaced.discard(volume).await;
        }
    }

    /// The operation was ROLLED BACK: what replaced each entry is already gone
    /// (the reversal ran first), so each comes home, newest first. One that
    /// can't is kept beside its name under a ` (recovered)` one, and answered.
    pub(super) async fn restore_all(&self, volume: &Arc<dyn Volume>) -> Vec<RecoveredOriginal> {
        let mut recovered = Vec::new();
        for held in self.take().into_iter().rev() {
            restore_into(held.displaced, volume, &mut recovered).await;
        }
        recovered
    }

    /// The operation was STOPPED or FAILED, which keeps everything that landed.
    ///
    /// An aside whose replacement landed in full goes, exactly as it would on
    /// success: the user asked for it to be replaced, and it was. Every other one
    /// comes home, and when a half-built folder already holds its name it is kept
    /// beside it under a ` (recovered)` name instead, which is answered so the
    /// failure can say where it went. Run it AFTER the partial cleanup, so a
    /// name a partial was holding is free again.
    pub(super) async fn settle_interrupted(&self, volume: &Arc<dyn Volume>) -> Vec<RecoveredOriginal> {
        let mut recovered = Vec::new();
        for held in self.take().into_iter().rev() {
            if held.landed {
                held.displaced.discard(volume).await;
            } else {
                restore_into(held.displaced, volume, &mut recovered).await;
            }
        }
        recovered
    }

    fn take(&self) -> Vec<Held> {
        std::mem::take(&mut *self.held.lock_ignore_poison())
    }
}

/// Puts one aside back, noting it in `recovered` when it had to settle for a
/// ` (recovered)` name (or, if even that refused, its scratch one).
async fn restore_into(
    displaced: DisplacedDestination,
    volume: &Arc<dyn Volume>,
    recovered: &mut Vec<RecoveredOriginal>,
) {
    let original = displaced.original.clone();
    if let Some(kept_at) = displaced.restore(volume).await {
        log::warn!(
            target: "copy",
            "kept the entry an Overwrite set aside at {} beside its name, at {}",
            original.display(),
            kept_at.display()
        );
        recovered.push(RecoveredOriginal::new(&original, &kept_at));
    }
}
