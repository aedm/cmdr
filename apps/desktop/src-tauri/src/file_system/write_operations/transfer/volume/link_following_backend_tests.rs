//! The volume move engines on a backend whose `is_directory` FOLLOWS a link.
//!
//! `LocalPosixVolume` answers `is_directory` with an `lstat`, so a suite on it
//! can't see what an ADB phone or an SFTP server does: their stat follows the
//! link, and a link to a folder answers "directory". Anything that recursed on
//! that answer walked into the link's target and deleted or wrote there, a
//! folder the user never selected (#140). `Volume::entry_kind` is the question
//! that can't be fooled, and these cells pin that every recursing site asks it.
//!
//! The rig is a real `LocalPosixVolume` over a tempdir (real links, real
//! `rename`), behind a wrapper that keeps the trait's DEFAULT `is_directory` and
//! `entry_kind`, the two answers a stat-following backend gets.

#![cfg(unix)]

use super::move_cross::move_volumes_with_progress;
use super::move_same::move_within_same_volume_with_progress;
use super::rename_merge_test_support::{exists, make_state, mkdir, read, write_file};
use crate::file_system::listing::FileEntry;
use crate::file_system::volume::{
    CopyScanResult, InMemoryVolume, ListingProgress, LocalPosixVolume, Volume, VolumeError, VolumeReadStream,
};
use crate::file_system::write_operations::event_sinks::CollectorEventSink;
use crate::file_system::write_operations::types::{ConflictResolution, VolumeCopyConfig};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use tempfile::TempDir;

/// A `LocalPosixVolume` minus its `lstat`-based `is_directory` and `entry_kind`
/// overrides: both fall to the trait defaults, which read `get_metadata`, whose
/// `is_directory` is true for a link to a folder (with `is_symlink` beside it).
struct LinkFollowingVolume {
    inner: LocalPosixVolume,
}

impl Volume for LinkFollowingVolume {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn root(&self) -> &Path {
        self.inner.root()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn supports_streaming(&self) -> bool {
        true
    }
    fn list_directory<'a>(
        &'a self,
        path: &'a Path,
        on_progress: Option<&'a (dyn Fn(ListingProgress) + Sync)>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<FileEntry>, VolumeError>> + Send + 'a>> {
        self.inner.list_directory(path, on_progress)
    }
    fn get_metadata<'a>(
        &'a self,
        path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<FileEntry, VolumeError>> + Send + 'a>> {
        self.inner.get_metadata(path)
    }
    fn exists<'a>(&'a self, path: &'a Path) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        self.inner.exists(path)
    }
    fn delete<'a>(&'a self, path: &'a Path) -> Pin<Box<dyn Future<Output = Result<(), VolumeError>> + Send + 'a>> {
        self.inner.delete(path)
    }
    fn rename<'a>(
        &'a self,
        from: &'a Path,
        to: &'a Path,
        force: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), VolumeError>> + Send + 'a>> {
        self.inner.rename(from, to, force)
    }
    fn create_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<(), VolumeError>> + Send + 'a>> {
        self.inner.create_directory(path)
    }
    fn scan_for_copy<'a>(
        &'a self,
        path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<CopyScanResult, VolumeError>> + Send + 'a>> {
        self.inner.scan_for_copy(path)
    }
    fn open_read_stream<'a>(
        &'a self,
        path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn VolumeReadStream>, VolumeError>> + Send + 'a>> {
        self.inner.open_read_stream(path)
    }
}

fn following_volume() -> (Arc<dyn Volume>, TempDir) {
    let dir = TempDir::new().unwrap();
    let volume: Arc<dyn Volume> = Arc::new(LinkFollowingVolume {
        inner: LocalPosixVolume::new("V", dir.path().to_path_buf()),
    });
    (volume, dir)
}

/// The shared fixture: a target folder holding one file, OUTSIDE the selection.
fn plant_target(root: &Path) {
    write_file(root, "outside/target/inside.txt", b"OUTSIDE THE SELECTION");
}

