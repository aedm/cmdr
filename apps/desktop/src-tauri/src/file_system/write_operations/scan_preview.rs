//! Scan preview subsystem for the Copy dialog.
//!
//! Provides background scanning that feeds live stats to the frontend before
//! the actual copy starts. Results are cached so the copy can skip a redundant
//! scan.
//!
//! ## What a worker owes its preview
//!
//! Each worker ends by publishing a terminal OUTCOME through [`settle_preview`]
//! — complete with its result, errored with the typed failure a waiting
//! operation reports (`settle_failed_preview`), or cancelled — in the
//! same write that retires the in-flight state. An operation waiting on the preview may not
//! spawn for minutes behind a busy lane, so "look it up and find nothing" is
//! the common case rather than the rare one, and "nothing" would be ambiguous
//! four ways: complete-and-consumed, errored, cancelled, and never-existed.
//!
//! ⚠️ The `Cancelled` outcome comes from the worker's own cancel flag at its
//! exit, ❌ never from which event fired. A genuinely cancelled walk returns an
//! error (the local walk's `on_cancelled` string, the volume path's
//! `VolumeError::Cancelled`), so classifying on the event would reach the
//! operation as a FAILURE whose message happens to say "cancelled", and
//! recovering the truth from that message would be string-matching on the
//! control path.
//!
//! ## The progress bridge
//!
//! When an operation has claimed a preview, each progress tick is ALSO
//! republished under that operation's id as a scanning-phase `write-progress`
//! (`super::scan_bridge`). Both events keep firing: a pre-confirm dialog may
//! still be watching the same preview by `previewId`, and it has no operation
//! to watch instead.

//!
//! ## Layout
//!
//! This file holds the lifecycle API the IPC commands call (start, totals,
//! cancel) and the one settle both workers share on failure. The workers build
//! their own tallies and share nothing else: `local` walks the local filesystem
//! (with the compress-estimate sampler), and `volume` scans through a `Volume`
//! with the fresh-listing oracle in front.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use uuid::Uuid;

use super::event_sinks::{ScanPreviewEventSink, TauriScanPreviewSink};
use super::scan_cache::{
    ScanOutcome, cached_scan_totals, claimed_operation, in_flight_state, register_preview, release_preview,
    settle_preview,
};
use super::scan_watchdog::{SCAN_INACTIVITY_LIMIT, ScanTarget, ScanWatchdog};
use super::state::ScanPreviewState;
use super::types::{ScanPreviewErrorEvent, ScanPreviewStartResult, WriteOperationError};
use crate::file_system::listing::{SortColumn, SortOrder};
use crate::file_system::volume::Volume;

mod local;
mod volume;

pub(super) use local::run_scan_preview;
#[cfg(test)]
pub(super) use volume::run_oracle_aware_batch_scan;
pub(super) use volume::run_volume_scan_preview;

