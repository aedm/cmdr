//! Data-safety cells for the WebDAV backend, on a live server: a failed merge
//! that must not sweep the user's folder, a failed move that must lose no byte,
//! a delete bound to its preview, a recursive delete that takes exactly the
//! selection, an unanswerable source probe, and a download stopped mid-file.
//!
//! ❗ The two delete cells matter more here than anywhere else: a WebDAV
//! `DELETE` on a collection is recursive by protocol, so the one thing standing
//! between a delete that previewed one tree and a server that drops a bigger
//! one is `Volume::delete` refusing a non-empty collection
//! (`crates/cmdr-webdav/src/volume/mutation.rs`).
//!
//! The scenarios are backend-blind and live in `network_safety_test_support.rs`;
//! the cells stay here because the integration lane selects them by the
//! `webdav_integration_` name prefix. Twins: `sftp_transfer_safety_test.rs`,
//! `smb_transfer_safety_test.rs`.

use super::network_safety_test_support::{
    a_cancelled_download_leaves_nothing_behind, a_delete_of_a_non_empty_folder_takes_exactly_the_selection,
    a_delete_with_a_local_shaped_preview_removes_only_the_requested_tree,
    a_failed_folder_copy_onto_a_users_folder_keeps_their_files, a_failed_folder_move_onto_a_users_folder_loses_no_byte,
    an_unknown_source_type_never_clears_a_server_folder,
};
use super::webdav_test_support::fixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_failed_folder_copy_onto_a_users_folder_keeps_their_files() {
    let (remote, dir) = fixture().await;
    a_failed_folder_copy_onto_a_users_folder_keeps_their_files(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_failed_folder_move_onto_a_users_folder_loses_no_byte() {
    let (remote, dir) = fixture().await;
    a_failed_folder_move_onto_a_users_folder_loses_no_byte(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_delete_with_a_local_shaped_preview_removes_only_the_requested_tree() {
    let (remote, dir) = fixture().await;
    a_delete_with_a_local_shaped_preview_removes_only_the_requested_tree(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_delete_of_a_non_empty_folder_takes_exactly_the_selection() {
    let (remote, dir) = fixture().await;
    a_delete_of_a_non_empty_folder_takes_exactly_the_selection(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_an_unknown_source_type_never_clears_a_server_folder() {
    let (remote, dir) = fixture().await;
    an_unknown_source_type_never_clears_a_server_folder(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_cancelled_download_leaves_nothing_behind() {
    let (remote, dir) = fixture().await;
    a_cancelled_download_leaves_nothing_behind(remote, dir).await;
}
