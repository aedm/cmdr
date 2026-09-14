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

/// Every mount in the kernel's table with its filesystem identity (`f_fsid`), read with
/// `getfsstat(MNT_NOWAIT)`, the same non-blocking read the app's presence answer uses.
fn mount_table() -> Vec<(PathBuf, [i32; 2])> {
    // SAFETY: the documented count query: a null buffer and zero size write nothing.
    let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    assert!(count > 0, "getfsstat lists at least `/`");
    let capacity = count as usize + 4;
    let mut table: Vec<libc::statfs> = Vec::with_capacity(capacity);
    let bytes = (capacity * size_of::<libc::statfs>()) as libc::c_int;
    // SAFETY: `table` has room for `capacity` records and `bytes` is exactly their length.
    let filled = unsafe { libc::getfsstat(table.as_mut_ptr(), bytes, libc::MNT_NOWAIT) };
    assert!(filled > 0, "getfsstat fills the table");
    // SAFETY: the kernel initialized `filled` records, clamped to the capacity.
    unsafe { table.set_len((filled as usize).min(capacity)) };
    table
        .iter()
        .map(|stat| {
            // SAFETY: `f_mntonname` is NUL-terminated inside its fixed array, and `stat`
            // outlives the borrow.
            let on = unsafe { std::ffi::CStr::from_ptr(stat.f_mntonname.as_ptr()) };
            // SAFETY: libc declares `fsid_t` `#[repr(C)]` around exactly one `[i32; 2]`;
            // it only keeps the field private.
            let fsid: [i32; 2] = unsafe { std::mem::transmute(stat.f_fsid) };
            (PathBuf::from(on.to_string_lossy().into_owned()), fsid)
        })
        .collect()
}

/// Renames the image's volume with a file open for writing on it, and checks the rename
/// moved the mount point live: the old path left the mount table, the same filesystem
/// (same `f_fsid`) is listed at the new one, and the handle opened before the rename
/// still writes the file, now read back under the new path.
fn a_rename_moves_the_mount_point_of_a_filesystem_that_stays_mounted(spec: ImageSpec) {
    use std::io::Write;

    let session = DiskImageSession::acquire();
    let mut image = DiskImage::attach(&session, spec).expect("attach the image");
    let before = image.volumes()[0].mount_point.clone();
    let fsid = mount_table()
        .into_iter()
        .find(|(on, _)| *on == before)
        .map(|(_, fsid)| fsid)
        .expect("the volume is in the mount table");
    let mut open = File::create(before.join("open-across-the-rename.txt")).expect("create a file");
    open.write_all(b"before ").expect("write before the rename");

    let after = image.rename_volume(0).expect("rename the volume").mount_point.clone();

    assert_ne!(after, before, "the mount point follows the new name");
    let table = mount_table();
    assert!(
        !table.iter().any(|(on, _)| *on == before),
        "{} left the mount table",
        before.display()
    );
    assert_eq!(
        table.iter().find(|(on, _)| *on == after).map(|(_, fsid)| *fsid),
        Some(fsid),
        "the same filesystem is listed at {}",
        after.display()
    );
    open.write_all(b"after")
        .expect("the handle opened before the rename still writes");
    open.sync_all().expect("sync the file");
    drop(open);
    assert_eq!(
        std::fs::read(after.join("open-across-the-rename.txt")).expect("read under the new name"),
        b"before after",
        "nothing unmounted under the open handle"
    );
}

#[test]
#[ignore = "attaches a real APFS disk image via hdiutil; run with --run-ignored"]
fn renaming_a_mounted_apfs_volume_moves_its_mount_point_and_keeps_its_filesystem_mounted() {
    a_rename_moves_the_mount_point_of_a_filesystem_that_stays_mounted(ImageSpec::Apfs);
}

#[test]
#[ignore = "attaches a real HFS+ disk image via hdiutil; run with --run-ignored"]
fn renaming_a_mounted_hfs_volume_moves_its_mount_point_and_keeps_its_filesystem_mounted() {
    a_rename_moves_the_mount_point_of_a_filesystem_that_stays_mounted(ImageSpec::Hfs);
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
