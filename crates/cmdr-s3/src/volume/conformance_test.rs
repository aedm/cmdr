//! The shared `Volume` conformance promises a READ-ONLY place can be held to,
//! against both fixtures.
//!
//! The write-side promises (no recursive delete, no clobbering rename or
//! create, honest `create_directory_all`) arrive with the write milestone. What
//! a read-only volume owes is that it SAYS so: `is_writable` and
//! `supports_export` match what the methods do, and `NotFound` names the path.

use cmdr_fs::volume::Volume;
use cmdr_fs::volume::conformance;

use super::testing::*;

async fn a_read_only_place_keeps_the_promises_it_can(service: FixtureService) {
    let volume = connect_fixture(service, Some(FIXTURE_BUCKET)).await;
    let prefix = scratch_prefix("conformance");
    let file = format!("{prefix}held.txt");
    let content = b"the bytes the export assertion compares against";
    seed(service, FIXTURE_BUCKET, &[object(&file, content)]).await;

    conformance::assert_writability_matches_the_mutations_offered(&volume, &volume.root().join(format!("{prefix}new")))
        .await;
    conformance::assert_export_matches_the_bytes_offered(&volume, &volume.root().join(&file), content).await;
    conformance::assert_not_found_carries_the_path(&volume, &volume.root().join(format!("{prefix}missing.txt"))).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn a_read_only_place_keeps_the_promises_it_can_on_versitygw() {
    a_read_only_place_keeps_the_promises_it_can(VERSITYGW).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn a_read_only_place_keeps_the_promises_it_can_on_garage() {
    a_read_only_place_keeps_the_promises_it_can(GARAGE).await;
}
