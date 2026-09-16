//! The destination a same-volume Overwrite is replacing, held aside until the
//! rename that replaces it has landed.
//!
//! The cross-volume side's twin is `finalize.rs::finalize_safe_replace` (a temp
//! holding the NEW bytes); the local-FS side's is
//! `write_operations/overwrite.rs::DisplacedEntry`. This is the one for a move
//! that replaces by RENAMING, where the new bytes need no temp at all and the
//! only thing at risk is the file being replaced.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::super::super::in_flight_temps::{self, ItemKind, RecordHome, TrackedRecord};
use super::super::super::state::WriteOperationState;
use super::super::super::types::WriteOperationError;
use super::super::recovered_name::rescue_out_of_temp_space;
use super::transfer_error::{PathRole, map_volume_error};
use crate::file_system::staging::StagingTemp;
use crate::file_system::volume::{Volume, VolumeError};

/// The destination a same-volume Overwrite is about to replace, renamed out of
/// the way rather than deleted.
///
/// A same-volume move replaces by RENAMING the source onto the name, and a
/// rename can't clobber (`force = false`, and MTP's `force = true` doesn't
/// delete an existing dest either), so the name has to be free first. Deleting
/// to free it is what makes the gap fatal: the replacing rename is a separate
/// call, and an SMB `STATUS_SHARING_VIOLATION`, an MTP `MoveObject` refusal, or
/// a session blip fails it after the delete has already succeeded — the
/// destination gone, the source not moved. One rename aside costs one call on
/// every backend and turns that into a restore.
///
/// The guard rides along so the aside stays hidden from the pane for exactly as
/// long as it is on disk. ❗ It wears the `.cmdr-temp-` marker, which
/// `cleanup.rs::reap_stale_transfer_temps` does NOT match (it reaps `.cmdr-tmp-`
/// only), so an aside nothing could put back survives for the user to find.
pub(super) struct DisplacedDestination {
    aside: StagingTemp,
    original: PathBuf,
    /// The ledger's record of it, so a force-quit between the rename aside and
    /// the rename that replaces it doesn't leave the user's file wearing a
    /// scratch name with nothing that knows what it is. `None` for an operation
    /// that never named its destination volume, which production always does.
    record: Option<TrackedRecord>,
    state: Arc<WriteOperationState>,
}

/// Renames whatever is at `original` to a `.cmdr-temp-<uuid>` sibling, so the
/// name is free for the rename that replaces it.
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
    /// The replacement landed, so the file it replaced goes. Best-effort: a
    /// leftover wears the recognizable `.cmdr-temp-<uuid>` name and becomes
    /// visible in the pane once the operation ends.
    pub(super) async fn discard(self, volume: &Arc<dyn Volume>) {
        match volume.delete(self.aside.path()).await {
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
