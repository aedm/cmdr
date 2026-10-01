//! Tests for "Open with" on a file only a route serves (`open_with_extract`).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::ViewerError;
use super::materialize::reap_orphan_temps;
use super::open_with_extract::{is_open_with_temp_name, launch_paths_in, listing_path_in, reap_open_with_temps};

/// Registers a real local-FS "root" volume so the archive route finds a parent.
fn ensure_root_volume() {
    use crate::file_system::volume::LocalPosixVolume;
    use crate::file_system::volume::manager::get_volume_manager;
    get_volume_manager().register_if_absent("root", Arc::new(LocalPosixVolume::new("Test root", "/")));
}

fn build_zip(path: &Path, entries: &[(&str, &[u8])]) {
    use zip::write::SimpleFileOptions;
    let file = std::fs::File::create(path).expect("create zip");
    let mut writer = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, content) in entries {
        writer.start_file(*name, opts).expect("start file");
        writer.write_all(content).expect("write entry");
    }
    writer.finish().expect("finish zip");
}

const CAP: u64 = 1024 * 1024;

#[test]
fn an_ordinary_path_launches_as_it_is() {
    let dir = tempfile::tempdir().expect("open-with dir");
    let plain = PathBuf::from("/Users/me/report.pdf");
    let got = launch_paths_in(std::slice::from_ref(&plain), dir.path(), CAP).expect("no extraction needed");
    assert_eq!(got, vec![plain]);
    assert_eq!(
        std::fs::read_dir(dir.path()).expect("read dir").count(),
        0,
        "nothing written"
    );
}

#[test]
fn a_file_inside_an_archive_launches_as_a_fresh_read_only_copy() {
    ensure_root_volume();
    let src = tempfile::tempdir().expect("src dir");
    let zip = src.path().join("bundle.zip");
    build_zip(&zip, &[("docs/report.pdf", b"%PDF-1.4 hello")]);
    let dir = tempfile::tempdir().expect("open-with dir");
    let plain = src.path().join("beside.txt");

    let got = launch_paths_in(&[zip.join("docs/report.pdf"), plain.clone()], dir.path(), CAP).expect("extract");

    let copy = &got[0];
    assert_eq!(
        copy.file_name().and_then(|n| n.to_str()),
        Some("report.pdf"),
        "keeps the name"
    );
    assert_eq!(std::fs::read(copy).expect("read copy"), b"%PDF-1.4 hello");
    let subdir = copy.parent().expect("subdir");
    assert_eq!(subdir.parent(), Some(dir.path()), "lands in the open-with dir");
    assert!(is_open_with_temp_name(
        &subdir.file_name().expect("name").to_string_lossy()
    ));
    let perms = std::fs::metadata(copy).expect("meta").permissions();
    assert!(
        perms.readonly(),
        "a read-only copy: edits can't look like they reach the archive"
    );
    assert_eq!(got[1], plain, "an ordinary path beside it is untouched");
}

#[test]
fn every_launch_gets_its_own_copy() {
    ensure_root_volume();
    let src = tempfile::tempdir().expect("src dir");
    let zip = src.path().join("bundle.zip");
    build_zip(&zip, &[("a.txt", b"one")]);
    let dir = tempfile::tempdir().expect("open-with dir");
    let inner = zip.join("a.txt");

    let first = launch_paths_in(std::slice::from_ref(&inner), dir.path(), CAP).expect("first");
    let second = launch_paths_in(std::slice::from_ref(&inner), dir.path(), CAP).expect("second");

    assert_ne!(
        first[0], second[0],
        "no dedup: an earlier app may still hold the first copy"
    );
    assert!(first[0].exists() && second[0].exists());
}

#[test]
fn an_oversize_file_is_refused_before_anything_is_written() {
    ensure_root_volume();
    let src = tempfile::tempdir().expect("src dir");
    let zip = src.path().join("big.zip");
    build_zip(&zip, &[("big.bin", &[7u8; 4096])]);
    let dir = tempfile::tempdir().expect("open-with dir");

    let err = launch_paths_in(&[zip.join("big.bin")], dir.path(), 100).expect_err("over the cap");

    assert!(matches!(err, ViewerError::TooLargeToPreview { .. }), "got {err:?}");
    assert_eq!(
        std::fs::read_dir(dir.path()).expect("read dir").count(),
        0,
        "nothing left behind"
    );
}

#[test]
fn each_reaper_takes_only_its_own_family() {
    let dir = tempfile::tempdir().expect("shared parent");
    let open_with = dir.path().join(".cmdr-open-with-1");
    let viewer = dir.path().join(".cmdr-viewer-1");
    std::fs::create_dir_all(&open_with).expect("mk open-with temp");
    std::fs::create_dir_all(&viewer).expect("mk viewer temp");

    reap_orphan_temps(dir.path());
    assert!(
        open_with.exists(),
        "the viewer's reaper must not touch an open-with temp"
    );
    assert!(!viewer.exists());

    std::fs::create_dir_all(&viewer).expect("mk viewer temp again");
    reap_open_with_temps(dir.path());
    assert!(!open_with.exists());
    assert!(viewer.exists(), "the open-with reaper must not touch a viewer temp");
}

#[test]
fn a_file_inside_an_archive_lists_apps_against_a_real_stand_in_with_its_extension() {
    let dir = tempfile::tempdir().expect("open-with dir");
    let inner = PathBuf::from("/Users/me/bundle.zip/photos/IMG_1.HEIC");

    let stand_in = listing_path_in(&inner, dir.path());

    assert_ne!(stand_in, inner);
    assert!(
        stand_in.is_file(),
        "LaunchServices answers no apps for a path with nothing at it"
    );
    assert_eq!(stand_in.extension().and_then(|e| e.to_str()), Some("HEIC"));
    assert_eq!(
        listing_path_in(&inner, dir.path()),
        stand_in,
        "one stand-in per extension"
    );
}

#[test]
fn an_ordinary_path_lists_apps_against_itself() {
    let dir = tempfile::tempdir().expect("open-with dir");
    let plain = PathBuf::from("/Users/me/report.pdf");
    assert_eq!(listing_path_in(&plain, dir.path()), plain);
}
