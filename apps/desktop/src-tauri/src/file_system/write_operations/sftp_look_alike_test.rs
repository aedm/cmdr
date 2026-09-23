//! A name the server already holds under another Unicode spelling, and renames,
//! against a live SFTP server.
//!
//! OpenSSH on Linux stores a name's bytes as sent and matches them exactly, so
//! `café` composed and `café` decomposed are two entries to it: the case the
//! look-alike guard exists for. SFTP keeps the trait default for
//! `composes_new_names` (`false`), so a new name lands spelled as the source
//! spelled it; `a_new_name_lands_spelled_the_way_the_server_asks` pins whichever
//! answer the backend gives.
//!
//! The scenarios are backend-blind and live in
//! `network_look_alike_test_support.rs`; the cells stay here because the lane
//! selects them by the `sftp_integration_` prefix. SMB twins:
//! `smb_look_alike_test.rs`.

use super::network_look_alike_test_support::{
    a_bulk_rename_skips_a_look_alike, a_decomposed_file_onto_its_composed_twin_is_skipped,
    a_decomposed_folder_merges_into_its_composed_twin, a_new_name_lands_spelled_the_way_the_server_asks,
    a_rename_never_lands_on_a_taken_name, a_same_server_move_skips_a_look_alike,
    overwriting_a_composed_twin_leaves_one_entry,
};
use super::sftp_test_support::{SftpFixture, fixture, fixture_on};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_a_decomposed_file_onto_its_composed_twin_is_skipped() {
    let (remote, dir) = fixture("look-alike-skip").await;
    a_decomposed_file_onto_its_composed_twin_is_skipped(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_overwriting_a_composed_twin_leaves_one_entry() {
    let (remote, dir) = fixture("look-alike-overwrite").await;
    overwriting_a_composed_twin_leaves_one_entry(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_a_decomposed_folder_merges_into_its_composed_twin() {
    let (remote, dir) = fixture("look-alike-merge").await;
    a_decomposed_folder_merges_into_its_composed_twin(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_a_new_name_lands_spelled_the_way_the_server_asks() {
    let (remote, dir) = fixture("look-alike-new-name").await;
    a_new_name_lands_spelled_the_way_the_server_asks(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_a_same_server_move_skips_a_look_alike() {
    let (remote, dir) = fixture("look-alike-same-move").await;
    a_same_server_move_skips_a_look_alike(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_a_rename_never_lands_on_a_taken_name() {
    let (remote, dir) = fixture("rename").await;
    a_rename_never_lands_on_a_taken_name(remote, dir).await;
}

/// The same on the server without `posix-rename@openssh.com`, where a rename
/// that must not replace anything is plain `SSH_FXP_RENAME`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_a_rename_never_lands_on_a_taken_name_without_posix_rename() {
    let (remote, dir) = fixture_on(SftpFixture::NoPosixRename, "rename-plain").await;
    a_rename_never_lands_on_a_taken_name(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the SFTP fixture stack: sftp-servers/start.sh (sftp-fixture)"]
async fn sftp_integration_a_bulk_rename_skips_a_look_alike() {
    let (remote, dir) = fixture("bulk-rename").await;
    a_bulk_rename_skips_a_look_alike(remote, dir).await;
}
