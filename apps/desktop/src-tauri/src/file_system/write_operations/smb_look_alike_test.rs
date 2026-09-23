//! A name the share already holds under another Unicode spelling, against a real
//! SMB server (require Docker SMB containers).
//!
//! The fixture's Samba stores a name's bytes exactly as sent and matches them
//! exactly, so `café` composed (NFC) and `café` decomposed (NFD, what macOS hands
//! out for many local files) are two entries to it. A copy that asks "is `café`
//! taken?" in the source's spelling hears "no" and writes a second, identical-
//! looking entry beside the user's. These cells pin the guard: such a name is a
//! conflict like any other, an Overwrite or a merge lands on the entry that is
//! there (so the share ends with ONE), and a genuinely new name goes out composed
//! (`Volume::composes_new_names`). Unit twins: `transfer/volume/look_alike_tests.rs`.
//!
//! Case-only look-alikes aren't pinned here: the fixture runs `case sensitive =
//! auto`, so Samba folds case itself and the twin can't be made.

use super::smb_test_support::*;
use crate::file_system::write_operations::{
    CollectorEventSink, ConflictResolution, VolumeCopyConfig, WriteOperationState, copy_volumes_with_progress,
};

const CAFE_NFC: &str = "caf\u{e9}.txt";
const CAFE_NFD: &str = "cafe\u{301}.txt";
const FOTOK_NFC: &str = "fot\u{f3}k";
const FOTOK_NFD: &str = "foto\u{301}k";

/// A local source volume rooted at a fresh temp dir. APFS keeps the spelling a
/// name was created with, so a decomposed name written here lists decomposed.
fn local_source(dir: &tempfile::TempDir) -> Arc<dyn Volume> {
    Arc::new(crate::file_system::volume::LocalPosixVolume::new(
        "src",
        dir.path().to_path_buf(),
    ))
}

/// Copies `sources` (relative to the local root) into `dest_dir` on the share
/// under `policy`, and returns the state for its skip counters.
async fn copy_to_share(
    label: &str,
    source: &Arc<dyn Volume>,
    sources: &[&str],
    dest: &Arc<dyn Volume>,
    dest_dir: &str,
    policy: ConflictResolution,
) -> Arc<WriteOperationState> {
    let state = Arc::new(WriteOperationState::new(Duration::from_millis(200)));
    let events = Arc::new(CollectorEventSink::new());
    let config = VolumeCopyConfig {
        conflict_resolution: policy,
        ..VolumeCopyConfig::default()
    };
    let paths: Vec<PathBuf> = sources.iter().map(PathBuf::from).collect();
    let result = copy_volumes_with_progress(
        events,
        label,
        &state,
        Arc::clone(source),
        &paths,
        Arc::clone(dest),
        Path::new(dest_dir),
        &config,
    )
    .await;
    assert!(result.is_ok(), "{label}: the copy should finish, got {result:?}");
    state
}

/// The names the share holds in `dir`, byte for byte.
async fn names_in(vol: &Arc<dyn Volume>, dir: &str) -> Vec<String> {
    let mut names: Vec<String> = vol
        .list_directory(Path::new(dir), None)
        .await
        .unwrap_or_else(|e| panic!("list {dir}: {e:?}"))
        .into_iter()
        .map(|e| e.name)
        .collect();
    names.sort();
    names
}

async fn read_smb(vol: &Arc<dyn Volume>, path: &str) -> Vec<u8> {
    let mut s = vol
        .open_read_stream(Path::new(path))
        .await
        .unwrap_or_else(|e| panic!("open {path}: {e:?}"));
    let mut out = Vec::new();
    while let Some(Ok(chunk)) = s.next_chunk().await {
        out.extend_from_slice(&chunk);
    }
    out
}