/// Starts a scan preview for the Copy dialog.
/// Returns a preview_id that can be used to cancel or to pass to copy_files.
///
/// When `source_volume` is provided, uses `Volume::scan_for_copy()` instead of `std::fs`,
/// enabling MTP and other non-local volumes to produce scan previews.
///
/// `source_volume_id` identifies the volume the sources live on. It's used by the
/// fresh-listing oracle (`try_get_authoritative_listing`) to short-circuit re-reading
/// directories that an open pane is already keeping in sync. Pass `"root"` for
/// local-FS scans.
/// `sample_for_estimate` turns on the compressed-size sampler for the LOCAL
/// walk only (compress-mode scans). It's ignored for volume/remote scans, which
/// never sample (the estimate is suppressed there). See `compress_estimate`.
#[allow(
    clippy::too_many_arguments,
    reason = "IPC pass-through mirroring the command's parameter list"
)]
pub fn start_scan_preview(
    app: tauri::AppHandle,
    sources: Vec<PathBuf>,
    source_volume: Option<Arc<dyn Volume>>,
    source_volume_id: String,
    sort_column: SortColumn,
    sort_order: SortOrder,
    progress_interval_ms: u64,
    sample_for_estimate: bool,
) -> ScanPreviewStartResult {
    let preview_id = Uuid::new_v4().to_string();
    let preview_id_clone = preview_id.clone();

    let state = Arc::new(ScanPreviewState {
        cancelled: AtomicBool::new(false),
        progress_interval: Duration::from_millis(progress_interval_ms),
    });

    register_preview(preview_id.clone(), Arc::clone(&state));

    let events: Arc<dyn ScanPreviewEventSink> = Arc::new(TauriScanPreviewSink::new(app));
    // Bounds the walk by INACTIVITY and narrates it in the log. Started here,
    // before either worker spawns, so a walk that wedges on its very first
    // syscall is still bounded — the workers only feed it.
    let watchdog = ScanWatchdog::start(
        preview_id.clone(),
        ScanTarget::of(&sources, &source_volume_id),
        SCAN_INACTIVITY_LIMIT,
        Arc::clone(&state),
        Arc::clone(&events),
    );

    // Spawn background task.
    // Volume scans need a Tokio runtime context (MtpVolume uses Handle::block_on),
    // so we capture the runtime handle and enter it on the spawned thread.
    // Local scans use std::thread directly (no runtime needed).
    if let Some(volume) = source_volume {
        tokio::spawn(async move {
            run_volume_scan_preview(
                events,
                preview_id_clone,
                sources,
                volume,
                source_volume_id,
                state,
                watchdog,
            )
            .await;
        });
    } else {
        std::thread::spawn(move || {
            run_scan_preview(
                events,
                preview_id_clone,
                sources,
                sort_column,
                sort_order,
                state,
                sample_for_estimate,
                watchdog,
            );
        });
    }

    ScanPreviewStartResult { preview_id }
}

/// Returns the cached totals from a completed scan preview, or `None` if the
/// scan is still running, was cancelled, or errored. The FE uses this both to
/// know whether the scan is done AND to recover its display state when the
/// scan-preview events fired before listeners were attached (a real race
/// surfaced by M2a's watcher-backed oracle, which can complete a scan in
/// ~5 ms — faster than the FE's `await startScanPreview()` IPC round-trip).
pub fn get_scan_preview_totals(preview_id: &str) -> Option<super::types::ScanPreviewTotals> {
    cached_scan_totals(preview_id)
}

/// Cancels a running scan preview AND frees any cached result.
///
/// Cancelling sets the in-flight cancel flag (a still-running scan exits
/// promptly). Freeing the cached result covers the dialog-dismissed-after-scan-
/// completed case: the FE calls this on every dialog teardown, regardless of
/// whether the scan was still running, so a completed-but-unconsumed
/// `CachedScanResult` (tens of thousands of `FileInfo`) doesn't linger until
/// quit. Consuming the result for a started op goes through
/// `take_cached_scan_result` instead, which already removes it.
///
/// ⚠️ A preview an OPERATION has claimed is left alone. Its owner decides when
/// it ends (`cancel_operation` reaches it through the operation's own cancel
/// token), and freeing it from a dialog teardown would pull the result out from
/// under a transfer that is waiting on it.
pub fn cancel_scan_preview(preview_id: &str) {
    if let Some(owner) = claimed_operation(preview_id) {
        log::debug!(
            target: "cmdr_lib::write_operations",
            "scan preview {preview_id} is owned by operation {owner}; leaving it to its operation"
        );
        return;
    }
    if let Some(state) = in_flight_state(preview_id) {
        state.cancelled.store(true, Ordering::Relaxed);
    }
    release_preview(preview_id);
}

/// Publishes a walk that stopped on an error: the typed failure for a waiting
/// operation, and the dialog's notice. The dialog reads only `timed_out` off its
/// event, so the message there is the failure's technical form, for the log.
fn settle_failed_preview(
    events: &dyn ScanPreviewEventSink,
    watchdog: &ScanWatchdog,
    preview_id: String,
    error: WriteOperationError,
) {
    watchdog.note_settled("stopped");
    let message = format!("{error:?}");
    settle_preview(&preview_id, ScanOutcome::Error(error), None);
    events.emit_error(ScanPreviewErrorEvent {
        preview_id,
        message,
        timed_out: false,
    });
}
