//! What happens to the new bytes when a safe-replace finalize can't give them
//! the destination's name.
//!
//! By then the original is gone (the finalize deletes it between the last byte
//! and the rename), so the temp is the ONLY complete copy in existence. It must
//! leave `.cmdr-tmp-*` space, because `cleanup.rs::reap_stale_transfer_temps`
//! matches on that marker plus an age and runs at the start of every transfer
//! into the directory: an hour later it would delete the user's only copy.

use super::cleanup::reap_stale_transfer_temps;
use super::faulty_volume::forward_volume_methods;
use super::finalize::finalize_safe_replace;
use crate::file_system::listing::FileEntry;
use crate::file_system::volume::{InMemoryVolume, Volume, VolumeError};
use crate::file_system::write_operations::event_sinks::CollectorEventSink;
use crate::file_system::write_operations::state::WriteOperationState;
use crate::file_system::write_operations::types::{ConflictResolution, VolumeCopyConfig, WriteOperationError};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

/// A destination whose `rename` refuses to land anything at `fails_onto`, and
/// forwards everything else. Models the disconnect at the exact instant the
/// finalize swaps the completed temp over the original: `delete(orig)` goes
/// through, the rename that follows doesn't.
struct RenameRefusesOne {
    inner: Arc<InMemoryVolume>,
    fails_onto: PathBuf,
}

impl Volume for RenameRefusesOne {
    forward_volume_methods!(inner =>
        name, root, list_directory, get_metadata, exists, is_directory, create_file,
        create_directory, delete, get_space_info, supports_streaming, open_read_stream,
        write_from_stream,
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
        if to != self.fails_onto {
            return self.inner.rename(from, to, force);
        }
        Box::pin(async {
            Err(VolumeError::IoError {
                message: "simulated disconnect during the finalize rename".to_string(),
                raw_os_error: None,
            })
        })
    }
}

/// The destination as a `dyn Volume`, plus the inner volume the cells read
/// through. `/notes.txt` holds the user's old file; a completed temp beside it
/// holds the new bytes, exactly as the streaming write leaves them.
async fn dest_mid_finalize(fails_onto: &str) -> (Arc<InMemoryVolume>, Arc<dyn Volume>, PathBuf) {
    let inner = Arc::new(InMemoryVolume::new("Dest").with_space_info(10_000_000, 10_000_000));
    inner.create_file(Path::new("/notes.txt"), b"OLD").await.unwrap();
    let temp = PathBuf::from("/notes.txt.cmdr-tmp-11111111");
    inner.create_file(&temp, b"NEW").await.unwrap();
    let dest: Arc<dyn Volume> = Arc::new(RenameRefusesOne {
        inner: Arc::clone(&inner),
        fails_onto: PathBuf::from(fails_onto),
    });
    (inner, dest, temp)
}

/// What `path` holds right now, or `None` if it isn't there.
async fn contents(volume: &InMemoryVolume, path: &Path) -> Option<Vec<u8>> {
    let mut stream = volume.open_read_stream(path).await.ok()?;
    let mut buf = Vec::new();
    while let Some(Ok(chunk)) = stream.next_chunk().await {
        buf.extend_from_slice(&chunk);
    }
    Some(buf)
}

async fn names_in_root(volume: &InMemoryVolume) -> Vec<String> {
    volume
        .list_directory(Path::new("/"), None)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect()
}

/// The rescue: the complete new bytes take a real filename next to where they
/// were meant to land, and nothing is left wearing a temp name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_finalize_moves_the_new_data_out_of_temp_space() {
    let (inner, dest, temp) = dest_mid_finalize("/notes.txt").await;

    let failure = finalize_safe_replace(&dest, &temp, Path::new("/notes.txt"))
        .await
        .expect_err("the finalize rename is rigged to fail");

    assert_eq!(
        failure.new_data_at.as_deref(),
        Some(Path::new("/notes (recovered).txt")),
        "the failure has to name where the user's new file actually is"
    );
    assert_eq!(
        contents(&inner, Path::new("/notes (recovered).txt")).await.as_deref(),
        Some(&b"NEW"[..]),
        "and that path has to hold the complete new bytes"
    );
    let names = names_in_root(&inner).await;
    assert!(
        !names.iter().any(|n| n.contains(".cmdr-tmp-")),
        "nothing may still wear a temp name; found {names:?}"
    );
}

