//! Pins of the index's presence answer on real synthetic disk images (macOS).
//!
//! `#[ignore]`d: each attaches a real APFS or HFS+ image. Hand-run with
//! `cargo nextest run -p cmdr --run-ignored only -E 'test(file_system::index_provider::real_image::)'`,
//! or through `pnpm check disk-images`. Serialized in the `disk-image` nextest group,
//! and machine-wide by the harness's session lock.
//!
//! The index keys a drive's presence on the filesystem it captured when the drive's
//! index started, ❌ never its root path. These prove the app's answer follows that
//! filesystem through a rename, which moves the mount point while the drive stays
//! mounted, and reads it gone once the image detaches.

use std::time::Duration;

use cmdr_fs::testing::disk_images::{DiskImage, DiskImageSession, ImageSpec};
use cmdr_index::host::volumes::VolumeProvider;

use super::AppVolumeProvider;

fn a_renamed_volume_stays_mounted_by_identity_until_it_detaches(spec: ImageSpec) {
    let session = DiskImageSession::acquire();
    let mut image = DiskImage::attach(&session, spec).expect("attach the image");
    let before = image.volumes()[0].mount_point.clone();
    let identity = AppVolumeProvider
        .mount_identity(&before)
        .expect("a mounted volume's root names its filesystem");
    assert_eq!(AppVolumeProvider.is_mounted(identity), Some(true));

    let after = image.rename_volume(0).expect("rename the volume").mount_point.clone();

    assert_ne!(after, before, "the rename moved the mount point");
    assert_eq!(
        AppVolumeProvider.mount_identity(&before),
        None,
        "nothing is mounted at the old root any more"
    );
    assert_eq!(
        AppVolumeProvider.mount_identity(&after),
        Some(identity),
        "the same filesystem is mounted at the new root"
    );
    assert_eq!(
        AppVolumeProvider.is_mounted(identity),
        Some(true),
        "a renamed drive is still mounted, so a removable stop must keep waiting on its work"
    );

    image.force_detach().expect("detach the image");
    crate::test_support::wait_until(
        Duration::from_secs(10),
        "the detached filesystem to leave the mount table",
        || AppVolumeProvider.is_mounted(identity) == Some(false),
    );
}

#[test]
#[ignore = "attaches a real APFS disk image via hdiutil; run with --run-ignored"]
fn a_renamed_apfs_volume_stays_mounted_by_identity_until_it_detaches() {
    a_renamed_volume_stays_mounted_by_identity_until_it_detaches(ImageSpec::Apfs);
}

#[test]
#[ignore = "attaches a real HFS+ disk image via hdiutil; run with --run-ignored"]
fn a_renamed_hfs_volume_stays_mounted_by_identity_until_it_detaches() {
    a_renamed_volume_stays_mounted_by_identity_until_it_detaches(ImageSpec::Hfs);
}
