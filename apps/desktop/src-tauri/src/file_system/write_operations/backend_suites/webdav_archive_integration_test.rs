//! Archives on a live WebDAV server: browse and extract over ranged reads, the
//! write-routing predicate, a remote edit and its cancel, a copy into a remote
//! zip, and a compress that lands a new one (or replaces a look-alike in place).
//!
//! The browse cell runs twice: once on the stock server, which answers every
//! ranged GET with a 206 window, and once on `webdav-fixture-norange`, which
//! answers it 200 with the whole file. A zip reader asks for many small windows
//! (the central directory, then each entry), so that second server is what puts
//! `streams.rs`'s skip-locally branch under an app workload rather than a
//! single read.
//!
//! The scenarios are backend-blind and live in `network_archive_test_support.rs`;
//! the cells stay here because the lane selects them by the `webdav_integration_`
//! prefix. Twins: `sftp_archive_integration_test.rs`,
//! `smb_archive_integration_test.rs`.

use super::network_archive_test_support::{
    a_cancel_before_the_swap_keeps_the_original_zip, a_compress_onto_the_server_lands_a_valid_zip,
    a_compress_replaces_a_look_alike_archive_in_place, a_remote_zip_edit_deletes_an_entry_on_the_server,
    a_zip_inner_path_on_the_server_routes_as_inside_the_archive, a_zip_on_the_server_browses_and_extracts,
    local_files_copied_into_a_zip_on_the_server_join_it,
};
use super::webdav_test_support::{WebdavFixture, fixture, fixture_on};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_zip_on_the_server_browses_and_extracts() {
    let (remote, dir) = fixture().await;
    a_zip_on_the_server_browses_and_extracts(remote, dir).await;
}

/// On the server that ignores `Range`, where every window arrives inside a
/// whole-file answer and has to be cut out locally.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_zip_on_a_server_that_ignores_range_browses_and_extracts() {
    let (remote, dir) = fixture_on(WebdavFixture::IgnoresRange).await;
    a_zip_on_the_server_browses_and_extracts(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_zip_inner_path_on_the_server_routes_as_inside_the_archive() {
    let (remote, dir) = fixture().await;
    a_zip_inner_path_on_the_server_routes_as_inside_the_archive(remote, dir).await;
}

/// The swap is `MOVE` with `Overwrite: T` from the staging sibling onto the zip.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_remote_zip_edit_deletes_an_entry_on_the_server() {
    let (remote, dir) = fixture().await;
    a_remote_zip_edit_deletes_an_entry_on_the_server(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_cancel_before_the_swap_keeps_the_original_zip() {
    let (remote, dir) = fixture().await;
    a_cancel_before_the_swap_keeps_the_original_zip(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_local_files_copied_into_a_zip_on_the_server_join_it() {
    let (remote, dir) = fixture().await;
    local_files_copied_into_a_zip_on_the_server_join_it(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_compress_onto_the_server_lands_a_valid_zip() {
    let (remote, dir) = fixture().await;
    a_compress_onto_the_server_lands_a_valid_zip(remote, dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn webdav_integration_a_compress_replaces_a_look_alike_archive_in_place() {
    let (remote, dir) = fixture().await;
    a_compress_replaces_a_look_alike_archive_in_place(remote, dir).await;
}
