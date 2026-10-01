//! Renames that copy, and copies inside one account, against both Docker S3
//! fixtures, through the app's own entry points.
//!
//! On S3 a folder rename is one copy and one delete per object, and a big
//! file's is a multipart copy (`Volume::rename_work`), so a rename runs as a
//! move through the transfer engine (`routing::start_rename_by_move`, the
//! route F2, the MCP rename, and a bulk rename all take). These cells drive
//! that route end to end against live servers: the batch delete past a
//! thousand keys, the multipart copy and the date it keeps, pause and cancel
//! leaving every source whole, and a copy between two buckets of one account
//! running on the server unless the provider copies within a bucket only.
//!
//! The cells stay named for the `s3_integration_` lane prefix.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cmdr_fs::volume::{Volume, VolumeError};
use cmdr_s3::S3Volume;
use cmdr_s3::volume::testing::{
    FIXTURE_BUCKET, FIXTURE_BUCKET_2, FixtureService, GARAGE, Seed, VERSITYGW, connect_fixture, distant_mtime, object,
    scratch_prefix, seed, self_describing_bytes, stored_mtime_header, stored_write_token, unfinished_uploads,
};

use super::network_transfer_test_support::{read_all, run_copy, sha256};
use crate::file_system::volume::manager::get_volume_manager;
use crate::file_system::write_operations::event_sinks::CollectorEventSink;
use crate::file_system::write_operations::start_rename_by_move;
use crate::file_system::write_operations::state::{
    cancel_write_operation, pause_write_operation, resume_write_operation,
};
use crate::file_system::write_operations::types::{VolumeCopyConfig, WriteOperationType};
use crate::ignore_poison::IgnorePoison;
use crate::operation_log::types::Initiator;

const MIB: usize = 1024 * 1024;

/// How long a rename-by-move may take to settle: a thousand server-side copies
/// on a fixture several suites share.
const SETTLE_BUDGET: Duration = Duration::from_secs(120);

/// A bucket place on `service`, registered with the volume manager under a
/// fresh id the way a connect registers it, plus a scratch prefix and its
/// folder.
async fn registered(service: FixtureService, label: &str, part_floor: Option<u64>) -> (String, Arc<S3Volume>, String) {
    let volume = connect_fixture(service, Some(FIXTURE_BUCKET)).await;
    if let Some(floor) = part_floor {
        volume.set_part_floor(floor);
    }
    let volume = Arc::new(volume);
    let volume_id = format!("{}-{label}", volume.volume_id());
    get_volume_manager().register(&volume_id, Arc::clone(&volume) as Arc<dyn Volume>);
    (volume_id, volume, scratch_prefix(label))
}

fn at(volume: &S3Volume, key: &str) -> PathBuf {
    volume.root().join(key.trim_end_matches('/'))
}

/// Starts `from` → `new_name` in its own folder through the rename route,
/// returning the sink and the operation id.
async fn start(volume_id: &str, from: PathBuf, new_name: &str) -> (Arc<CollectorEventSink>, String) {
    let events = Arc::new(CollectorEventSink::new());
    let parent = from.parent().expect("a parent").display().to_string();
    let started = start_rename_by_move(
        events.clone(),
        volume_id.to_string(),
        vec![(from, new_name.to_string())],
        parent,
        VolumeCopyConfig::default(),
        Initiator::User,
        None,
    )
    .await
    .expect("the rename starts");
    assert_eq!(started.operation_type, WriteOperationType::Move);
    (events, started.operation_id)
}

async fn settle(events: &CollectorEventSink, what: &str) {
    crate::test_support::wait_until_async(SETTLE_BUDGET, what, || !events.settled.lock_ignore_poison().is_empty())
        .await;
    let errors = events.errors.lock_ignore_poison();
    assert!(
        errors.is_empty(),
        "{what}: {:?}",
        errors.iter().map(|e| &e.error).collect::<Vec<_>>()
    );
}

