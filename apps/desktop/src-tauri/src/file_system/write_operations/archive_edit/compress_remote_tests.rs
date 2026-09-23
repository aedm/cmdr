//! Compress onto a REMOTE parent (SMB / MTP, modeled by a non-local
//! `InMemoryVolume`). The net-new surface here is the SEED: a remote target must
//! be seeded THROUGH the parent volume, because `route_archive_copy_into`'s remote
//! path PULLS the target before editing — a local-FS seed would be invisible to
//! it. These pin: a fresh remote target gets seeded and packed, an overwrite
//! replaces the remote file with a fresh zip (never merges into it), the MTP swap
//! shape (delete-then-rename over a brand-new target) works, and no temp debris is
//! left at the user's destination.

use super::compress::compress_start;
use super::test_support::*;
use crate::file_system::volume::InMemoryVolume;
use uuid::Uuid;

/// Registers a NON-local `InMemoryVolume` (the remote-parent stand-in) with `dir`
/// created but NO target zip — compress must create it. `mtp_style` allows same-name
/// siblings (`create_directory_errors_on_existing_dir() == false`), so the swap
/// takes MTP's delete-then-rename path instead of SMB's atomic rename-replace.
/// Unregister with `get_volume_manager().unregister(&id)` when done.
async fn register_remote_parent(dir: &Path, mtp_style: bool) -> (String, Arc<InMemoryVolume>) {
    let id = format!("remote-parent-{}", Uuid::new_v4());
    let mut vol = InMemoryVolume::new("Remote").with_lane_key(id.clone());
    if mtp_style {
        vol = vol.with_sibling_duplicates_allowed();
    }
    vol.create_directory(dir).await.expect("seed parent dir");
    let vol = Arc::new(vol);
    get_volume_manager().register(&id, Arc::clone(&vol) as Arc<dyn Volume>);
    (id, vol)
}

/// A local source volume over a temp dir holding `files` at its root. The common
/// case: compress LOCAL files onto a remote share.
fn local_source_with(files: &[(&str, &[u8])]) -> (tempfile::TempDir, Arc<dyn Volume>) {
    use crate::file_system::volume::backends::LocalPosixVolume;
    let tmp = tempfile::tempdir().expect("tempdir");
    for (name, bytes) in files {
        std::fs::write(tmp.path().join(name), bytes).expect("write source file");
    }
    let vol: Arc<dyn Volume> = Arc::new(LocalPosixVolume::new("src", tmp.path().to_path_buf()));
    (tmp, vol)
}

/// Names of every entry in the archive's parent dir, to assert no leftover
/// `.cmdr-tmp-*` upload temp remains after the swap.
async fn sibling_names(parent: &dyn Volume, archive_path: &Path) -> Vec<String> {
    let dir = archive_path.parent().expect("archive has a parent dir");
    parent
        .list_directory(dir, None)
        .await
        .expect("list parent dir")
        .into_iter()
        .map(|e| e.name)
        .collect()
}

#[tokio::test]
async fn compress_onto_a_remote_parent_seeds_and_packs_local_files() {
    // A local-FS seed at `/share/bundle.zip` is invisible to the remote parent's
    // pull, so pre-seed-through-Volume the copy-into pulls a missing file and the
    // entries never land — this test is RED until the seed goes through the volume.
    let (_src_tmp, source_volume) = local_source_with(&[("one.txt", b"first"), ("two.txt", b"second")]);
    let archive_path = PathBuf::from("/share/bundle.zip");
    let (parent_id, parent) = register_remote_parent(Path::new("/share"), false).await;

    let events = Arc::new(CollectorEventSink::new());
    compress_start(
        Arc::clone(&events) as Arc<dyn OperationEventSink>,
        source_volume,
        vec![PathBuf::from("one.txt"), PathBuf::from("two.txt")],
        archive_path.clone(),
        parent_id.clone(),
        ConflictResolution::Overwrite,
        0,
        None,
        None,
        crate::operation_log::types::Initiator::User,
    )
    .await
    .expect("start remote compress");

    wait_until_async(Duration::from_secs(5), "a terminal event (complete or error)", || {
        !events.complete.lock_ignore_poison().is_empty() || !events.errors.lock_ignore_poison().is_empty()
    })
    .await;
    assert!(
        !events.complete.lock_ignore_poison().is_empty(),
        "remote compress should complete, errors: {:?}",
        events.errors.lock_ignore_poison()
    );

    assert_eq!(
        read_remote_entry(parent.as_ref(), &archive_path, "one.txt")
            .await
            .as_deref(),
        Some(b"first".as_slice()),
        "the first source must land in the remote zip"
    );
    assert_eq!(
        read_remote_entry(parent.as_ref(), &archive_path, "two.txt")
            .await
            .as_deref(),
        Some(b"second".as_slice()),
        "the second source must land in the remote zip"
    );
    // No upload temp debris left at the user's destination.
    let names = sibling_names(parent.as_ref(), &archive_path).await;
    assert!(
        !names.iter().any(|n| n.contains(".cmdr-tmp-")),
        "no upload temp should remain after the swap, got: {names:?}"
    );

    get_volume_manager().unregister(&parent_id);
}