fn link(root: &Path, at: &str, to: &str) {
    std::os::unix::fs::symlink(root.join(to), root.join(at)).expect("symlink");
}

fn is_link(root: &Path, rel: &str) -> bool {
    std::fs::symlink_metadata(root.join(rel))
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

fn config(resolution: ConflictResolution) -> VolumeCopyConfig {
    VolumeCopyConfig {
        conflict_resolution: resolution,
        progress_interval_ms: 0,
        ..VolumeCopyConfig::default()
    }
}

async fn move_across(source: &Arc<dyn Volume>, op_id: &str, sources: &[PathBuf]) -> Arc<dyn Volume> {
    let dest: Arc<dyn Volume> = Arc::new(InMemoryVolume::new("Dest").with_space_info(10_000_000, 10_000_000));
    let result = move_volumes_with_progress(
        Arc::new(CollectorEventSink::new()),
        op_id,
        &make_state(),
        Arc::clone(source),
        sources,
        Arc::clone(&dest),
        Path::new("/"),
        &config(ConflictResolution::Skip),
    )
    .await;
    assert!(result.is_ok(), "{op_id}: expected Ok, got {result:?}");
    dest
}

/// A selected link moved to another volume lands there as a copy of what it
/// points at (the destination can't hold a link), and the source sweep removes
/// the LINK. The target keeps every byte.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cross_volume_move_of_a_dir_link_removes_the_link_never_the_target() {
    let (source, dir) = following_volume();
    let root = dir.path();
    plant_target(root);
    mkdir(root, "src");
    link(root, "src/album", "outside/target");

    let dest = move_across(&source, "op-follow-cross-top-link", &[PathBuf::from("src/album")]).await;

    assert_eq!(
        read(root, "outside/target/inside.txt"),
        b"OUTSIDE THE SELECTION",
        "the source sweep must delete the link, never walk into its target"
    );
    assert!(
        !is_link(root, "src/album") && !exists(root, "src/album"),
        "the moved link is gone"
    );
    assert!(
        dest.exists(Path::new("/album/inside.txt")).await,
        "the destination got the copy"
    );
}

/// The same sweep one level down: a moved folder holding a link to a folder.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cross_volume_move_of_a_folder_holding_a_dir_link_spares_the_target() {
    let (source, dir) = following_volume();
    let root = dir.path();
    plant_target(root);
    write_file(root, "src/album/plain.txt", b"PLAIN");
    link(root, "src/album/link", "outside/target");

    let dest = move_across(&source, "op-follow-cross-child-link", &[PathBuf::from("src/album")]).await;

    assert_eq!(read(root, "outside/target/inside.txt"), b"OUTSIDE THE SELECTION");
    assert!(!exists(root, "src/album"), "the moved folder is gone, link and all");
    assert!(dest.exists(Path::new("/album/plain.txt")).await);
}

/// A same-volume move of a real folder onto a same-named LINK to a folder: the
/// destination's type answer must not follow the link, or the merge renames the
/// source's files into the link's target.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_same_volume_move_onto_a_dir_link_never_lands_in_the_target() {
    let (volume, dir) = following_volume();
    let root = dir.path();
    plant_target(root);
    write_file(root, "src/album/mine.txt", b"MINE");
    mkdir(root, "dst");
    link(root, "dst/album", "outside/target");

    let result = move_within_same_volume_with_progress(
        Arc::new(CollectorEventSink::new()),
        "op-follow-same-dest-link",
        &make_state(),
        Arc::clone(&volume),
        &[PathBuf::from("src/album")],
        Path::new("dst"),
        &config(ConflictResolution::Skip),
    )
    .await;
    assert!(result.is_ok(), "expected Ok, got {result:?}");

    assert!(
        !exists(root, "outside/target/mine.txt"),
        "nothing may land in the link's target"
    );
    assert_eq!(
        read(root, "src/album/mine.txt"),
        b"MINE",
        "the skipped folder stays home"
    );
    assert!(is_link(root, "dst/album"), "the destination link is untouched");
}
