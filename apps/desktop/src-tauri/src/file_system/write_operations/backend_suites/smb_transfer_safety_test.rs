//! Data-safety integration cells for the SMB backend, on a REAL share (require
//! Docker SMB containers): a failed merge that must not sweep the user's share,
//! a failed move that must lose no byte, a delete bound to its preview, a
//! recursive delete that takes exactly the selection, an unanswerable source
//! probe, and a download stopped mid-file.
//!
//! The scenarios are backend-blind and live in `network_safety_test_support.rs`,
//! shared with the SFTP suite (`sftp_transfer_safety_test.rs`), so a claim can't
//! hold on one backend and rot on another. `safety_grid_tests.rs` covers the same
//! axes against doubles and says why these stay against a real share. Same
//! gating as every SMB cell: start the containers with
//! `./apps/desktop/test/smb-servers/start.sh` and run
//! `cargo nextest run smb_integration --run-ignored all`.

use super::super::types::ConflictResolution;
use super::network_safety_test_support::{
    FITS_ONE_SMB_WRITE, STAGED_ON_EVERY_BACKEND, a_cancelled_download_leaves_nothing_behind,
    a_delete_of_a_non_empty_folder_takes_exactly_the_selection,
    a_delete_with_a_local_shaped_preview_removes_only_the_requested_tree,
    a_failed_folder_copy_onto_a_users_folder_keeps_their_files, a_failed_folder_move_onto_a_users_folder_loses_no_byte,
    a_name_taken_mid_upload_is_never_replaced, an_unknown_source_type_never_clears_a_server_folder,
};
use super::smb_test_support::fixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_a_name_taken_mid_upload_is_never_replaced() {
    let (remote, dir) = fixture().await;
    a_name_taken_mid_upload_is_never_replaced(remote, dir, ConflictResolution::Skip, STAGED_ON_EVERY_BACKEND).await;
}

/// ❗ KNOWN GAP, red today: the same race on a file SMB lands SINGLE-SHOT.
///
/// A write that fits one compound frame skips staging (`resolve_staging`) and
/// goes straight to its final name through smb2's `write_file_compound`, whose
/// CREATE uses `FileOverwriteIf`. So it never meets the no-replace landing the
/// staged path relies on, and a file another writer put at the name while ours
/// was buffering is REPLACED, under Skip, with the copy reporting success
/// (verified against the fixture's Samba, 2026-09-23: `completed: true`, the
/// name holding our bytes). Closing it needs an exclusive (`FileCreate`)
/// compound write in smb2 and a way for `write_from_stream` to learn the name
/// must be new; that design is open, so the cell sits outside the lane.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
// allowed-out-of-lane-fixture-cell: pins a known SMB data-safety gap (single-shot uploads
// replace a name taken mid-upload) until the exclusive compound write lands; in the lane
// it would fail every run.
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_known_gap_a_single_shot_upload_replaces_a_name_taken_mid_upload() {
    let (remote, dir) = fixture().await;
    a_name_taken_mid_upload_is_never_replaced(remote, dir, ConflictResolution::Skip, FITS_ONE_SMB_WRITE).await;
}

/// THE PRODUCTION BUG, ON THE WIRE: an empty-`per_path` cache hit made the driver
/// believe the source was a FILE, and the cleanup guard keyed on that belief
/// then swept the MERGED destination root off the NAS.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_failed_dir_copy_onto_a_merged_share_folder_keeps_the_users_files() {
    let (remote, dir) = fixture().await;
    a_failed_folder_copy_onto_a_users_folder_keeps_their_files(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_failed_dir_move_onto_a_merged_share_folder_loses_no_byte() {
    let (remote, dir) = fixture().await;
    a_failed_folder_move_onto_a_users_folder_loses_no_byte(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_delete_with_a_local_shaped_preview_removes_only_the_requested_tree() {
    let (remote, dir) = fixture().await;
    a_delete_with_a_local_shaped_preview_removes_only_the_requested_tree(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_a_delete_of_a_non_empty_folder_takes_exactly_the_selection() {
    let (remote, dir) = fixture().await;
    a_delete_of_a_non_empty_folder_takes_exactly_the_selection(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_unknown_source_type_never_clears_a_share_folder() {
    let (remote, dir) = fixture().await;
    an_unknown_source_type_never_clears_a_server_folder(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_a_cancelled_download_leaves_nothing_behind() {
    let (remote, dir) = fixture().await;
    a_cancelled_download_leaves_nothing_behind(remote, dir).await;
}
