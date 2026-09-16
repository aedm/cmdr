//! What a walk that ran to the end writes about itself.
//!
//! Every write here is gated on `was_completed` by the one caller. They live
//! together rather than inline in the completion flow because they share that one
//! condition: a reader checking "what does completing a scan claim?" gets the whole
//! answer in one place, and a gate over the claim has one place to sit.

use crate::indexing::IndexPathSpace;
use crate::indexing::reconcile::reconciler;
use crate::indexing::scanner::ScanSummary;
use crate::indexing::store::ScanCalibrationKind;
use crate::indexing::writer::{IndexWriter, WriteMessage};

/// Stamp the completion marker, restart the shallow-`MustScanSubDirs` sweep window,
/// record this walk kind's calibration numbers, and store the volume's root path.
///
/// ⚠️ **Only on a walk that ran to the end.** A user-stopped scan holds only partial
/// totals, and writing `scan_completed_at` for it marks a partial index complete —
/// the next startup then skips the healing rescan and serves a permanently
/// half-built index. A cancelled scan leaves NO completion marker, so it heals on
/// restart.
pub(super) fn stamp_a_completed_walk(
    volume_id: &str,
    summary: &ScanSummary,
    space: &IndexPathSpace,
    calibration_kind: ScanCalibrationKind,
    writer: &IndexWriter,
) {
    stamp_what_every_completed_walk_records(summary, &space.volume_root_string(), calibration_kind, writer);
    // Any completed full walk restarts the shallow-`MustScanSubDirs`
    // sweep window and clears its coalesced count (the drift those
    // skipped signals stood for has now been repaired). Not only a
    // shallow-triggered sweep: the window means "a full walk happened
    // recently", so the user's own "Rescan now" counts too. See
    // `reconcile/reconciler/rescan/route.rs`.
    //
    // Local-walk only, which is why it isn't in the shared stamp above: a
    // trait-scanned volume has no FSEvents stream and so no shallow sweep to
    // restart.
    let sweep = reconciler::record_sweep_completed(volume_id, reconciler::now_unix());
    if let Some(at) = sweep.last_sweep_unix {
        let _ = writer.send(WriteMessage::UpdateMeta {
            key: reconciler::SHALLOW_SWEEP_AT_KEY.to_string(),
            value: at.to_string(),
        });
    }
    let _ = writer.send(WriteMessage::UpdateMeta {
        key: reconciler::SHALLOW_COALESCED_KEY.to_string(),
        value: "0".to_string(),
    });
}

/// The three facts EVERY completed walk records, local or trait-scanned: when it
/// finished, its calibration numbers, and the volume root it walked.
///
/// Shared between the local completion path above and `network_scan.rs`, so the
/// two-bucket calibration rule can't drift between them: a walk kind's own keys
/// (so the next run of the same kind gets an ETA from a comparable run) AND the
/// unsuffixed keys (the last-completed-scan facts the badge tooltip and the
/// any-kind fallback read).
///
/// ⚠️ **Only on a walk that ran to the end**, for the reason
/// [`stamp_a_completed_walk`] gives: `scan_completed_at` on a partial index makes
/// the next startup skip the healing rescan.
pub(in crate::indexing::lifecycle) fn stamp_what_every_completed_walk_records(
    summary: &ScanSummary,
    volume_root: &str,
    calibration_kind: ScanCalibrationKind,
    writer: &IndexWriter,
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default();
    let _ = writer.send(WriteMessage::UpdateMeta {
        key: "scan_completed_at".to_string(),
        value: now,
    });
    for (key, value) in [
        ("scan_duration_ms", summary.duration_ms.to_string()),
        ("total_entries", summary.total_entries.to_string()),
        ("total_physical_bytes", summary.total_physical_bytes.to_string()),
    ] {
        let _ = writer.send(WriteMessage::UpdateMeta {
            key: calibration_kind.meta_key(key),
            value: value.clone(),
        });
        let _ = writer.send(WriteMessage::UpdateMeta {
            key: key.to_string(),
            value,
        });
    }
    let _ = writer.send(WriteMessage::UpdateMeta {
        key: "volume_path".to_string(),
        value: volume_root.to_string(),
    });
}
