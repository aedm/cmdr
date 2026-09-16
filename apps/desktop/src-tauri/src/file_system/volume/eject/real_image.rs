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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cmdr_fs::testing::disk_images::{DiskImage, DiskImageSession, FileHolder, HarnessError, ImageSpec};

use super::disk_flight::DiskTeardown;
use super::disk_flight::test_support::FakeIndex;
use super::disk_flight::{FlightResume, Sibling, capture, captured_paths, hand_back, owed_ids};
use super::disk_target::{self, DiskMounts, Resolution};
use super::unmount_tool::{self, Target, ToolOutcome, UnmountVerb};
use super::{EjectError, INDEX_STOP_DEADLINE};
use crate::file_system::volume::drive_release::{DriveRelease, IndexDoor};
use crate::volumes::disk_units::MountedVolume;

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

// ── The whole physical disk (M12) ─────────────────────────────────

/// The physical disk under `mount_point`, as `eject_now` resolves it.
fn disk_under(mount_point: &Path) -> disk_target::DiskTarget {
    match disk_target::resolve(&mount_point.to_string_lossy()) {
        Resolution::Disk(target) => target,
        other => panic!("an attached image's volume sits on a physical disk, got {other:?}"),
    }
}

fn sibling(volume_id: &str, path: &Path) -> Sibling {
    Sibling {
        volume_id: volume_id.to_string(),
        path: path.to_path_buf(),
    }
}

/// The teardown the flight hands `run_teardown` for `target`: every mount of the disk
/// captured, plus a fresh read for the ones it never named.
fn disk_teardown(target: &disk_target::DiskTarget, siblings: &[Sibling]) -> DiskTeardown {
    let units = target.units.clone();
    DiskTeardown::new(captured_paths(&mounts_on(&target.units), siblings), move || {
        !mounts_on(&units).is_empty()
    })
}

/// What's mounted on the disk, for a lane where DiskArbitration is answering (the
/// production path fails closed on an unreadable answer instead).
fn mounts_on(units: &[u32]) -> Vec<MountedVolume> {
    match disk_target::mounted_volumes_on_disk(units) {
        DiskMounts::Read(mounted) => mounted,
        DiskMounts::Unreadable => panic!("DiskArbitration answers for an attached image's disk"),
    }
}

/// One `diskutil eject` at `aimed_at` through the harness runner.
fn guarded_eject_at(image: &DiskImage, aimed_at: PathBuf) -> ToolOutcome {
    match image.eject(&aimed_at) {
        Ok(_) => ToolOutcome::Succeeded,
        Err(HarnessError::Failed { code, stderr, .. }) => ToolOutcome::Exited { code, stderr },
        Err(HarnessError::TimedOut { .. }) => ToolOutcome::TimedOut,
        Err(other) => ToolOutcome::CouldNotStart {
            detail: other.to_string(),
        },
    }
}

/// Tears the whole disk down the way the flight does, with the guarded tool: each
/// attempt aimed at a mount of the disk that's still listed, and success needing every
/// captured mount gone AND a fresh read finding nothing else on it.
async fn eject_the_disk_through_the_production_retries(
    image: &DiskImage,
    mount_point: &Path,
    teardown: &DiskTeardown,
) -> Result<(), EjectError> {
    let mount_path = mount_point.to_string_lossy().to_string();
    unmount_tool::settle_with_retries(
        Target {
            volume_id: "real-image-disk-pin",
            verb: UnmountVerb::Eject,
            mount_path: &mount_path,
        },
        || {
            let aimed_at = teardown.aim(&mount_path);
            async move { guarded_eject_at(image, PathBuf::from(aimed_at)) }
        },
        || teardown.is_still_mounted(),
    )
    .await
}

/// Ejects volume A of a two-volume `spec` while a file on its sibling B is held open.
///
/// ❗ **This is M12's flip.** `diskutil eject` unmounts A, then can't take the rest of
/// the disk down while B is held. Asking only about A read that partial unmount as a
/// clean eject, leaving the drive powered on with B still mounted; now every mount of
/// the disk has to be gone, so it's an honest refusal.
async fn ejecting_a_while_b_is_held_is_refused_and_leaves_the_disk_up(spec: ImageSpec) {
    let session = DiskImageSession::acquire();
    let image = DiskImage::attach(&session, spec).expect("attach the image");
    let (a, b) = (
        image.volumes()[0].mount_point.clone(),
        image.volumes()[1].mount_point.clone(),
    );
    let held = b.join("held.txt");
    std::fs::write(&held, b"held").expect("write the held file");
    let holder = FileHolder::hold(&held).expect("hold the file open");
    let siblings = [sibling("vol-a", &a), sibling("vol-b", &b)];
    let teardown = disk_teardown(&disk_under(&a), &siblings);

    let result = eject_the_disk_through_the_production_retries(&image, &a, &teardown).await;

    assert!(
        matches!(result, Err(EjectError::UnmountRefused { .. })),
        "a disk whose sibling is held is refused, not read as half-ejected, got {result:?}"
    );
    assert!(is_mounted(&b), "B, held, is still mounted");
    assert!(image.is_attached().expect("hdiutil info"), "the disk is still attached");
    drop(holder);
}

/// M12's flip on two APFS volumes in one container.
#[tokio::test]
#[ignore = "attaches a real APFS disk image via hdiutil; run with --run-ignored"]
async fn ejecting_one_apfs_volume_while_its_sibling_is_held_is_refused_and_leaves_the_disk_attached() {
    ejecting_a_while_b_is_held_is_refused_and_leaves_the_disk_up(ImageSpec::ApfsTwoVolumes).await;
}

