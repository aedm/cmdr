//! The date scenarios every backend owes, written once: a copy keeps the
//! source's modification date, onto the server and off it.
//!
//! These run the app's whole pipeline, so they catch what the per-backend
//! conformance cells (`cmdr_fs::volume::conformance`'s two date assertions)
//! can't: a staging rename, a wrapper stream, or an engine path that drops the
//! date between the two ends. The contract:
//! `../transfer/volume/DETAILS.md` § "Copies keep the source's date". Cells stay
//! per backend, named for their lane, like `network_transfer_test_support.rs`'s.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cmdr_fs::volume::Volume;
use cmdr_fs::volume::conformance::{SOURCE_DATE_SECS, assert_write_from_stream_keeps_the_source_date};

use super::network_transfer_test_support::{clean_deep, run_copy, self_describing_bytes};
use crate::file_system::volume::LocalPosixVolume;
use crate::test_support::TestDir;

/// What `path` lists as its modification date, which the scenarios insist on.
async fn listed_date(volume: &dyn Volume, path: &Path, what: &str) -> u64 {
    volume
        .get_metadata(path)
        .await
        .unwrap_or_else(|e| panic!("{what}: {} must be stattable, got {e:?}", path.display()))
        .modified_at
        .unwrap_or_else(|| panic!("{what}: {} lists no modification date at all", path.display()))
}

/// A file copied onto the server keeps the date it had on local disk, through
/// the app's whole pipeline: the staging name, the final rename, and the
/// checkpoint wrapper around the source stream all sit between the two ends.
///
/// `tolerance` is the server's clock granularity, as for
/// `conformance::assert_write_from_stream_keeps_the_source_date` (which pins
/// the destination half alone, in the backend's own crate).
pub(super) async fn a_copy_onto_the_server_keeps_the_source_date(
    remote: Arc<dyn Volume>,
    dir: PathBuf,
    tolerance: Duration,
) {
    let local_dir = TestDir::new("network_dated_onto_server");
    let local_file = local_dir.join("dated.bin");
    std::fs::write(&local_file, self_describing_bytes(4_000, "dated.bin")).expect("seed the local file");
    let source_date = std::time::UNIX_EPOCH + Duration::from_secs(SOURCE_DATE_SECS);
    std::fs::File::options()
        .write(true)
        .open(&local_file)
        .and_then(|file| file.set_modified(source_date))
        .expect("date the local file into the past");
    let local: Arc<dyn Volume> = Arc::new(LocalPosixVolume::new("Local", &*local_dir));

    run_copy(
        "dated-onto-server",
        Arc::clone(&local),
        vec![PathBuf::from("dated.bin")],
        Arc::clone(&remote),
        dir.clone(),
    )
    .await;

    let landed = listed_date(remote.as_ref(), &dir.join("dated.bin"), "after the copy").await;
    let off_by = landed.abs_diff(SOURCE_DATE_SECS);
    assert!(
        off_by <= tolerance.as_secs(),
        "the copy on the server must keep the local file's date ({SOURCE_DATE_SECS}); it lists {landed}, {off_by} s off"
    );

    clean_deep(remote.as_ref(), &dir).await;
}

/// A file copied off the server keeps the date the server lists for it.
///
/// The seed is a dated write through the server's own `write_from_stream`,
/// asserted as it goes, so a server that can't store a date fails at the seed
/// with a sentence saying so, rather than here with a date nobody chose.
pub(super) async fn a_copy_off_the_server_keeps_the_source_date(
    remote: Arc<dyn Volume>,
    dir: PathBuf,
    tolerance: Duration,
) {
    let on_server = dir.join("dated.txt");
    assert_write_from_stream_keeps_the_source_date(remote.as_ref(), &on_server, tolerance).await;
    a_copy_off_the_server_keeps_the_date_it_lists(Arc::clone(&remote), on_server).await;
    clean_deep(remote.as_ref(), &dir).await;
}

/// A file already on the server, listed with a date at least a day old, keeps
/// that date when copied off it.
///
/// For a server that can't store a date (Apache `mod_dav`), whose fixture dates
/// a file by its own means; [`a_copy_off_the_server_keeps_the_source_date`]
/// seeds one through the server for everything else. Leaves `on_server` alone.
pub(super) async fn a_copy_off_the_server_keeps_the_date_it_lists(remote: Arc<dyn Volume>, on_server: PathBuf) {
    let source_date = listed_date(remote.as_ref(), &on_server, "the seed").await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is past 1970")
        .as_secs();
    assert!(
        source_date + 24 * 60 * 60 <= now,
        "fixture precondition: {} must list a date at least a day old, so a copy stamping \"now\" can't pass; it lists {source_date}",
        on_server.display()
    );
    let name = on_server
        .file_name()
        .expect("the seed is a file, so it has a name")
        .to_owned();

    let local_dir = TestDir::new("network_dated_off_server");
    let local: Arc<dyn Volume> = Arc::new(LocalPosixVolume::new("Local", &*local_dir));
    run_copy(
        "dated-off-server",
        Arc::clone(&remote),
        vec![on_server],
        Arc::clone(&local),
        PathBuf::from(""),
    )
    .await;

    // Local disk keeps nanoseconds, so only the server's own rounding of the
    // date it reports can put the two a second apart.
    let landed = listed_date(local.as_ref(), Path::new(&name), "after the copy").await;
    let off_by = landed.abs_diff(source_date);
    assert!(
        off_by <= 1,
        "the copy on local disk must keep the date the server lists ({source_date}); it lists {landed}, {off_by} s off"
    );
}
