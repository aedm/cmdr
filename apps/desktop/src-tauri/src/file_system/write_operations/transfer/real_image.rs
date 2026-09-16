//! What a transfer does when its drive is REALLY pulled, on a synthetic HFS+
//! image (macOS).
//!
//! `#[ignore]`d: each attaches a real disk image. Hand-run with
//! `cargo nextest run -p cmdr --run-ignored only -E 'test(file_system::write_operations::transfer::real_image::)'`,
//! or through `pnpm check disk-images`. Serialized in the `disk-image` nextest
//! group, and machine-wide by the harness's session lock.
//!
//! The unit tests answer the mount table from a hook (`move_vanished_tests.rs`,
//! `transfer_sides_tests.rs`); these prove the same gates against a real detach,
//! where the kernel picks the errno and the mount table really loses an entry.

use std::sync::Arc;
use std::time::Duration;

use cmdr_fs::testing::disk_images::{DiskImage, DiskImageSession, ImageSpec};

use super::chunked_copy::chunk_park;
use super::copy::copy_files_with_progress_inner;
use super::move_op::move_files_with_progress_inner;
use crate::file_system::write_operations::event_sinks::CollectorEventSink;
use crate::file_system::write_operations::state::{
    WriteOperationState, register_operation_status, unregister_operation_status,
};
use crate::file_system::write_operations::transfer_sides::{TransferSide, TransferSides};
use crate::file_system::write_operations::types::{
    TransferRole, WriteOperationConfig, WriteOperationError, WriteOperationType,
};
use crate::ignore_poison::IgnorePoison;
use crate::test_support::TestDir;

/// Big enough to span several 1 MiB chunks, small enough to write in a moment.
const SOURCE_SIZE: usize = 8 * 1024 * 1024;
/// Where the copy parks: two chunks in, so the file is provably partial.
const PARK_AFTER: u64 = 2 * 1024 * 1024;

/// A Mac-side source folder holding one multi-chunk file.
fn mac_source(name: &str) -> (TestDir, std::path::PathBuf) {
    let dir = TestDir::new(name);
    let file = dir.join("footage.mov");
    std::fs::write(&file, vec![7u8; SOURCE_SIZE]).expect("write the source file");
    (dir, file)
}

fn state_onto(image_root: &std::path::Path, image_name: &str, mac_root: &std::path::Path) -> Arc<WriteOperationState> {
    Arc::new(
        WriteOperationState::new(Duration::from_millis(50)).with_sides(Some(TransferSides::new(
            TransferSide::new("root".to_string(), "Macintosh HD".to_string(), mac_root.to_path_buf()),
            TransferSide::new(
                "vol-image".to_string(),
                image_name.to_string(),
                image_root.to_path_buf(),
            ),
        ))),
    )
}

/// Runs `engine` on its own thread, force-detaches the image once the copy has
/// parked mid-file, and answers what the engine ended with.
fn detach_mid_transfer(
    image: &DiskImage,
    events: Arc<CollectorEventSink>,
    state: Arc<WriteOperationState>,
    engine: impl FnOnce(Arc<CollectorEventSink>, Arc<WriteOperationState>) -> Result<(), WriteOperationError>
    + Send
    + 'static,
) -> Result<(), WriteOperationError> {
    let park = chunk_park::park_after(PARK_AFTER);
    let worker = std::thread::spawn(move || engine(events, state));

    let written = park
        .wait_until_parked(Duration::from_secs(20))
        .expect("the copy parks mid-file");
    assert!(written >= PARK_AFTER, "parked after {written} bytes");

    image.force_detach().expect("the drive is pulled");
    park.release();
    worker.join().expect("the engine thread finishes")
}

/// The whole point of M10: a real detach mid-copy is reported as the drive
/// leaving, named, with how far it got — and the Mac original is untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "attaches a real HFS+ disk image via hdiutil; run with --run-ignored"]
async fn a_copy_onto_a_drive_that_is_pulled_mid_file_names_the_drive_and_keeps_the_original() {
    let session = DiskImageSession::acquire();
    let image = DiskImage::attach(&session, ImageSpec::Hfs).expect("attach an HFS+ image");
    let volume = image.volumes()[0].clone();
    let (mac_dir, source) = mac_source("real-image-copy");

    let events = Arc::new(CollectorEventSink::new());
    let state = state_onto(&volume.mount_point, &volume.name, std::path::Path::new("/"));
    let op_id = "op-real-image-copy";
    register_operation_status(op_id, WriteOperationType::Copy, Vec::new());

    let sources = vec![source.clone()];
    let destination = volume.mount_point.clone();
    let result = detach_mid_transfer(&image, Arc::clone(&events), Arc::clone(&state), move |events, state| {
        copy_files_with_progress_inner(
            &*events,
            op_id,
            &state,
            &sources,
            &destination,
            &WriteOperationConfig::default(),
        )
    });

    match &result {
        Err(WriteOperationError::DeviceDisconnected { side, .. }) => {
            let side = side.as_ref().expect("a real detach names the drive that left");
            assert_eq!(side.role, TransferRole::Destination);
            assert_eq!(side.volume_name, volume.name);
        }
        other => panic!("a pulled drive must read as a disconnect, got {other:?}"),
    }

    let errors = events.errors.lock_ignore_poison();
    let event = errors.first().expect("the copy says why it stopped");
    let progress = event
        .progress_at_stop
        .as_ref()
        .expect("the status cache still had the operation when the event was built");
    assert_eq!(progress.files_total, 1, "one file was asked for");

    assert_eq!(
        std::fs::metadata(&source).expect("the Mac original is untouched").len(),
        SOURCE_SIZE as u64,
        "nothing on the Mac side may be touched by a copy"
    );
    drop(mac_dir);
    unregister_operation_status(op_id);
}

/// The same pull during a move. The originals are the only other copy of the
/// data until Phase 4, and a move that can't finish keeps every one of them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "attaches a real HFS+ disk image via hdiutil; run with --run-ignored"]
async fn a_move_onto_a_drive_that_is_pulled_mid_file_keeps_every_mac_source() {
    let session = DiskImageSession::acquire();
    let image = DiskImage::attach(&session, ImageSpec::Hfs).expect("attach an HFS+ image");
    let volume = image.volumes()[0].clone();
    let (mac_dir, source) = mac_source("real-image-move");

    let events = Arc::new(CollectorEventSink::new());
    let state = state_onto(&volume.mount_point, &volume.name, std::path::Path::new("/"));
    let op_id = "op-real-image-move";
    register_operation_status(op_id, WriteOperationType::Move, Vec::new());

    let sources = vec![source.clone()];
    let destination = volume.mount_point.clone();
    let result = detach_mid_transfer(&image, Arc::clone(&events), Arc::clone(&state), move |events, state| {
        move_files_with_progress_inner(
            &*events,
            op_id,
            &state,
            &sources,
            &destination,
            &WriteOperationConfig::default(),
        )
    });

    assert!(result.is_err(), "a move onto a pulled drive can't succeed");
    assert!(
        source.exists(),
        "the source is the only other copy of the data: a move that didn't finish keeps it"
    );
    assert_eq!(
        std::fs::metadata(&source).expect("the source is readable").len(),
        SOURCE_SIZE as u64,
        "and keeps all of its bytes"
    );
    drop(mac_dir);
    unregister_operation_status(op_id);
}
