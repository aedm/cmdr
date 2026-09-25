//! What a transfer MEANS on a live WebDAV server: merges, policies, moves in
//! both directions, same-server moves and copies, and load.
//!
//! The backend-blind scenarios live in `network_semantics_test_support.rs`,
//! shared with SFTP and SMB so a claim can't hold on one and quietly rot on
//! another. The cells stay here because the integration lane selects them by
//! the `webdav_integration_` name prefix.
//!
//! What these reach on this backend: a same-server move is one `MOVE` with
//! `Overwrite: F` per item (the server refuses an occupied name, so the merge
//! engine hears `AlreadyExists` and descends), and a same-server copy is one
//! `COPY` per file onto a staging sibling, so no byte crosses the link.
//!
//! The dialog-addressed destination cells SFTP and SMB carry have no WebDAV twin
//! here: the fixture's remote root is `/`, where a volume-relative path and a
//! server-absolute one are spelled alike, so a doubled root can't be told from
//! a right one. `crates/cmdr-webdav/src/volume/paths_test.rs` pins the
//! translation, the bare-path refusal included, with a non-root root.

use super::network_move_drift_test_support::{
    a_file_added_mid_move_off_the_server_stays, a_file_saved_over_mid_move_off_the_server_stays,
};
use super::network_semantics_test_support::{
    a_copy_into_a_missing_nested_destination_makes_every_level,
    a_deep_clash_merge_under_overwrite_replaces_only_the_clash,
    a_deep_clash_merge_under_skip_keeps_every_dest_only_file, a_folder_moved_off_the_server_leaves_no_source,
    a_folder_moved_onto_the_server_leaves_no_source, a_move_merge_onto_the_server_spares_what_it_skipped,
    a_multi_megabyte_file_round_trips_byte_exact, a_rename_policy_lands_the_clash_beside_the_users_file,
    a_same_server_copy_duplicates_a_tree, a_same_server_move_merges_without_a_folder_prompt,
    a_same_server_move_without_a_clash_moves_the_folder_whole, many_files_at_full_concurrency_land_intact,
    overwrite_smaller_replaces_only_the_smaller_destination,
};
use super::webdav_test_support::fixture;

// ── Merges and policies ──────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_deep_clash_merge_under_skip_keeps_every_dest_only_file() {
    let (remote, dir) = fixture().await;
    a_deep_clash_merge_under_skip_keeps_every_dest_only_file(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_deep_clash_merge_under_overwrite_replaces_only_the_clash() {
    let (remote, dir) = fixture().await;
    a_deep_clash_merge_under_overwrite_replaces_only_the_clash(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_rename_policy_lands_the_clash_beside_the_users_file() {
    let (remote, dir) = fixture().await;
    a_rename_policy_lands_the_clash_beside_the_users_file(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_overwrite_smaller_replaces_only_the_smaller_destination() {
    let (remote, dir) = fixture().await;
    overwrite_smaller_replaces_only_the_smaller_destination(remote, dir).await;
}

// ── Moves across the volume boundary ─────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_move_merge_onto_the_server_spares_what_it_skipped() {
    let (remote, dir) = fixture().await;
    a_move_merge_onto_the_server_spares_what_it_skipped(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_folder_moved_onto_the_server_leaves_no_source() {
    let (remote, dir) = fixture().await;
    a_folder_moved_onto_the_server_leaves_no_source(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_folder_moved_off_the_server_leaves_no_source() {
    let (remote, dir) = fixture().await;
    a_folder_moved_off_the_server_leaves_no_source(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_file_saved_over_mid_move_off_the_server_stays() {
    let (remote, dir) = fixture().await;
    a_file_saved_over_mid_move_off_the_server_stays(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_file_added_mid_move_off_the_server_stays() {
    let (remote, dir) = fixture().await;
    a_file_added_mid_move_off_the_server_stays(remote, dir).await;
}

// ── Staying on the server ────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_same_server_move_merges_without_a_folder_prompt() {
    let (remote, dir) = fixture().await;
    a_same_server_move_merges_without_a_folder_prompt(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_same_server_move_without_a_clash_moves_the_folder_whole() {
    let (remote, dir) = fixture().await;
    a_same_server_move_without_a_clash_moves_the_folder_whole(remote, dir).await;
}

/// The server copies for itself (`COPY`), so no byte crosses the link.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_same_server_copy_duplicates_a_tree() {
    let (remote, dir) = fixture().await;
    a_same_server_copy_duplicates_a_tree(remote, dir).await;
}

// ── Destinations, sizes, and load ────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_copy_into_a_missing_nested_destination_makes_every_level() {
    let (remote, dir) = fixture().await;
    a_copy_into_a_missing_nested_destination_makes_every_level(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_multi_megabyte_file_round_trips_byte_exact() {
    let (remote, dir) = fixture().await;
    a_multi_megabyte_file_round_trips_byte_exact(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_many_files_at_full_concurrency_land_intact() {
    let (remote, dir) = fixture().await;
    many_files_at_full_concurrency_land_intact(remote, dir).await;
}
