//! Real copies off a live S3 bucket onto local disk, driven through the app's
//! own `copy_between_volumes`, against both Docker fixtures.
//!
//! ❗ **The point is the entry point**, the same as
//! `webdav_transfer_integration_test.rs`: `cmdr-s3`'s own suite exercises every
//! read method, but a copy also needs the capability predicates, the scan, and
//! the pre-flight to agree, and none of those live in the crate. S3 has no
//! writes yet, so the source is seeded through the crate's own request
//! builders (`cmdr_s3::volume::testing::seed`), and only the off-the-bucket
//! direction runs.
//!
//! Every cell checksums the bytes at BOTH ends. The cells stay named for the
//! `s3_integration_` lane prefix.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cmdr_fs::volume::Volume;
use cmdr_s3::volume::testing::{
    FIXTURE_BUCKET, FixtureService, GARAGE, VERSITYGW, connect_fixture, object, scratch_prefix, seed, seed_once,
};

use super::network_transfer_test_support::{
    a_seeded_tree_lands_intact_off_the_server, read_all, run_copy, self_describing_bytes, sha256, tree_files,
};
use crate::file_system::volume::LocalPosixVolume;
use crate::test_support::TestDir;

/// Big enough to cross the read windows rather than ride in one chunk.
const PAYLOAD_BYTES: usize = 700_000;

/// Past 64 MiB, the size where holding a file whole would show. Shared with the
/// crate's own large-object cell, and seeded once per fixture (`seed_once`).
const LARGE_LEN: usize = 65 * 1024 * 1024 + 123;
const LARGE_KEY: &str = "cmdr-test-large-65mib/blob.bin";

/// A bucket place on `service`, as the transfer engine sees it, plus the key
/// prefix and app path of a scratch folder nothing else in the run uses.
async fn fixture(service: FixtureService, label: &str) -> (Arc<dyn Volume>, String, PathBuf) {
    let volume = connect_fixture(service, Some(FIXTURE_BUCKET)).await;
    let prefix = scratch_prefix(label);
    let dir = volume.root().join(prefix.trim_end_matches('/'));
    (Arc::new(volume), prefix, dir)
}

/// Copies `source` off the bucket into a fresh local directory and insists the
/// bytes that landed checksum to what the bucket holds.
async fn copy_off_and_compare(label: &str, remote: Arc<dyn Volume>, source: PathBuf, expected_len: usize) {
    let source_digest = sha256(&read_all(remote.as_ref(), &source).await);
    let name = source.file_name().expect("a file name").to_string_lossy().into_owned();

    let local_dir = TestDir::new(label);
    let local: Arc<dyn Volume> = Arc::new(LocalPosixVolume::new("Local", &*local_dir));
    run_copy(
        label,
        Arc::clone(&remote),
        vec![source],
        Arc::clone(&local),
        PathBuf::from(""),
    )
    .await;

    let landed = read_all(local.as_ref(), Path::new(&name)).await;
    assert_eq!(
        landed.len(),
        expected_len,
        "{label}: the copy landed the wrong number of bytes"
    );
    assert_eq!(
        sha256(&landed),
        source_digest,
        "{label}: the bytes on local disk must checksum to what the bucket holds"
    );
}

async fn copying_off_a_bucket_lands_every_byte(service: FixtureService) {
    let (remote, prefix, dir) = fixture(service, "copy-off").await;
    let content = self_describing_bytes(PAYLOAD_BYTES, "downloaded.bin");
    seed(
        service,
        FIXTURE_BUCKET,
        &[object(&format!("{prefix}downloaded.bin"), &content)],
    )
    .await;
    assert_eq!(
        sha256(&read_all(remote.as_ref(), &dir.join("downloaded.bin")).await),
        sha256(&content),
        "the fixture seed must round-trip"
    );

    copy_off_and_compare("s3_copy_off_bucket", remote, dir.join("downloaded.bin"), PAYLOAD_BYTES).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn s3_integration_copying_off_a_bucket_lands_every_byte_on_versitygw() {
    copying_off_a_bucket_lands_every_byte(VERSITYGW).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn s3_integration_copying_off_a_bucket_lands_every_byte_on_garage() {
    copying_off_a_bucket_lands_every_byte(GARAGE).await;
}

async fn a_large_object_copies_off_a_bucket_byte_for_byte(service: FixtureService) {
    seed_once(service, FIXTURE_BUCKET, LARGE_KEY, LARGE_LEN, || {
        cmdr_s3::volume::testing::self_describing_bytes(LARGE_LEN, "large")
    })
    .await;
    let remote: Arc<dyn Volume> = Arc::new(connect_fixture(service, Some(FIXTURE_BUCKET)).await);
    let source = remote.root().join(LARGE_KEY);
    copy_off_and_compare("s3_copy_large_off_bucket", remote, source, LARGE_LEN).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn s3_integration_a_large_object_copies_off_a_bucket_byte_for_byte_on_versitygw() {
    a_large_object_copies_off_a_bucket_byte_for_byte(VERSITYGW).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn s3_integration_a_large_object_copies_off_a_bucket_byte_for_byte_on_garage() {
    a_large_object_copies_off_a_bucket_byte_for_byte(GARAGE).await;
}

async fn a_directory_tree_lands_intact_off_a_bucket(service: FixtureService) {
    let (remote, prefix, dir) = fixture(service, "tree-off").await;
    let files: Vec<(String, Vec<u8>)> = tree_files()
        .map(|(relative, bytes)| (format!("{prefix}tree/{relative}"), bytes))
        .collect();
    let seeds: Vec<_> = files.iter().map(|(key, bytes)| object(key, bytes)).collect();
    seed(service, FIXTURE_BUCKET, &seeds).await;

    a_seeded_tree_lands_intact_off_the_server(remote, &dir).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn s3_integration_a_directory_tree_lands_intact_off_a_bucket_on_versitygw() {
    a_directory_tree_lands_intact_off_a_bucket(VERSITYGW).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn s3_integration_a_directory_tree_lands_intact_off_a_bucket_on_garage() {
    a_directory_tree_lands_intact_off_a_bucket(GARAGE).await;
}
