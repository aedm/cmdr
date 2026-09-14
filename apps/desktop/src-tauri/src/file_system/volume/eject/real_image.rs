//! Pins of what today's eject answers on real synthetic disk images (macOS).
//!
//! `#[ignore]`d: each attaches a real APFS or HFS+ image. Hand-run with
//! `cargo nextest run -p cmdr --run-ignored only -E 'test(file_system::volume::eject::real_image::)'`.
//! Serialized in the `disk-image` nextest group, and machine-wide by the harness's
//! session lock.
//!
//! They drive the production retry loop, [`unmount_tool::settle_with_retries`], with
//! the real mount-table read and a guarded `run_tool`: every attempt is a
//! `diskutil eject` through the harness runner (`cmdr_fs::testing::disk_images`),
//! which proves the mount point is this image's before it runs and SIGKILLs a stuck
//! tool.

use std::path::Path;

use cmdr_fs::testing::disk_images::{DiskImage, DiskImageSession, FileHolder, HarnessError, ImageSpec};

use super::EjectError;
use super::unmount_tool::{self, Target, ToolOutcome, UnmountVerb};

/// A `run_tool` for `settle_with_retries`: `diskutil eject <mount_point>` through the
/// harness. A target the harness can't prove is this image's never runs; that reads
/// as a tool that couldn't start, which `settle` counts as done once the path left
/// the mount table, the way production counts `diskutil`'s "Failed to find disk".
fn guarded_eject<'a>(
    image: &'a DiskImage,
    mount_point: &'a Path,
) -> impl FnMut() -> std::future::Ready<ToolOutcome> + 'a {
    move || {
        std::future::ready(match image.eject(mount_point) {
            Ok(_) => ToolOutcome::Succeeded,
            Err(HarnessError::Failed { code, stderr, .. }) => ToolOutcome::Exited { code, stderr },
            Err(HarnessError::TimedOut { .. }) => ToolOutcome::TimedOut,
            Err(other) => ToolOutcome::CouldNotStart {
                detail: other.to_string(),
            },
        })
    }
}

/// Ejects `mount_point` the way `run_teardown` does, with the guarded tool.
async fn eject_through_the_production_retries(image: &DiskImage, mount_point: &Path) -> Result<(), EjectError> {
    let mount_path = mount_point.to_string_lossy().to_string();
    unmount_tool::settle_with_retries(
        Target {
            volume_id: "real-image-pin",
            verb: UnmountVerb::Eject,
            mount_path: &mount_path,
        },
        guarded_eject(image, mount_point),
        || unmount_tool::is_still_mounted(&mount_path),
    )
    .await
}

fn is_mounted(mount_point: &Path) -> bool {
    unmount_tool::is_still_mounted(&mount_point.to_string_lossy())
}

#[tokio::test]
#[ignore = "attaches a real APFS disk image via hdiutil; run with --run-ignored"]
async fn an_idle_volume_ejects_and_its_image_detaches() {
    let session = DiskImageSession::acquire();
    let image = DiskImage::attach(&session, ImageSpec::Apfs).expect("attach the image");
    let mount_point = image.volumes()[0].mount_point.clone();

    let result = eject_through_the_production_retries(&image, &mount_point).await;

    assert!(result.is_ok(), "got {result:?}");
    assert!(!is_mounted(&mount_point), "the volume left the mount table");
    assert!(
        !image.is_attached().expect("hdiutil info"),
        "`diskutil eject` of a disk image's volume detaches the whole image"
    );
}

#[tokio::test]
#[ignore = "attaches a real APFS disk image via hdiutil; run with --run-ignored"]
async fn a_held_file_answers_unmount_refused_and_the_volume_stays_mounted() {
    let session = DiskImageSession::acquire();
    let image = DiskImage::attach(&session, ImageSpec::Apfs).expect("attach the image");
    let mount_point = image.volumes()[0].mount_point.clone();
    let held = mount_point.join("held.txt");
    std::fs::write(&held, b"held").expect("write the held file");
    let holder = FileHolder::hold(&held).expect("hold the file open");

    let result = eject_through_the_production_retries(&image, &mount_point).await;

    assert!(
        matches!(result, Err(EjectError::UnmountRefused { .. })),
        "got {result:?}"
    );
    assert!(is_mounted(&mount_point), "a refused eject leaves the volume mounted");
    drop(holder);
}

/// Ejects volume A of a two-volume `spec` while a file on its sibling B is held open,
/// and pins what today answers.
///
/// ❗ **Today's gap, which M12 flips.** `diskutil eject` unmounts A, then can't take
/// down the rest of the disk while B is held. `settle` asks only whether A is still
/// listed, so the eject reads as done while B stays mounted and the image stays
/// attached.
async fn ejecting_a_while_b_is_held_answers_ok_and_leaves_b_mounted(spec: ImageSpec) {
    let session = DiskImageSession::acquire();
    let image = DiskImage::attach(&session, spec).expect("attach the image");
    let (a, b) = (
        image.volumes()[0].mount_point.clone(),
        image.volumes()[1].mount_point.clone(),
    );
    let held = b.join("held.txt");
    std::fs::write(&held, b"held").expect("write the held file");
    let holder = FileHolder::hold(&held).expect("hold the file open");

    let result = eject_through_the_production_retries(&image, &a).await;

    assert!(result.is_ok(), "got {result:?}");
    assert!(!is_mounted(&a), "A was unmounted");
    assert!(is_mounted(&b), "B, held, is still mounted");
    assert!(image.is_attached().expect("hdiutil info"), "the disk is still attached");
    drop(holder);
}

/// Today's sibling gap on two APFS volumes in one container.
#[tokio::test]
#[ignore = "attaches a real APFS disk image via hdiutil; run with --run-ignored"]
async fn ejecting_one_apfs_volume_while_its_sibling_is_held_answers_ok_and_leaves_the_sibling_mounted_today() {
    ejecting_a_while_b_is_held_answers_ok_and_leaves_b_mounted(ImageSpec::ApfsTwoVolumes).await;
}

/// Today's sibling gap on two HFS+ partitions of one GPT disk.
#[tokio::test]
#[ignore = "attaches a real HFS+ disk image via hdiutil; run with --run-ignored"]
async fn ejecting_one_hfs_partition_while_its_sibling_is_held_answers_ok_and_leaves_the_sibling_mounted_today() {
    ejecting_a_while_b_is_held_answers_ok_and_leaves_b_mounted(ImageSpec::HfsTwoPartitions).await;
}