#[tokio::test]
async fn compress_onto_a_remote_parent_overwrites_an_existing_zip_with_a_fresh_archive() {
    // The target already holds a zip on the remote. Compress-overwrite REPLACES it
    // with a fresh archive of just the sources — it never merges into the old one
    // (the seed clears it to empty before the copy-into).
    let (_src_tmp, source_volume) = local_source_with(&[("new.txt", b"brand new")]);
    let archive_path = PathBuf::from("/share/existing.zip");
    let (parent_id, parent) = register_remote_zip(&archive_path, &[("stale.txt", b"old content")]).await;

    let events = Arc::new(CollectorEventSink::new());
    compress_start(
        Arc::clone(&events) as Arc<dyn OperationEventSink>,
        source_volume,
        vec![PathBuf::from("new.txt")],
        archive_path.clone(),
        parent_id.clone(),
        ConflictResolution::Overwrite,
        0,
        None,
        None,
        crate::operation_log::types::Initiator::User,
    )
    .await
    .expect("start remote compress over existing");

    wait_until_async(Duration::from_secs(5), "a terminal event (complete or error)", || {
        !events.complete.lock_ignore_poison().is_empty() || !events.errors.lock_ignore_poison().is_empty()
    })
    .await;
    assert!(
        !events.complete.lock_ignore_poison().is_empty(),
        "remote compress-overwrite should complete, errors: {:?}",
        events.errors.lock_ignore_poison()
    );

    assert_eq!(
        read_remote_entry(parent.as_ref(), &archive_path, "new.txt")
            .await
            .as_deref(),
        Some(b"brand new".as_slice()),
        "the new source must be in the fresh archive"
    );
    assert!(
        read_remote_entry(parent.as_ref(), &archive_path, "stale.txt")
            .await
            .is_none(),
        "the pre-existing entry must be gone — compress-overwrite creates a fresh zip, not a merge"
    );

    get_volume_manager().unregister(&parent_id);
}

#[tokio::test]
async fn compress_onto_an_mtp_style_remote_parent_seeds_and_packs() {
    // An MTP-shaped parent allows same-name siblings, so its swap is
    // delete-then-rename, not the atomic rename-replace. The seed's swap over a
    // BRAND-NEW target must tolerate the missing original (nothing to delete) — this
    // exercises that path without a virtual MTP device.
    let (_src_tmp, source_volume) = local_source_with(&[("photo.raw", b"pixels")]);
    let archive_path = PathBuf::from("/device/DCIM/album.zip");
    let (parent_id, parent) = register_remote_parent(Path::new("/device/DCIM"), true).await;

    let events = Arc::new(CollectorEventSink::new());
    compress_start(
        Arc::clone(&events) as Arc<dyn OperationEventSink>,
        source_volume,
        vec![PathBuf::from("photo.raw")],
        archive_path.clone(),
        parent_id.clone(),
        ConflictResolution::Overwrite,
        0,
        None,
        None,
        crate::operation_log::types::Initiator::User,
    )
    .await
    .expect("start mtp-style remote compress");

    wait_until_async(Duration::from_secs(5), "a terminal event (complete or error)", || {
        !events.complete.lock_ignore_poison().is_empty() || !events.errors.lock_ignore_poison().is_empty()
    })
    .await;
    assert!(
        !events.complete.lock_ignore_poison().is_empty(),
        "mtp-style remote compress should complete, errors: {:?}",
        events.errors.lock_ignore_poison()
    );
    assert_eq!(
        read_remote_entry(parent.as_ref(), &archive_path, "photo.raw")
            .await
            .as_deref(),
        Some(b"pixels".as_slice()),
        "the source must land in the zip on an MTP-style parent"
    );
    let names = sibling_names(parent.as_ref(), &archive_path).await;
    assert!(
        !names.iter().any(|n| n.contains(".cmdr-tmp-")),
        "no upload temp should remain after the delete-then-rename swap, got: {names:?}"
    );

    get_volume_manager().unregister(&parent_id);
}