/// M12's flip on two HFS+ partitions of one GPT disk.
#[tokio::test]
#[ignore = "attaches a real HFS+ disk image via hdiutil; run with --run-ignored"]
async fn ejecting_one_hfs_partition_while_its_sibling_is_held_is_refused_and_leaves_the_disk_attached() {
    ejecting_a_while_b_is_held_is_refused_and_leaves_the_disk_up(ImageSpec::HfsTwoPartitions).await;
}

/// Both volumes of an APFS container key ONE physical disk, and the flight sees both.
///
/// The container is a synthesized whole disk of its own, so keying by DiskArbitration's
/// whole disk alone would stop at the container and miss the hardware carrying it.
#[tokio::test]
#[ignore = "attaches a real APFS disk image via hdiutil; run with --run-ignored"]
async fn two_volumes_of_one_container_key_the_same_physical_disk_and_name_each_other() {
    let session = DiskImageSession::acquire();
    let image = DiskImage::attach(&session, ImageSpec::ApfsTwoVolumes).expect("attach the image");
    let (a, b) = (
        image.volumes()[0].mount_point.clone(),
        image.volumes()[1].mount_point.clone(),
    );

    let (from_a, from_b) = (disk_under(&a), disk_under(&b));

    assert_eq!(from_a.key, from_b.key, "one disk, one flight");
    assert!(
        from_a.units.len() >= 2,
        "the physical disk and its synthesized container are both units, got {:?}",
        from_a.units
    );
    let mounted = mounts_on(&from_a.units);
    let paths: Vec<PathBuf> = mounted.iter().map(|volume| volume.path.clone()).collect();
    assert!(paths.contains(&a) && paths.contains(&b), "got {paths:?}");

    // And the capture turns those mounts into the volumes the flight stops, whichever
    // one the person clicked.
    let registered = |path: &Path| {
        if path == a {
            Some("vol-a".to_string())
        } else if path == b {
            Some("vol-b".to_string())
        } else {
            None
        }
    };
    let siblings = capture("vol-b", &b, &mounted, registered);
    assert_eq!(
        siblings.iter().map(|s| s.volume_id.as_str()).collect::<Vec<_>>(),
        ["vol-b", "vol-a"],
        "the clicked volume leads and its sibling comes with it"
    );
}

/// An idle two-volume disk goes down whole, and only then does the eject answer `Ok`.
#[tokio::test]
#[ignore = "attaches a real APFS disk image via hdiutil; run with --run-ignored"]
async fn an_idle_two_volume_disk_ejects_whole_and_its_image_detaches() {
    let session = DiskImageSession::acquire();
    let image = DiskImage::attach(&session, ImageSpec::ApfsTwoVolumes).expect("attach the image");
    let (a, b) = (
        image.volumes()[0].mount_point.clone(),
        image.volumes()[1].mount_point.clone(),
    );
    let siblings = [sibling("vol-a", &a), sibling("vol-b", &b)];
    let teardown = disk_teardown(&disk_under(&a), &siblings);

    let result = eject_the_disk_through_the_production_retries(&image, &a, &teardown).await;

    assert!(result.is_ok(), "got {result:?}");
    assert!(!is_mounted(&a) && !is_mounted(&b), "both volumes left the mount table");
    assert!(
        !image.is_attached().expect("hdiutil info"),
        "the whole image detached, not just the volume that was clicked"
    );
}

/// A refused disk eject hands back the index of the sibling that's STILL mounted, and
/// leaves the one that really went alone.
///
/// The partial unmount is the point: A is gone, so starting its index again would walk a
/// drive that isn't there; B stayed, so it's owed the index the pre-stop took.
#[tokio::test]
#[ignore = "attaches a real APFS disk image via hdiutil; run with --run-ignored"]
async fn a_refused_disk_eject_hands_back_only_the_sibling_that_stayed_mounted() {
    let session = DiskImageSession::acquire();
    let image = DiskImage::attach(&session, ImageSpec::ApfsTwoVolumes).expect("attach the image");
    let (a, b) = (
        image.volumes()[0].mount_point.clone(),
        image.volumes()[1].mount_point.clone(),
    );
    let held = b.join("held.txt");
    std::fs::write(&held, b"held").expect("write the held file");
    let holder = FileHolder::hold(&held).expect("hold the file open");

    let index = Arc::new(FakeIndex::default());
    index.indexes("vol-a");
    index.indexes("vol-b");
    let gate = DriveRelease::with_door(Arc::clone(&index) as Arc<dyn IndexDoor>);
    let siblings = [sibling("vol-a", &a), sibling("vol-b", &b)];
    let owner = Arc::new(FlightResume {
        flight_id: u64::MAX,
        paths: siblings
            .iter()
            .map(|sibling| (sibling.volume_id.clone(), sibling.path.clone()))
            .collect(),
    });

    // The pre-stop, then the teardown the held sibling refuses.
    let ids: Vec<String> = siblings.iter().map(|sibling| sibling.volume_id.clone()).collect();
    let release = gate.release(
        &ids,
        std::time::Instant::now() + INDEX_STOP_DEADLINE,
        index.stop(),
        |_| {},
    );
    let teardown = disk_teardown(&disk_under(&a), &siblings);
    let result = eject_the_disk_through_the_production_retries(&image, &a, &teardown).await;
    assert!(
        matches!(result, Err(EjectError::UnmountRefused { .. })),
        "got {result:?}"
    );
    assert!(!is_mounted(&a), "A really went, so it's owed nothing back");

    hand_back(&gate, &owner, owed_ids(&release), |id| gate.epoch(id));
    index.wait_for_a_resume_to_settle();

    assert_eq!(
        index.started(),
        ["vol-b"],
        "only the volume still in the mount table gets its index back"
    );
    drop(holder);
}
