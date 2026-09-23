//! Archives on a live SMB share (require Docker SMB containers): browse and
//! extract through `SmbVolume::read_range`, the parent-aware write-routing
//! predicate, a remote EDIT (pull → apply → upload → swap) and its cancel before
//! the swap, local files copied into a remote zip, and a compress that lands a
//! new one.
//!
//! The scenarios are backend-blind and live in `network_archive_test_support.rs`,
//! shared with `sftp_archive_integration_test.rs`. The in-memory twins are the
//! `remote_backed_archive_*` unit tests in `archive/volume_test.rs` and
//! `archive_edit::remote_tests`.
//!
//! Every test here is `#[ignore]`d so default runs skip it. Start the
//! containers with `./apps/desktop/test/smb-servers/start.sh`, then run
//! `cargo nextest run smb_integration --run-ignored all`.

use super::network_archive_test_support::{
    a_cancel_before_the_swap_keeps_the_original_zip, a_compress_onto_the_server_lands_a_valid_zip,
    a_remote_zip_edit_deletes_an_entry_on_the_server, a_zip_inner_path_on_the_server_routes_as_inside_the_archive,
    a_zip_on_the_server_browses_and_extracts, local_files_copied_into_a_zip_on_the_server_join_it,
};
use super::smb_test_support::fixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_archive_browse_and_extract_via_read_range() {
    let (remote, dir) = fixture().await;
    a_zip_on_the_server_browses_and_extracts(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_archive_routing_detection() {
    let (remote, dir) = fixture().await;
    a_zip_inner_path_on_the_server_routes_as_inside_the_archive(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_remote_zip_edit_deletes_an_entry_through_the_share() {
    let (remote, dir) = fixture().await;
    a_remote_zip_edit_deletes_an_entry_on_the_server(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_remote_zip_edit_cancel_before_swap_keeps_original() {
    let (remote, dir) = fixture().await;
    a_cancel_before_the_swap_keeps_the_original_zip(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_local_files_copied_into_a_zip_on_the_share_join_it() {
    let (remote, dir) = fixture().await;
    local_files_copied_into_a_zip_on_the_server_join_it(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_compress_local_files_onto_the_share() {
    let (remote, dir) = fixture().await;
    a_compress_onto_the_server_lands_a_valid_zip(remote, dir).await;
}