/// The point of the rescue: the hourly stale-temp reap can't touch the rescued
/// file, however long it sits there.
///
/// Pre-fix the new bytes stayed at `notes.txt.cmdr-tmp-<uuid>`, which is exactly
/// what `reap_stale_transfer_temps` matches once it is an hour old, and the next
/// transfer into that folder deleted the only copy.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_stale_temp_reap_cannot_touch_rescued_data() {
    let (inner, dest, temp) = dest_mid_finalize("/notes.txt").await;
    finalize_safe_replace(&dest, &temp, Path::new("/notes.txt"))
        .await
        .expect_err("the finalize rename is rigged to fail");

    // A day later, the next transfer into the same folder runs the reap.
    for name in names_in_root(&inner).await {
        inner.set_modified_at(&PathBuf::from("/").join(&name), Some(1));
    }
    reap_stale_transfer_temps(&dest, Path::new("/")).await;

    assert_eq!(
        contents(&inner, Path::new("/notes (recovered).txt")).await.as_deref(),
        Some(&b"NEW"[..]),
        "the only copy of the user's new file must survive the reap"
    );
}

/// A ` (recovered)` name already in use continues the house ` (N)` series rather
/// than replacing whatever is there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_taken_recovered_name_continues_the_house_series() {
    let (inner, dest, temp) = dest_mid_finalize("/notes.txt").await;
    inner
        .create_file(Path::new("/notes (recovered).txt"), b"AN EARLIER RESCUE")
        .await
        .unwrap();

    let failure = finalize_safe_replace(&dest, &temp, Path::new("/notes.txt"))
        .await
        .expect_err("the finalize rename is rigged to fail");

    assert_eq!(
        failure.new_data_at.as_deref(),
        Some(Path::new("/notes (recovered) (1).txt"))
    );
    assert_eq!(
        contents(&inner, Path::new("/notes (recovered).txt")).await.as_deref(),
        Some(&b"AN EARLIER RESCUE"[..]),
        "the earlier rescue is somebody's file too"
    );
}

/// When the destination refuses every rename — the same dead link that failed
/// the finalize — the bytes stay under the temp name and the failure says so.
/// Reporting the recovered path we never created would send the user to a file
/// that isn't there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rescue_that_cannot_rename_reports_the_temp_it_left() {
    let inner = Arc::new(
        InMemoryVolume::new("Dest")
            .with_space_info(10_000_000, 10_000_000)
            .with_rename_failing(VolumeError::DeviceDisconnected("gone".to_string())),
    );
    inner.create_file(Path::new("/notes.txt"), b"OLD").await.unwrap();
    let temp = PathBuf::from("/notes.txt.cmdr-tmp-22222222");
    inner.create_file(&temp, b"NEW").await.unwrap();
    let dest: Arc<dyn Volume> = Arc::clone(&inner) as Arc<dyn Volume>;

    let failure = finalize_safe_replace(&dest, &temp, Path::new("/notes.txt"))
        .await
        .expect_err("every rename fails here");

    assert_eq!(failure.new_data_at.as_deref(), Some(temp.as_path()));
    assert_eq!(contents(&inner, &temp).await.as_deref(), Some(&b"NEW"[..]));
}

/// A destination whose `delete` of `refuses` answers `PermissionDenied`, and
/// forwards everything else. With `goes_through`, the delete HAPPENS and the
/// answer still says it didn't: a transport that dropped the response after
/// the server acted.
struct DeleteRefusesOne {
    inner: Arc<InMemoryVolume>,
    refuses: PathBuf,
    goes_through: bool,
}