/// ❗ A folder past a thousand objects renames through the engine: every
/// object arrives under the new name, and the sources go in `DeleteObjects`
/// batches, the second one paging past the first thousand.
async fn a_folder_of_1005_objects_renames_through_the_engine(service: FixtureService) {
    let (volume_id, volume, prefix) = registered(service, "rename-1005", None).await;
    let keys: Vec<String> = (0..1_005).map(|n| format!("{prefix}folder/f{n:04}.txt")).collect();
    let seeds: Vec<Seed<'_>> = keys.iter().map(|key| object(key, b"x")).collect();
    seed(service, FIXTURE_BUCKET, &seeds).await;

    let (events, _) = start(&volume_id, at(&volume, &format!("{prefix}folder")), "renamed").await;
    settle(&events, "the folder rename to settle").await;

    let renamed = volume
        .list_directory(&at(&volume, &format!("{prefix}renamed")), None)
        .await
        .expect("the renamed folder lists");
    assert_eq!(renamed.len(), 1_005, "every object arrived under the new name");
    assert!(
        matches!(
            volume
                .list_directory(&at(&volume, &format!("{prefix}folder")), None)
                .await,
            Err(VolumeError::NotFound(_))
        ),
        "the old folder is gone, all 1,005 of it"
    );
}

/// A file past the part floor renames by multipart copy: the bytes and the
/// source's date arrive, the old key goes, and no upload stays open.
async fn a_big_file_renames_by_multipart_copy_keeping_its_date(service: FixtureService) {
    let (volume_id, volume, prefix) = registered(service, "rename-big", Some(5 * MIB as u64)).await;
    let bytes = self_describing_bytes(17 * MIB, "big");
    let from_key = format!("{prefix}clip.mov");
    seed(
        service,
        FIXTURE_BUCKET,
        &[Seed {
            key: &from_key,
            bytes: &bytes,
            mtime: Some(distant_mtime()),
        }],
    )
    .await;

    let (events, _) = start(&volume_id, at(&volume, &from_key), "renamed.mov").await;
    settle(&events, "the big file's rename to settle").await;

    let to_key = format!("{prefix}renamed.mov");
    assert_eq!(
        sha256(&read_all(volume.as_ref(), &at(&volume, &to_key)).await),
        sha256(&bytes)
    );
    assert_eq!(
        stored_mtime_header(service, FIXTURE_BUCKET, &to_key).await,
        Some(rclone_mtime_of_distant()),
        "the source's date survives the rename"
    );
    assert!(!volume.exists(&at(&volume, &from_key)).await, "the old name is gone");
    assert!(unfinished_uploads(service, FIXTURE_BUCKET, &prefix).await.is_empty());
}

/// The rclone-format mtime of `distant_mtime()`, which is whole seconds.
fn rclone_mtime_of_distant() -> String {
    distant_mtime()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after the epoch")
        .as_secs()
        .to_string()
}

/// ❗ A paused rename copies nothing until resumed, and a cancel while it's
/// parked leaves the source whole, nothing at the new name, and no upload.
async fn a_paused_then_cancelled_rename_keeps_the_source_whole(service: FixtureService) {
    let (volume_id, volume, prefix) = registered(service, "rename-pause-cancel", Some(5 * MIB as u64)).await;
    let bytes = self_describing_bytes(20 * MIB, "kept");
    let from_key = format!("{prefix}kept.bin");
    seed(service, FIXTURE_BUCKET, &[object(&from_key, &bytes)]).await;

    let (events, operation_id) = start(&volume_id, at(&volume, &from_key), "gone.bin").await;
    assert!(pause_write_operation(&operation_id) || !events.settled.lock_ignore_poison().is_empty());
    cancel_write_operation(&operation_id, false);
    crate::test_support::wait_until_async(SETTLE_BUDGET, "the cancelled rename to settle", || {
        !events.settled.lock_ignore_poison().is_empty()
    })
    .await;

    assert_eq!(
        sha256(&read_all(volume.as_ref(), &at(&volume, &from_key)).await),
        sha256(&bytes),
        "the source stays whole"
    );
    assert!(unfinished_uploads(service, FIXTURE_BUCKET, &prefix).await.is_empty());
    if volume.exists(&at(&volume, &format!("{prefix}gone.bin"))).await {
        // A cancel that landed after the copy published is a finished copy
        // with its source kept: a duplicate, never a loss.
        assert_eq!(
            sha256(&read_all(volume.as_ref(), &at(&volume, &format!("{prefix}gone.bin"))).await),
            sha256(&bytes)
        );
    }
}

