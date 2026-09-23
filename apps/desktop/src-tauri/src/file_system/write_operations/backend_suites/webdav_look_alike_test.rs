//! A name the server already holds under another Unicode spelling, and renames,
//! against a live WebDAV server.
//!
//! Apache `mod_dav` on Linux stores a name's bytes as the percent-decoded URL
//! spelled them and matches them exactly, so `café` composed and `café`
//! decomposed are two entries to it: the case the look-alike guard exists for.
//! WebDAV answers `composes_new_names` with `true` and keeps the trait default
//! for `matches_names_in_any_unicode_form` (`false`); the backend folds nothing
//! itself, so every scenario applies. A name Cmdr creates lands composed
//! (`a_new_name_lands_composed`) while an entry the server already holds
//! decomposed keeps its bytes
//! (`an_existing_decomposed_entry_keeps_its_exact_bytes`).
//!
//! The scenarios are backend-blind and live in
//! `network_look_alike_test_support.rs` (the compress one in
//! `network_archive_test_support.rs`); the cells stay here because the lane
//! selects them by the `webdav_integration_` prefix. Twins:
//! `sftp_look_alike_test.rs`, `smb_look_alike_test.rs`.

use super::network_look_alike_test_support::{
    a_bulk_rename_skips_a_look_alike, a_decomposed_file_onto_its_composed_twin_is_skipped,
    a_decomposed_folder_merges_into_its_composed_twin, a_new_name_lands_composed, a_rename_never_lands_on_a_taken_name,
    a_same_server_move_skips_a_look_alike, an_existing_decomposed_entry_keeps_its_exact_bytes,
    overwriting_a_composed_twin_leaves_one_entry,
};
use super::webdav_test_support::fixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_decomposed_file_onto_its_composed_twin_is_skipped() {
    let (remote, dir) = fixture().await;
    a_decomposed_file_onto_its_composed_twin_is_skipped(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_overwriting_a_composed_twin_leaves_one_entry() {
    let (remote, dir) = fixture().await;
    overwriting_a_composed_twin_leaves_one_entry(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_decomposed_folder_merges_into_its_composed_twin() {
    let (remote, dir) = fixture().await;
    a_decomposed_folder_merges_into_its_composed_twin(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_new_name_lands_composed() {
    let (remote, dir) = fixture().await;
    a_new_name_lands_composed(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_an_existing_decomposed_entry_keeps_its_exact_bytes() {
    let (remote, dir) = fixture().await;
    an_existing_decomposed_entry_keeps_its_exact_bytes(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_same_server_move_skips_a_look_alike() {
    let (remote, dir) = fixture().await;
    a_same_server_move_skips_a_look_alike(remote, dir).await;
}

/// A rename that must not replace anything is `MOVE` with `Overwrite: F`, which
/// the server refuses atomically on an occupied name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_rename_never_lands_on_a_taken_name() {
    let (remote, dir) = fixture().await;
    a_rename_never_lands_on_a_taken_name(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_bulk_rename_skips_a_look_alike() {
    let (remote, dir) = fixture().await;
    a_bulk_rename_skips_a_look_alike(remote, dir).await;
}