impl Volume for DeleteRefusesOne {
    forward_volume_methods!(inner =>
        name, root, lane_key, list_directory, get_metadata, exists, is_directory, create_file,
        create_directory, create_directory_all, rename, get_space_info, local_path, supports_streaming,
        supports_export, supports_local_fs_access, operations_are_local, max_concurrent_ops,
        create_directory_errors_on_existing_dir, scan_for_copy, scan_for_copy_batch, scan_for_conflicts,
        open_read_stream, write_from_stream, write_is_single_shot,
    );
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn delete<'a>(&'a self, path: &'a Path) -> Pin<Box<dyn Future<Output = Result<(), VolumeError>> + Send + 'a>> {
        Box::pin(async move {
            if path != self.refuses {
                return self.inner.delete(path).await;
            }
            if self.goes_through {
                self.inner.delete(path).await?;
            }
            Err(VolumeError::PermissionDenied(path.display().to_string()))
        })
    }
}

/// `/notes.txt` holds the user's old file and a completed temp beside it holds
/// the new bytes; the destination refuses to delete `/notes.txt`.
async fn dest_refusing_the_delete(goes_through: bool) -> (Arc<InMemoryVolume>, Arc<dyn Volume>, PathBuf) {
    let inner = Arc::new(InMemoryVolume::new("Dest").with_space_info(10_000_000, 10_000_000));
    inner.create_file(Path::new("/notes.txt"), b"OLD").await.unwrap();
    let temp = PathBuf::from("/notes.txt.cmdr-tmp-44444444");
    inner.create_file(&temp, b"NEW").await.unwrap();
    let dest: Arc<dyn Volume> = Arc::new(DeleteRefusesOne {
        inner: Arc::clone(&inner),
        refuses: PathBuf::from("/notes.txt"),
        goes_through,
    });
    (inner, dest, temp)
}

/// The server wouldn't let the original go, so the swap can't happen: the
/// original stays exactly as it was, the refusal reaches the caller typed, and
/// the complete-but-unplaced temp goes at once rather than sitting in the
/// user's folder until an hourly reap. The source still holds those bytes, so
/// removing them loses nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_delete_keeps_the_original_and_takes_its_own_temp_away() {
    let (inner, dest, temp) = dest_refusing_the_delete(false).await;

    let failure = finalize_safe_replace(&dest, &temp, Path::new("/notes.txt"))
        .await
        .expect_err("the delete is rigged to be refused");

    assert!(
        matches!(failure.error, VolumeError::PermissionDenied(_)),
        "the refusal must arrive typed, got {:?}",
        failure.error
    );
    assert!(
        failure.new_data_at.is_none(),
        "nothing was cleared, so nothing was rescued"
    );
    assert_eq!(
        contents(&inner, Path::new("/notes.txt")).await.as_deref(),
        Some(&b"OLD"[..]),
        "❗ the user's file must be exactly as it was"
    );
    assert_eq!(
        names_in_root(&inner).await,
        vec!["notes.txt".to_string()],
        "no `.cmdr-tmp-*` may be left beside it"
    );
}

/// A delete that ANSWERED with a refusal but went through anyway (the response
/// lost on the way back) leaves the name empty. Discarding the temp then would
/// leave neither file at the destination; landing it is what the user asked for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delete_that_went_through_despite_its_answer_still_lands_the_new_bytes() {
    let (inner, dest, temp) = dest_refusing_the_delete(true).await;

    finalize_safe_replace(&dest, &temp, Path::new("/notes.txt"))
        .await
        .expect("the original is gone, so the swap completes");

    assert_eq!(
        contents(&inner, Path::new("/notes.txt")).await.as_deref(),
        Some(&b"NEW"[..])
    );
    assert_eq!(names_in_root(&inner).await, vec!["notes.txt".to_string()]);
}

/// When the destination can't even say whether the original survived, the temp
/// stays: if the original IS gone, the temp is the only copy at the destination,
/// and this is not the moment to guess.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_delete_nobody_can_confirm_keeps_the_temp() {
    let (inner, dest, temp) = dest_refusing_the_delete(false).await;
    inner.set_stat_failing(Path::new("/notes.txt"));

    let failure = finalize_safe_replace(&dest, &temp, Path::new("/notes.txt"))
        .await
        .expect_err("the delete is rigged to be refused");

    assert!(matches!(failure.error, VolumeError::PermissionDenied(_)));
    assert_eq!(contents(&inner, &temp).await.as_deref(), Some(&b"NEW"[..]));
}

