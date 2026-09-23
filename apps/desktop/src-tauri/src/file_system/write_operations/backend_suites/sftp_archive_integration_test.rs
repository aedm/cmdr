//! Archives on a live SFTP server: browse and extract over ranged reads, the
//! write-routing predicate, a remote edit and its cancel, a copy into a remote
//! zip, and a compress that lands a new one (or replaces a look-alike in place).
//!
//! The scenarios are backend-blind and live in `network_archive_test_support.rs`;
//! the cells stay here because the lane selects them by the `sftp_integration_`
//! prefix. SMB twins: `smb_archive_integration_test.rs` and the compress cells in
//! `smb_transfer_semantics_test.rs` / `smb_look_alike_test.rs`.

use super::network_archive_test_support::{
    a_cancel_before_the_swap_keeps_the_original_zip, a_compress_onto_the_server_lands_a_valid_zip,
    a_compress_replaces_a_look_alike_archive_in_place, a_remote_zip_edit_deletes_an_entry_on_the_server,
    a_zip_inner_path_on_the_server_routes_as_inside_the_archive, a_zip_on_the_server_browses_and_extracts,
    local_files_copied_into_a_zip_on_the_server_join_it,
};
use super::sftp_test_support::{SftpFixture, fixture, fixture_on};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_a_zip_on_the_server_browses_and_extracts() {
    let (remote, dir) = fixture("zip-browse").await;
    a_zip_on_the_server_browses_and_extracts(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_a_zip_inner_path_on_the_server_routes_as_inside_the_archive() {
    let (remote, dir) = fixture("zip-routing").await;
    a_zip_inner_path_on_the_server_routes_as_inside_the_archive(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_a_remote_zip_edit_deletes_an_entry_on_the_server() {
    let (remote, dir) = fixture("zip-edit").await;
    a_remote_zip_edit_deletes_an_entry_on_the_server(remote, dir).await;
}

/// On the server without `posix-rename@openssh.com`, where the swap can't be
/// one atomic rename.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_a_remote_zip_edit_deletes_an_entry_without_posix_rename() {
    let (remote, dir) = fixture_on(SftpFixture::NoPosixRename, "zip-edit-plain").await;
    a_remote_zip_edit_deletes_an_entry_on_the_server(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_a_cancel_before_the_swap_keeps_the_original_zip() {
    let (remote, dir) = fixture("zip-edit-cancel").await;
    a_cancel_before_the_swap_keeps_the_original_zip(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_local_files_copied_into_a_zip_on_the_server_join_it() {
    let (remote, dir) = fixture("zip-copy-into").await;
    local_files_copied_into_a_zip_on_the_server_join_it(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_a_compress_onto_the_server_lands_a_valid_zip() {
    let (remote, dir) = fixture("compress").await;
    a_compress_onto_the_server_lands_a_valid_zip(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_a_compress_replaces_a_look_alike_archive_in_place() {
    let (remote, dir) = fixture("compress-look-alike").await;
    a_compress_replaces_a_look_alike_archive_in_place(remote, dir).await;
}
