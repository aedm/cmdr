//! The harness against real images: each spec attaches, mounts every volume
//! `-nobrowse`, and leaves nothing behind when it drops.
//!
//! `#[ignore]`d because they attach real disk images; hand-run with
//! `cargo nextest run -p cmdr-fs --run-ignored only -E 'test(testing::disk_images::real_images::)'`.
//! Serialized in the `disk-image` nextest group, and machine-wide by the session lock.

use super::*;
use crate::testing::wait_until;
use std::time::Duration;

/// Whether the mount at `mount_point` carries `MNT_DONTBROWSE`, read from the mount
/// table's own flags.
fn is_mounted_nobrowse(mount_point: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;

    let c_path = std::ffi::CString::new(mount_point.as_os_str().as_bytes()).expect("no NUL in a mount point");
    // SAFETY: `statfs` is a plain C struct for which all-zero bytes are valid; the
    // call overwrites it.
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c_path` is NUL-terminated and outlives the call; `stat` is a live out-pointer.
    let answered = unsafe { libc::statfs(c_path.as_ptr(), &mut stat) } == 0;
    answered && stat.f_flags & libc::MNT_DONTBROWSE as u32 != 0
}

/// Attaches `spec`, checks every volume is a writable nobrowse mount, drops the
/// image, and checks it's detached, unmounted, and deleted. Returns the facts of
/// each volume, read while attached.
fn attach_check_and_drop(spec: ImageSpec, volume_count: usize) -> Vec<DiskFacts> {
    let session = DiskImageSession::acquire();
    let image = DiskImage::attach(&session, spec).expect("attach the image");
    let image_path = image.image_path().to_path_buf();
    assert_eq!(image.volumes().len(), volume_count, "volumes: {:?}", image.volumes());

    let mut facts = Vec::new();
    for volume in image.volumes() {
        assert!(
            volume.mount_point.is_dir(),
            "{} is mounted",
            volume.mount_point.display()
        );
        assert!(
            is_mounted_nobrowse(&volume.mount_point),
            "{} is mounted nobrowse",
            volume.mount_point.display()
        );
        std::fs::write(volume.mount_point.join("probe.txt"), b"x").expect("the volume is writable");
        facts.push(session.disk_facts(&volume.node).expect("diskutil knows the volume"));
    }
    let mount_points: Vec<PathBuf> = image.volumes().iter().map(|v| v.mount_point.clone()).collect();

    drop(image);

    let still_attached = session
        .attached_images()
        .expect("hdiutil info")
        .images
        .iter()
        .any(|attached| attached.image_path == image_path);
    assert!(!still_attached, "the image detaches on drop");
    for mount_point in mount_points {
        let description = format!("{} to leave /Volumes", mount_point.display());
        wait_until(Duration::from_secs(5), &description, || !mount_point.exists());
    }
    assert!(!image_path.exists(), "the backing file is deleted");
    facts
}

#[test]
#[ignore = "attaches a real APFS disk image via hdiutil; run with --run-ignored"]
fn an_apfs_image_mounts_one_nobrowse_volume_and_leaves_nothing_behind() {
    attach_check_and_drop(ImageSpec::Apfs, 1);
}

#[test]
#[ignore = "attaches a real APFS disk image via hdiutil; run with --run-ignored"]
fn a_two_volume_apfs_image_mounts_both_volumes_of_one_container() {
    let facts = attach_check_and_drop(ImageSpec::ApfsTwoVolumes, 2);
    assert_ne!(facts[0].volume_name, facts[1].volume_name);
    assert!(facts[0].apfs_container_reference.is_some());
    assert_eq!(facts[0].apfs_container_reference, facts[1].apfs_container_reference);
}

#[test]
#[ignore = "attaches a real HFS+ disk image via hdiutil; run with --run-ignored"]
fn an_hfs_image_mounts_one_nobrowse_volume_and_leaves_nothing_behind() {
    attach_check_and_drop(ImageSpec::Hfs, 1);
}

#[test]
#[ignore = "attaches a real HFS+ disk image via hdiutil; run with --run-ignored"]
fn a_two_partition_hfs_image_mounts_both_partitions_of_one_disk() {
    let facts = attach_check_and_drop(ImageSpec::HfsTwoPartitions, 2);
    assert_ne!(facts[0].volume_name, facts[1].volume_name);
    assert!(facts[0].apfs_container_reference.is_none());
    assert_eq!(facts[0].parent_whole_disk, facts[1].parent_whole_disk);
}

#[test]
#[ignore = "attaches a real APFS disk image via hdiutil; run with --run-ignored"]
fn an_eject_aimed_at_a_volume_that_isnt_this_images_is_refused_before_diskutil_runs() {
    let session = DiskImageSession::acquire();
    let image = DiskImage::attach(&session, ImageSpec::Apfs).expect("attach the image");
    // The Mac's own Data volume: `diskutil info` answers for it, and `hdiutil` lists
    // it under no image of ours, so the eject never runs.
    let result = image.eject(Path::new("/System/Volumes/Data"));
    assert!(
        matches!(
            result.as_ref().map_err(HarnessError::refusal),
            Err(Some(Refusal::NotOurs { .. }))
        ),
        "got {result:?}"
    );
}