const CAFE_ZIP_NFC: &str = "caf\u{e9}.zip";
const CAFE_ZIP_NFD: &str = "cafe\u{301}.zip";

/// A registered SMB-shaped parent: byte-exact, composing new names, holding
/// `/share` and, when `existing` names one, a zip with one `stale.txt` entry.
async fn register_share(existing: Option<&str>) -> (String, Arc<InMemoryVolume>) {
    let id = format!("remote-share-{}", Uuid::new_v4());
    let vol = InMemoryVolume::new("Share")
        .with_lane_key(id.clone())
        .with_composed_new_names();
    vol.create_directory(Path::new("/share"))
        .await
        .expect("seed parent dir");
    if let Some(name) = existing {
        vol.create_file(&Path::new("/share").join(name), &zip_bytes(&[("stale.txt", b"old")]))
            .await
            .expect("seed existing zip");
    }
    let vol = Arc::new(vol);
    get_volume_manager().register(&id, Arc::clone(&vol) as Arc<dyn Volume>);
    (id, vol)
}

/// Starts a compress of one local `new.txt` into `/share/<name>` and, when it
/// starts, waits for it to complete.
async fn compress_to_share(parent_id: &str, name: &str) -> Result<(), WriteOperationError> {
    let (_src_tmp, source_volume) = local_source_with(&[("new.txt", b"brand new")]);
    let events = Arc::new(CollectorEventSink::new());
    compress_start(
        Arc::clone(&events) as Arc<dyn OperationEventSink>,
        source_volume,
        vec![PathBuf::from("new.txt")],
        Path::new("/share").join(name),
        parent_id.to_string(),
        ConflictResolution::Overwrite,
        0,
        None,
        None,
        crate::operation_log::types::Initiator::User,
    )
    .await?;
    wait_until_async(Duration::from_secs(5), "a terminal event (complete or error)", || {
        !events.complete.lock_ignore_poison().is_empty() || !events.errors.lock_ignore_poison().is_empty()
    })
    .await;
    assert!(
        !events.complete.lock_ignore_poison().is_empty(),
        "the compress should complete, errors: {:?}",
        events.errors.lock_ignore_poison()
    );
    Ok(())
}

async fn names_in_share(parent: &dyn Volume) -> Vec<String> {
    let mut names = sibling_names(parent, Path::new("/share/x.zip")).await;
    names.sort();
    names
}

#[tokio::test]
async fn a_new_archive_on_a_share_is_named_composed() {
    let (parent_id, parent) = register_share(None).await;

    compress_to_share(&parent_id, CAFE_ZIP_NFD)
        .await
        .expect("start compress");

    assert_eq!(names_in_share(parent.as_ref()).await, vec![CAFE_ZIP_NFC.to_string()]);
    let archive = Path::new("/share").join(CAFE_ZIP_NFC);
    assert_eq!(
        read_remote_entry(parent.as_ref(), &archive, "new.txt").await.as_deref(),
        Some(b"brand new".as_slice())
    );
    get_volume_manager().unregister(&parent_id);
}