/// A decomposed file copied onto a share holding its composed twin, under Skip:
/// the user's file keeps its bytes, the copy is reported as skipped, and no
/// second `café.txt` appears.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_a_decomposed_file_onto_its_composed_twin_is_skipped_under_skip() {
    let smb = Arc::new(make_docker_volume().await);
    let base = test_dir_name();
    ensure_clean(&smb, &base).await;
    let dest: Arc<dyn Volume> = smb.clone();
    smb.create_directory(Path::new(&base)).await.unwrap();
    smb.create_file(Path::new(&format!("{base}/{CAFE_NFC}")), b"DEST")
        .await
        .unwrap();

    let local = tempfile::TempDir::new().expect("create TempDir");
    std::fs::write(local.path().join(CAFE_NFD), b"SOURCE").unwrap();
    let source = local_source(&local);

    let state = copy_to_share(
        "op-look-alike-skip",
        &source,
        &[CAFE_NFD],
        &dest,
        &base,
        ConflictResolution::Skip,
    )
    .await;

    assert_eq!(
        names_in(&dest, &base).await,
        vec![CAFE_NFC.to_string()],
        "the share must still hold exactly the user's own spelling"
    );
    assert_eq!(read_smb(&dest, &format!("{base}/{CAFE_NFC}")).await, b"DEST");
    assert_eq!(
        state.skipped_totals().0,
        1,
        "the look-alike has to be reported as skipped"
    );

    ensure_clean(&smb, &base).await;
}

/// A decomposed file deep in a merged folder, under Overwrite: the source's bytes
/// replace the user's file IN PLACE, under the spelling the share already had, so
/// the folder ends with one `café.txt`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_overwriting_a_composed_twin_leaves_one_entry() {
    let smb = Arc::new(make_docker_volume().await);
    let base = test_dir_name();
    ensure_clean(&smb, &base).await;
    let dest: Arc<dyn Volume> = smb.clone();
    let album = format!("{base}/album");
    smb.create_directory(Path::new(&base)).await.unwrap();
    smb.create_directory(Path::new(&album)).await.unwrap();
    smb.create_file(Path::new(&format!("{album}/{CAFE_NFC}")), b"DEST")
        .await
        .unwrap();

    let local = tempfile::TempDir::new().expect("create TempDir");
    std::fs::create_dir(local.path().join("album")).unwrap();
    std::fs::write(local.path().join("album").join(CAFE_NFD), b"SOURCE").unwrap();
    let source = local_source(&local);

    copy_to_share(
        "op-look-alike-overwrite",
        &source,
        &["album"],
        &dest,
        &base,
        ConflictResolution::Overwrite,
    )
    .await;

    assert_eq!(
        names_in(&dest, &album).await,
        vec![CAFE_NFC.to_string()],
        "an Overwrite must replace the entry that's there, not stand a second one beside it"
    );
    assert_eq!(read_smb(&dest, &format!("{album}/{CAFE_NFC}")).await, b"SOURCE");

    ensure_clean(&smb, &base).await;
}

/// A decomposed FOLDER copied onto its composed twin merges into the folder the
/// share has: the user's file inside survives, the source's file joins it, and
/// no second `fotók` appears.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_a_decomposed_folder_merges_into_its_composed_twin() {
    let smb = Arc::new(make_docker_volume().await);
    let base = test_dir_name();
    ensure_clean(&smb, &base).await;
    let dest: Arc<dyn Volume> = smb.clone();
    let fotok = format!("{base}/{FOTOK_NFC}");
    smb.create_directory(Path::new(&base)).await.unwrap();
    smb.create_directory(Path::new(&fotok)).await.unwrap();
    smb.create_file(Path::new(&format!("{fotok}/keep.txt")), b"DEST")
        .await
        .unwrap();

    let local = tempfile::TempDir::new().expect("create TempDir");
    std::fs::create_dir(local.path().join(FOTOK_NFD)).unwrap();
    std::fs::write(local.path().join(FOTOK_NFD).join("new.txt"), b"SOURCE").unwrap();
    let source = local_source(&local);

    copy_to_share(
        "op-look-alike-merge",
        &source,
        &[FOTOK_NFD],
        &dest,
        &base,
        ConflictResolution::Stop,
    )
    .await;

    assert_eq!(
        names_in(&dest, &base).await,
        vec![FOTOK_NFC.to_string()],
        "the folder must merge into the one the share has"
    );
    assert_eq!(
        names_in(&dest, &fotok).await,
        vec!["keep.txt".to_string(), "new.txt".to_string()],
        "the user's file survives and the source's file joins it"
    );

    ensure_clean(&smb, &base).await;
}

