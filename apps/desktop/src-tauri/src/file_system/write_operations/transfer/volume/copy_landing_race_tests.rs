//! A name another writer takes while our bytes are still on their way is theirs,
//! and a copy that fails over it must not delete it on the way out.
//!
//! The copy looked, found the name free, and staged its bytes on a
//! `.cmdr-tmp-*` sibling. Before the landing rename, someone else (another
//! app, another person on the share) creates a file at that name. The landing
//! rightly refuses (`staged_write.rs::land`, `LandingName::ExpectedFree`), and
//! the copy fails naming the clash. What these pin is the step AFTER: the
//! driver's post-loop "partial cleanup" used to treat the failed file's FINAL
//! name as our partial and delete it, which on this path is the other writer's
//! file. Under ordinary staging our bytes never touch the final name, so there
//! is nothing of ours there to clean.
//!
//! The live-server twins are `backend_suites/network_safety_test_support.rs`'s
//! `a_name_taken_mid_upload_is_never_replaced`, which found this on WebDAV.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use super::tests::make_state;
use super::*;
use crate::file_system::volume::{InMemoryVolume, Volume, VolumeError};
use crate::file_system::write_operations::event_sinks::CollectorEventSink;
use crate::file_system::write_operations::transfer::volume::forward_volume_methods;
use crate::file_system::write_operations::types::{ConflictResolution, WriteOperationError};

/// What the other writer puts at the name.
const THEIRS: &[u8] = b"someone else's file, written while ours was in flight";

/// A destination where another writer takes `taken` the moment our landing
/// rename arrives for it, so the rename meets an occupied name exactly as it
/// would on a live share.
struct TakenAtLanding {
    inner: InMemoryVolume,
    taken: &'static str,
}

impl Volume for TakenAtLanding {
    forward_volume_methods!(
        inner => name,
        root,
        lane_key,
        list_directory,
        get_metadata,
        exists,
        is_directory,
        create_file,
        create_directory,
        create_directory_all,
        delete,
        get_space_info,
        local_path,
        supports_streaming,
        supports_export,
        supports_local_fs_access,
        operations_are_local,
        max_concurrent_ops,
        create_directory_errors_on_existing_dir,
        scan_for_copy,
        scan_for_copy_batch,
        scan_for_conflicts,
        open_read_stream,
        write_from_stream,
        write_is_single_shot,
    );

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn rename<'a>(
        &'a self,
        from: &'a Path,
        to: &'a Path,
        force: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), VolumeError>> + Send + 'a>> {
        Box::pin(async move {
            if to.file_name().is_some_and(|n| n == self.taken) && !self.inner.exists(to).await {
                self.inner.create_file(to, THEIRS).await?;
            }
            self.inner.rename(from, to, force).await
        })
    }
}

/// Copies `names` from a fresh source into `/dest` on a destination where
/// another writer takes `taken` at landing, and returns the destination and the
/// copy's failure.
async fn copy_with_a_name_taken_at_landing(
    names: &[&str],
    taken: &'static str,
    policy: ConflictResolution,
) -> (Arc<TakenAtLanding>, WriteOperationError) {
    let source = Arc::new(InMemoryVolume::new("Source"));
    for name in names {
        source
            .create_file(&Path::new("/").join(name), format!("SRC-{name}").as_bytes())
            .await
            .expect("seeding the source");
    }
    let dest = Arc::new(TakenAtLanding {
        inner: InMemoryVolume::new("Dest"),
        taken,
    });
    dest.create_directory(Path::new("/dest"))
        .await
        .expect("the destination folder");

    let sources: Vec<PathBuf> = names.iter().map(|n| Path::new("/").join(n)).collect();
    let result = copy_volumes_with_progress(
        Arc::new(CollectorEventSink::new()),
        "test-op-name-taken-at-landing",
        &make_state(),
        source as Arc<dyn Volume>,
        &sources,
        Arc::clone(&dest) as Arc<dyn Volume>,
        Path::new("/dest"),
        &VolumeCopyConfig {
            conflict_resolution: policy,
            ..VolumeCopyConfig::default()
        },
    )
    .await;
    let Err(failure) = result else {
        panic!("a copy whose landing met a name nobody resolved must not report success");
    };
    (dest, failure.error)
}

async fn assert_theirs_survives(dest: &TakenAtLanding, error: &WriteOperationError, taken: &str) {
    assert!(
        matches!(error, WriteOperationError::DestinationExists { .. }),
        "the copy fails naming the clash, got {error:?}"
    );
    let path = Path::new("/dest").join(taken);
    let meta = dest.inner.get_metadata(&path).await;
    assert!(
        meta.is_ok(),
        "❗ the other writer's {taken} must survive the failed copy; it was deleted"
    );
    assert_eq!(
        meta.ok().and_then(|m| m.size),
        Some(THEIRS.len() as u64),
        "and it must be exactly what they wrote"
    );
}

/// One source, so the SERIAL driver runs it.
#[tokio::test]
async fn serial_a_name_taken_at_landing_is_never_deleted_by_the_failed_copy() {
    let (dest, error) = copy_with_a_name_taken_at_landing(&["photo.jpg"], "photo.jpg", ConflictResolution::Skip).await;
    assert_theirs_survives(&dest, &error, "photo.jpg").await;
}

/// Three sources, so the CONCURRENT driver runs them.
#[tokio::test]
async fn concurrent_a_name_taken_at_landing_is_never_deleted_by_the_failed_copy() {
    let (dest, error) =
        copy_with_a_name_taken_at_landing(&["a.jpg", "photo.jpg", "z.jpg"], "photo.jpg", ConflictResolution::Skip)
            .await;
    assert_theirs_survives(&dest, &error, "photo.jpg").await;
}

/// The same under Rename: the name was free when the copy looked, so no
/// resolution ran and nothing claimed it.
#[tokio::test]
async fn serial_a_name_taken_at_landing_survives_under_rename_too() {
    let (dest, error) =
        copy_with_a_name_taken_at_landing(&["photo.jpg"], "photo.jpg", ConflictResolution::Rename).await;
    assert_theirs_survives(&dest, &error, "photo.jpg").await;
}