/// A target the share holds in another spelling is the archive the dialog warned
/// about (`destination_exists` counts a look-alike), so it's replaced IN PLACE:
/// one archive, under the share's own spelling, ❌ never a twin beside it.
#[tokio::test]
async fn a_compress_onto_a_look_alike_of_an_existing_archive_replaces_it_in_place() {
    let (parent_id, parent) = register_share(Some(CAFE_ZIP_NFD)).await;

    compress_to_share(&parent_id, CAFE_ZIP_NFC)
        .await
        .expect("start compress");

    assert_eq!(names_in_share(parent.as_ref()).await, vec![CAFE_ZIP_NFD.to_string()]);
    let archive = Path::new("/share").join(CAFE_ZIP_NFD);
    assert_eq!(
        read_remote_entry(parent.as_ref(), &archive, "new.txt").await.as_deref(),
        Some(b"brand new".as_slice())
    );
    assert_eq!(
        read_remote_entry(parent.as_ref(), &archive, "stale.txt").await,
        None,
        "a fresh archive, never a merge into the old one"
    );
    get_volume_manager().unregister(&parent_id);
}

/// The mirror case: the target composes onto a name the share holds exactly.
/// Same entry, same answer: replaced in place, still one archive.
#[tokio::test]
async fn a_compress_whose_composed_name_the_share_holds_replaces_that_archive() {
    let (parent_id, parent) = register_share(Some(CAFE_ZIP_NFC)).await;

    compress_to_share(&parent_id, CAFE_ZIP_NFD)
        .await
        .expect("start compress");

    assert_eq!(names_in_share(parent.as_ref()).await, vec![CAFE_ZIP_NFC.to_string()]);
    let archive = Path::new("/share").join(CAFE_ZIP_NFC);
    assert_eq!(
        read_remote_entry(parent.as_ref(), &archive, "new.txt").await.as_deref(),
        Some(b"brand new".as_slice())
    );
    get_volume_manager().unregister(&parent_id);
}

/// Two archives fit the target and neither is spelled as asked: which one to
/// replace is a guess, so the compress refuses before anything is seeded.
#[tokio::test]
async fn a_compress_two_stored_archives_fit_is_refused_and_leaves_both_alone() {
    let composed = "\u{e9}l\u{151}.zip";
    let decomposed = "e\u{301}lo\u{30b}.zip";
    let (parent_id, parent) = register_share(Some(composed)).await;
    parent
        .create_file(
            &Path::new("/share").join(decomposed),
            &zip_bytes(&[("stale.txt", b"old")]),
        )
        .await
        .expect("seed the second archive");

    let refused = compress_to_share(&parent_id, "\u{e9}lo\u{30b}.zip").await;

    assert!(
        matches!(refused, Err(WriteOperationError::DestinationExists { .. })),
        "{refused:?}"
    );
    let mut expected = vec![composed.to_string(), decomposed.to_string()];
    expected.sort();
    assert_eq!(names_in_share(parent.as_ref()).await, expected);
    for name in [composed, decomposed] {
        assert_eq!(
            read_remote_entry(parent.as_ref(), &Path::new("/share").join(name), "stale.txt")
                .await
                .as_deref(),
            Some(b"old".as_slice()),
            "{name:?} keeps its bytes"
        );
    }
    get_volume_manager().unregister(&parent_id);
}

/// The exact stored spelling is the name the dialog warned about: it's replaced
/// in place, under its own bytes, as it always was.
#[tokio::test]
async fn a_compress_onto_the_exact_stored_spelling_still_replaces_it_in_place() {
    let (parent_id, parent) = register_share(Some(CAFE_ZIP_NFD)).await;

    compress_to_share(&parent_id, CAFE_ZIP_NFD)
        .await
        .expect("start compress");

    assert_eq!(names_in_share(parent.as_ref()).await, vec![CAFE_ZIP_NFD.to_string()]);
    let archive = Path::new("/share").join(CAFE_ZIP_NFD);
    assert_eq!(
        read_remote_entry(parent.as_ref(), &archive, "new.txt").await.as_deref(),
        Some(b"brand new".as_slice())
    );
    get_volume_manager().unregister(&parent_id);
}