/// A refused replace through the whole copy pipeline, on both drivers: the
/// user's file stays, the copy fails typed, and no staging is left behind.
async fn copy_over_a_file_the_server_wont_let_go(names: &[&str]) {
    let source = Arc::new(InMemoryVolume::new("Source").with_space_info(10_000_000, 10_000_000));
    for name in names {
        source
            .create_file(&Path::new("/").join(name), format!("SRC-{name}").as_bytes())
            .await
            .unwrap();
    }
    let inner = Arc::new(InMemoryVolume::new("Dest").with_space_info(10_000_000, 10_000_000));
    inner.create_directory(Path::new("/dest")).await.unwrap();
    inner
        .create_file(Path::new("/dest/notes.txt"), b"THE USER'S FILE")
        .await
        .unwrap();
    let dest: Arc<dyn Volume> = Arc::new(DeleteRefusesOne {
        inner: Arc::clone(&inner),
        refuses: PathBuf::from("/dest/notes.txt"),
        goes_through: false,
    });

    let sources: Vec<PathBuf> = names.iter().map(|n| Path::new("/").join(n)).collect();
    let result = super::copy::copy_volumes_with_progress(
        Arc::new(CollectorEventSink::new()),
        "test-op-refused-replace",
        &Arc::new(WriteOperationState::new(std::time::Duration::from_millis(50))),
        source as Arc<dyn Volume>,
        &sources,
        dest,
        Path::new("/dest"),
        &VolumeCopyConfig {
            conflict_resolution: ConflictResolution::Overwrite,
            ..VolumeCopyConfig::default()
        },
    )
    .await;
    let Err(failure) = result else {
        panic!("a replace the destination refused must not report success");
    };
    assert!(
        matches!(failure.error, WriteOperationError::PermissionDenied { .. }),
        "got {:?}",
        failure.error
    );
    assert_eq!(
        contents(&inner, Path::new("/dest/notes.txt")).await.as_deref(),
        Some(&b"THE USER'S FILE"[..]),
        "❗ the user's file must be exactly as it was"
    );
    let leftovers: Vec<String> = inner
        .list_directory(Path::new("/dest"), None)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .filter(|n| n.contains(".cmdr-tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "no staging may be left behind: {leftovers:?}");
}

/// One source, so the SERIAL driver runs it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serial_a_refused_replace_leaves_no_staging() {
    copy_over_a_file_the_server_wont_let_go(&["notes.txt"]).await;
}

/// Three sources, so the CONCURRENT driver runs them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_a_refused_replace_leaves_no_staging() {
    copy_over_a_file_the_server_wont_let_go(&["a.txt", "notes.txt", "z.txt"]).await;
}

/// A finalize that fails on the DELETE rescued nothing and lost nothing: the
/// destination still holds the user's file. Here the destination refuses EVERY
/// delete, so the temp can't be taken away either; it stays for the stale-temp
/// reap, which is safe because the source still holds its bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_finalize_that_never_deleted_the_original_reports_no_rescue() {
    let inner = Arc::new(
        InMemoryVolume::new("Dest")
            .with_space_info(10_000_000, 10_000_000)
            .with_delete_failing(),
    );
    inner.create_file(Path::new("/notes.txt"), b"OLD").await.unwrap();
    let temp = PathBuf::from("/notes.txt.cmdr-tmp-33333333");
    inner.create_file(&temp, b"NEW").await.unwrap();
    let dest: Arc<dyn Volume> = Arc::clone(&inner) as Arc<dyn Volume>;

    let failure = finalize_safe_replace(&dest, &temp, Path::new("/notes.txt"))
        .await
        .expect_err("the delete is rigged to fail");

    assert!(
        failure.new_data_at.is_none(),
        "nothing was rescued, because nothing was lost"
    );
    assert_eq!(
        contents(&inner, Path::new("/notes.txt")).await.as_deref(),
        Some(&b"OLD"[..])
    );
    assert_eq!(
        contents(&inner, &temp).await.as_deref(),
        Some(&b"NEW"[..]),
        "a temp the destination won't delete stays, whole, for the stale-temp reap"
    );
}

/// The unused import guard: `FileEntry` is what `list_directory` answers with,
/// and the forwarding macro names the type in its expansion.
#[allow(dead_code, reason = "the macro expansion names the type")]
type ListingRow = FileEntry;