/// Names the copy CREATES go out composed, at every depth: what Finder over the
/// kernel mount, Windows, and Linux clients all expect to open.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_a_new_name_lands_on_the_share_composed() {
    let smb = Arc::new(make_docker_volume().await);
    let base = test_dir_name();
    ensure_clean(&smb, &base).await;
    let dest: Arc<dyn Volume> = smb.clone();
    smb.create_directory(Path::new(&base)).await.unwrap();

    let local = tempfile::TempDir::new().expect("create TempDir");
    std::fs::create_dir(local.path().join(FOTOK_NFD)).unwrap();
    std::fs::write(local.path().join(FOTOK_NFD).join(CAFE_NFD), b"SOURCE").unwrap();
    std::fs::write(local.path().join(CAFE_NFD), b"TOP").unwrap();
    let source = local_source(&local);

    copy_to_share(
        "op-new-name-composed",
        &source,
        &[FOTOK_NFD, CAFE_NFD],
        &dest,
        &base,
        ConflictResolution::Stop,
    )
    .await;

    let mut expected = vec![CAFE_NFC.to_string(), FOTOK_NFC.to_string()];
    expected.sort();
    assert_eq!(names_in(&dest, &base).await, expected, "top-level names land composed");
    assert_eq!(
        names_in(&dest, &format!("{base}/{FOTOK_NFC}")).await,
        vec![CAFE_NFC.to_string()],
        "and so do the names inside a folder the copy created"
    );
    assert_eq!(
        read_smb(&dest, &format!("{base}/{FOTOK_NFC}/{CAFE_NFC}")).await,
        b"SOURCE"
    );

    ensure_clean(&smb, &base).await;
}

/// A same-share MOVE of a decomposed file, deep in a merged folder and at the
/// top level, onto folders holding the composed twin, under Skip: nothing lands
/// beside the user's file, and both sources stay where they were (a skipped
/// move keeps its source).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires Docker SMB containers (./apps/desktop/test/smb-servers/start.sh)"]
async fn smb_integration_a_same_share_move_skips_a_look_alike_instead_of_landing_beside_it() {
    use crate::file_system::write_operations::move_within_same_volume_with_progress;

    let smb = Arc::new(make_docker_volume().await);
    let base = test_dir_name();
    ensure_clean(&smb, &base).await;
    let vol: Arc<dyn Volume> = smb.clone();
    for dir in ["", "/src", "/src/album", "/dst", "/dst/album"] {
        smb.create_directory(Path::new(&format!("{base}{dir}"))).await.unwrap();
    }
    smb.create_file(Path::new(&format!("{base}/src/album/{CAFE_NFD}")), b"DEEP-SOURCE")
        .await
        .unwrap();
    smb.create_file(Path::new(&format!("{base}/src/{CAFE_NFD}")), b"TOP-SOURCE")
        .await
        .unwrap();
    smb.create_file(Path::new(&format!("{base}/dst/album/{CAFE_NFC}")), b"DEEP-DEST")
        .await
        .unwrap();
    smb.create_file(Path::new(&format!("{base}/dst/{CAFE_NFC}")), b"TOP-DEST")
        .await
        .unwrap();

    let state = Arc::new(WriteOperationState::new(Duration::from_millis(200)));
    let config = VolumeCopyConfig {
        conflict_resolution: ConflictResolution::Skip,
        ..VolumeCopyConfig::default()
    };
    let result = move_within_same_volume_with_progress(
        Arc::new(CollectorEventSink::new()),
        "op-look-alike-same-share-move",
        &state,
        Arc::clone(&vol),
        &[
            PathBuf::from(format!("{base}/src/album")),
            PathBuf::from(format!("{base}/src/{CAFE_NFD}")),
        ],
        Path::new(&format!("{base}/dst")),
        &config,
    )
    .await;
    assert!(result.is_ok(), "the move should finish: {result:?}");

    let mut top = vec!["album".to_string(), CAFE_NFC.to_string()];
    top.sort();
    assert_eq!(
        names_in(&vol, &format!("{base}/dst")).await,
        top,
        "nothing lands beside the top-level twin"
    );
    assert_eq!(
        names_in(&vol, &format!("{base}/dst/album")).await,
        vec![CAFE_NFC.to_string()],
        "nor beside the deep one"
    );
    assert_eq!(
        read_smb(&vol, &format!("{base}/dst/album/{CAFE_NFC}")).await,
        b"DEEP-DEST"
    );
    assert_eq!(
        read_smb(&vol, &format!("{base}/src/album/{CAFE_NFD}")).await,
        b"DEEP-SOURCE"
    );
    assert_eq!(read_smb(&vol, &format!("{base}/src/{CAFE_NFD}")).await, b"TOP-SOURCE");

    ensure_clean(&smb, &base).await;
}