/// A paused rename resumes and lands.
async fn a_paused_rename_resumes_and_lands(service: FixtureService) {
    let (volume_id, volume, prefix) = registered(service, "rename-pause", Some(5 * MIB as u64)).await;
    let bytes = self_describing_bytes(12 * MIB, "resumed");
    let from_key = format!("{prefix}a.bin");
    seed(service, FIXTURE_BUCKET, &[object(&from_key, &bytes)]).await;

    let (events, operation_id) = start(&volume_id, at(&volume, &from_key), "b.bin").await;
    pause_write_operation(&operation_id);
    resume_write_operation(&operation_id);
    settle(&events, "the resumed rename to settle").await;

    assert_eq!(
        sha256(&read_all(volume.as_ref(), &at(&volume, &format!("{prefix}b.bin"))).await),
        sha256(&bytes)
    );
    assert!(!volume.exists(&at(&volume, &from_key)).await);
}

/// A reviewed batch with a folder in it (Ask Cmdr's proposals and the bulk
/// rename both start here) runs as ONE move with the new names, and lands
/// every one of them.
async fn a_batch_with_a_folder_renames_as_one_move(service: FixtureService) {
    use crate::file_system::write_operations::{BulkRenameRow, SourceFingerprint, start_renames};

    let (volume_id, volume, prefix) = registered(service, "rename-batch", None).await;
    let (folder_file, note) = (format!("{prefix}album/a.jpg"), format!("{prefix}note.txt"));
    seed(
        service,
        FIXTURE_BUCKET,
        &[object(&folder_file, b"jpeg"), object(&note, b"n")],
    )
    .await;
    let mut rows = Vec::new();
    for (id, from, to) in [("1", "album", "photos"), ("2", "note.txt", "memo.txt")] {
        let source = at(&volume, &format!("{prefix}{from}"));
        let fingerprint = SourceFingerprint::capture_remote(volume.as_ref(), &source)
            .await
            .expect("a fingerprint");
        rows.push(BulkRenameRow {
            row_id: id.to_string(),
            destination: source.with_file_name(to),
            source,
            expected_fingerprint: fingerprint,
        });
    }
    let events = Arc::new(CollectorEventSink::new());

    let started = start_renames(events.clone(), volume_id, rows, Initiator::Agent)
        .await
        .expect("the batch starts");
    assert_eq!(
        started.operation_type,
        WriteOperationType::Move,
        "one move, not a rename batch"
    );
    settle(&events, "the batch to settle").await;

    assert!(volume.exists(&at(&volume, &format!("{prefix}photos/a.jpg"))).await);
    assert!(volume.exists(&at(&volume, &format!("{prefix}memo.txt"))).await);
    assert!(!volume.exists(&at(&volume, &folder_file)).await);
    assert!(!volume.exists(&at(&volume, &note)).await);
}

/// A copy between two buckets of one account runs on the server: the object
/// arrives without the token a streamed PUT writes.
async fn a_copy_between_two_buckets_runs_on_the_server(service: FixtureService) {
    let source = connect_fixture(service, Some(FIXTURE_BUCKET)).await;
    let destination = connect_fixture(service, Some(FIXTURE_BUCKET_2)).await;
    let prefix = scratch_prefix("engine-cross-bucket");
    let bytes = self_describing_bytes(3 * MIB, "cross");
    let key = format!("{prefix}moved.bin");
    seed(
        service,
        FIXTURE_BUCKET,
        &[Seed {
            key: &key,
            bytes: &bytes,
            mtime: Some(distant_mtime()),
        }],
    )
    .await;
    let dest_dir = destination.root().join(prefix.trim_end_matches('/'));
    let source_path = source.root().join(&key);
    let destination: Arc<dyn Volume> = Arc::new(destination);
    run_copy(
        "s3_cross_bucket_server_side",
        Arc::new(source),
        vec![source_path],
        Arc::clone(&destination),
        dest_dir.clone(),
    )
    .await;

    assert_eq!(
        sha256(&read_all(destination.as_ref(), &dest_dir.join("moved.bin")).await),
        sha256(&bytes)
    );
    assert_eq!(
        stored_write_token(service, FIXTURE_BUCKET_2, &key).await,
        None,
        "copied on the server, so no streamed PUT wrote it"
    );
}

