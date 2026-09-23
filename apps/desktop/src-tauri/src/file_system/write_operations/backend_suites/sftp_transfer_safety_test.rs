//! Data-safety cells for the SFTP backend, on a live server: a failed merge that
//! must not sweep the user's folder, a failed move that must lose no byte, a
//! delete bound to its preview, a recursive delete that takes exactly the
//! selection, an unanswerable source probe, and a download stopped mid-file.
//!
//! The scenarios are backend-blind and live in `network_safety_test_support.rs`;
//! the cells stay here because the integration lane selects them by the
//! `sftp_integration_` name prefix. The SMB twins are `smb_transfer_safety_test.rs`.

use super::super::types::ConflictResolution;
use super::network_safety_test_support::{
    STAGED_ON_EVERY_BACKEND, a_cancelled_download_leaves_nothing_behind,
    a_delete_of_a_non_empty_folder_takes_exactly_the_selection,
    a_delete_with_a_local_shaped_preview_removes_only_the_requested_tree,
    a_failed_folder_copy_onto_a_users_folder_keeps_their_files, a_failed_folder_move_onto_a_users_folder_loses_no_byte,
    a_name_taken_mid_upload_is_never_replaced, an_unknown_source_type_never_clears_a_server_folder,
};
use super::sftp_test_support::fixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_a_name_taken_mid_upload_is_never_replaced() {
    let (remote, dir) = fixture("name-taken-mid-upload").await;
    a_name_taken_mid_upload_is_never_replaced(remote, dir, ConflictResolution::Skip, STAGED_ON_EVERY_BACKEND).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_a_failed_folder_copy_onto_a_users_folder_keeps_their_files() {
    let (remote, dir) = fixture("failed-merge-copy").await;
    a_failed_folder_copy_onto_a_users_folder_keeps_their_files(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_a_failed_folder_move_onto_a_users_folder_loses_no_byte() {
    let (remote, dir) = fixture("failed-merge-move").await;
    a_failed_folder_move_onto_a_users_folder_loses_no_byte(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_a_delete_with_a_local_shaped_preview_removes_only_the_requested_tree() {
    let (remote, dir) = fixture("delete-preview").await;
    a_delete_with_a_local_shaped_preview_removes_only_the_requested_tree(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_a_delete_of_a_non_empty_folder_takes_exactly_the_selection() {
    let (remote, dir) = fixture("delete-tree").await;
    a_delete_of_a_non_empty_folder_takes_exactly_the_selection(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_an_unknown_source_type_never_clears_a_server_folder() {
    let (remote, dir) = fixture("unknown-source-type").await;
    an_unknown_source_type_never_clears_a_server_folder(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_a_cancelled_download_leaves_nothing_behind() {
    let (remote, dir) = fixture("cancelled-download").await;
    a_cancelled_download_leaves_nothing_behind(remote, dir).await;
}
