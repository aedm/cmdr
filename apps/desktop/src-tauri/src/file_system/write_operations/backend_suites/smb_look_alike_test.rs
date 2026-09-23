//! A name the share already holds under another Unicode spelling, and renames,
//! against a real SMB server (require Docker SMB containers).
//!
//! The fixture's Samba stores a name's bytes exactly as sent and matches them
//! exactly, so `café` composed (NFC) and `café` decomposed (NFD, what macOS hands
//! out for many local files) are two entries to it. SMB answers `true` to
//! `Volume::composes_new_names`, so a genuinely new name lands composed here,
//! which `a_new_name_lands_spelled_the_way_the_server_asks` asserts from that
//! answer.
//!
//! The scenarios are backend-blind and live in
//! `network_look_alike_test_support.rs` (the compress one in
//! `network_archive_test_support.rs`), shared with `sftp_look_alike_test.rs`.
//! Unit twins: `transfer/volume/look_alike_tests.rs`.
//!
//! Case-only look-alikes aren't pinned here: the fixture runs `case sensitive =
//! auto`, so Samba folds case itself and the twin can't be made.

use super::network_archive_test_support::a_compress_replaces_a_look_alike_archive_in_place;
use super::network_look_alike_test_support::{
    a_bulk_rename_skips_a_look_alike, a_decomposed_file_onto_its_composed_twin_is_skipped,
    a_decomposed_folder_merges_into_its_composed_twin, a_new_name_lands_spelled_the_way_the_server_asks,
    a_rename_never_lands_on_a_taken_name, a_same_server_move_skips_a_look_alike,
    overwriting_a_composed_twin_leaves_one_entry,
};
use super::smb_test_support::fixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_a_decomposed_file_onto_its_composed_twin_is_skipped_under_skip() {
    let (remote, dir) = fixture().await;
    a_decomposed_file_onto_its_composed_twin_is_skipped(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_overwriting_a_composed_twin_leaves_one_entry() {
    let (remote, dir) = fixture().await;
    overwriting_a_composed_twin_leaves_one_entry(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_a_decomposed_folder_merges_into_its_composed_twin() {
    let (remote, dir) = fixture().await;
    a_decomposed_folder_merges_into_its_composed_twin(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_a_new_name_lands_on_the_share_composed() {
    let (remote, dir) = fixture().await;
    a_new_name_lands_spelled_the_way_the_server_asks(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_a_same_share_move_skips_a_look_alike_instead_of_landing_beside_it() {
    let (remote, dir) = fixture().await;
    a_same_server_move_skips_a_look_alike(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_a_rename_never_lands_on_a_taken_name() {
    let (remote, dir) = fixture().await;
    a_rename_never_lands_on_a_taken_name(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_a_bulk_rename_skips_a_look_alike_and_names_new_ones_composed() {
    let (remote, dir) = fixture().await;
    a_bulk_rename_skips_a_look_alike(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_a_compress_replaces_a_look_alike_archive_in_place_and_names_new_ones_composed() {
    let (remote, dir) = fixture().await;
    a_compress_replaces_a_look_alike_archive_in_place(remote, dir).await;
}