/// A provider that copies within one bucket only (Hetzner) streams a
/// cross-bucket copy through the Mac instead, with the same bytes landing.
async fn a_bucket_bound_provider_streams_a_cross_bucket_copy(service: FixtureService) {
    let source = connect_fixture(service, Some(FIXTURE_BUCKET)).await;
    let destination = connect_fixture(service, Some(FIXTURE_BUCKET_2)).await;
    destination.forbid_cross_bucket_copy().await;
    let prefix = scratch_prefix("engine-bucket-bound");
    let bytes = self_describing_bytes(2 * MIB, "streamed");
    let key = format!("{prefix}moved.bin");
    seed(service, FIXTURE_BUCKET, &[object(&key, &bytes)]).await;
    let dest_dir = destination.root().join(prefix.trim_end_matches('/'));
    let source_path = source.root().join(&key);
    let destination: Arc<dyn Volume> = Arc::new(destination);
    run_copy(
        "s3_cross_bucket_streamed",
        Arc::new(source),
        vec![source_path],
        Arc::clone(&destination),
        dest_dir.clone(),
    )
    .await;

    assert_eq!(
        sha256(&read_all(destination.as_ref(), &dest_dir.join("moved.bin")).await),
        sha256(&bytes)
    );
    assert!(
        stored_write_token(service, FIXTURE_BUCKET_2, &key).await.is_some(),
        "streamed through the Mac, so a PUT wrote it"
    );
}

/// One `#[tokio::test]` per fixture for each cell, `#[ignore]`d for the shared
/// fixture lane.
macro_rules! on_both_fixtures {
    ($($cell:ident => $versitygw:ident, $garage:ident;)*) => {$(
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        #[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
        async fn $versitygw() {
            $cell(VERSITYGW).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        #[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
        async fn $garage() {
            $cell(GARAGE).await;
        }
    )*};
}

on_both_fixtures! {
    a_folder_of_1005_objects_renames_through_the_engine
        => s3_integration_a_folder_of_1005_objects_renames_through_the_engine_on_versitygw,
           s3_integration_a_folder_of_1005_objects_renames_through_the_engine_on_garage;
    a_big_file_renames_by_multipart_copy_keeping_its_date
        => s3_integration_a_big_file_renames_by_multipart_copy_keeping_its_date_on_versitygw,
           s3_integration_a_big_file_renames_by_multipart_copy_keeping_its_date_on_garage;
    a_paused_then_cancelled_rename_keeps_the_source_whole
        => s3_integration_a_paused_then_cancelled_rename_keeps_the_source_whole_on_versitygw,
           s3_integration_a_paused_then_cancelled_rename_keeps_the_source_whole_on_garage;
    a_paused_rename_resumes_and_lands
        => s3_integration_a_paused_rename_resumes_and_lands_on_versitygw,
           s3_integration_a_paused_rename_resumes_and_lands_on_garage;
    a_batch_with_a_folder_renames_as_one_move
        => s3_integration_a_batch_with_a_folder_renames_as_one_move_on_versitygw,
           s3_integration_a_batch_with_a_folder_renames_as_one_move_on_garage;
    a_copy_between_two_buckets_runs_on_the_server
        => s3_integration_a_copy_between_two_buckets_runs_on_the_server_on_versitygw,
           s3_integration_a_copy_between_two_buckets_runs_on_the_server_on_garage;
    a_bucket_bound_provider_streams_a_cross_bucket_copy
        => s3_integration_a_bucket_bound_provider_streams_a_cross_bucket_copy_on_versitygw,
           s3_integration_a_bucket_bound_provider_streams_a_cross_bucket_copy_on_garage;
}
