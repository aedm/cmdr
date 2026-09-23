//! Bulk rename on a byte-exact volume that holds a name in another Unicode
//! spelling, or that asks for composed new names.
//!
//! The same rule single rename follows (`look_alike_instant_tests.rs`): a new
//! name goes out the way the volume spells new names, a destination the folder
//! holds under another spelling is taken (the row is skipped, the way an exact
//! clash is), and a row renaming an entry to its own other spelling lands. The
//! batch-only question is the one the rotation adds: a look-alike that a row of
//! the SAME batch moves away is vacated, not taken. Docker twins:
//! `write_operations/backend_suites/smb_look_alike_test.rs`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::super::super::event_sinks::{CollectorEventSink, OperationEventSink};
use super::super::super::source_binding::SourceFingerprint;
use super::{BulkRenameRow, start_bulk_rename};
use crate::file_system::Volume;
use crate::file_system::volume::InMemoryVolume;
use crate::file_system::volume::manager::get_volume_manager;
use crate::ignore_poison::IgnorePoison;
use crate::operation_log::types::Initiator;
use crate::test_support::wait_until_async;

const CAFE_NFC: &str = "caf\u{e9}.txt";
const CAFE_NFD: &str = "cafe\u{301}.txt";
/// Two accents, so three spellings: two stored ones can both differ from the
/// asked one.
const EE_NFC: &str = "\u{e9}\u{e9}.txt";
const EE_NFD: &str = "e\u{301}e\u{301}.txt";
const EE_MIXED: &str = "\u{e9}e\u{301}.txt";

/// A registered in-memory volume holding `/dir`: byte-exact, composing new
/// names when `composes` says so (an SMB share), keeping them as given when not
/// (SFTP, a phone).
async fn volume(label: &str, composes: bool) -> (String, Arc<InMemoryVolume>) {
    static N: AtomicU64 = AtomicU64::new(0);
    let id = format!("bulk-look-alike-{label}-{}", N.fetch_add(1, Ordering::Relaxed));
    let mut volume = InMemoryVolume::new(&id).with_lane_key(id.clone());
    if composes {
        volume = volume.with_composed_new_names();
    }
    let volume = Arc::new(volume);
    volume.create_directory(Path::new("/dir")).await.unwrap();
    get_volume_manager().register(&id, Arc::clone(&volume) as Arc<dyn Volume>);
    (id, volume)
}

async fn file(volume: &InMemoryVolume, name: &str, bytes: &[u8]) {
    volume
        .create_file(Path::new(&format!("/dir/{name}")), bytes)
        .await
        .unwrap();
}

async fn row(volume: &InMemoryVolume, source: &str, destination: &str) -> BulkRenameRow {
    let source = PathBuf::from(format!("/dir/{source}"));
    let expected_fingerprint = SourceFingerprint::capture_remote(volume, &source)
        .await
        .expect("fingerprint fixture source");
    BulkRenameRow {
        row_id: "1".to_string(),
        source,
        destination: PathBuf::from(format!("/dir/{destination}")),
        expected_fingerprint,
    }
}

/// Runs the batch to its end and answers how many rows it skipped.
async fn run(id: &str, rows: Vec<BulkRenameRow>) -> usize {
    let sink = Arc::new(CollectorEventSink::new());
    start_bulk_rename(
        Arc::clone(&sink) as Arc<dyn OperationEventSink>,
        id.to_string(),
        rows,
        Initiator::Agent,
    )
    .expect("start bulk rename");
    wait_until_async(Duration::from_secs(5), "the batch settles", || {
        !sink.settled.lock_ignore_poison().is_empty()
    })
    .await;
    let complete = sink.complete.lock_ignore_poison();
    assert_eq!(complete.len(), 1, "one terminal complete event");
    complete[0].files_skipped
}

async fn names_in(volume: &InMemoryVolume) -> Vec<String> {
    let mut names: Vec<String> = volume
        .list_directory(Path::new("/dir"), None)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    names.sort();
    names
}

async fn read(volume: &InMemoryVolume, name: &str) -> Vec<u8> {
    let mut stream = volume
        .open_read_stream(Path::new(&format!("/dir/{name}")))
        .await
        .unwrap();
    let mut out = Vec::new();
    while let Some(Ok(chunk)) = stream.next_chunk().await {
        out.extend_from_slice(&chunk);
    }
    out
}

fn sorted(names: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = names.iter().map(|name| name.to_string()).collect();
    names.sort();
    names
}

#[tokio::test]
async fn a_new_name_goes_out_composed() {
    let (id, volume) = volume("composed", true).await;
    file(&volume, "a.txt", b"A").await;

    let skipped = run(&id, vec![row(&volume, "a.txt", CAFE_NFD).await]).await;

    assert_eq!(skipped, 0);
    assert_eq!(names_in(&volume).await, vec![CAFE_NFC.to_string()]);
}

#[tokio::test]
async fn a_destination_the_folder_holds_in_another_spelling_is_skipped() {
    let (id, volume) = volume("taken", true).await;
    file(&volume, "a.txt", b"MINE").await;
    file(&volume, CAFE_NFD, b"THEIRS").await;

    let skipped = run(&id, vec![row(&volume, "a.txt", CAFE_NFC).await]).await;

    assert_eq!(skipped, 1, "the look-alike is a clash like an exact one");
    assert_eq!(
        names_in(&volume).await,
        sorted(&["a.txt", CAFE_NFD]),
        "no twin appeared"
    );
    assert_eq!(read(&volume, CAFE_NFD).await, b"THEIRS");
}

/// A volume that keeps new names as given (SFTP, a phone) is just as byte-exact,
/// so the look-alike is just as taken there.
#[tokio::test]
async fn a_look_alike_is_taken_on_a_volume_that_keeps_names_as_given() {
    let (id, volume) = volume("as-given", false).await;
    file(&volume, "a.txt", b"MINE").await;
    file(&volume, CAFE_NFC, b"THEIRS").await;

    let skipped = run(&id, vec![row(&volume, "a.txt", CAFE_NFD).await]).await;

    assert_eq!(skipped, 1);
    assert_eq!(names_in(&volume).await, sorted(&["a.txt", CAFE_NFC]));
}

#[tokio::test]
async fn a_destination_two_stored_spellings_fit_is_skipped_rather_than_guessed() {
    let (id, volume) = volume("ambiguous", true).await;
    file(&volume, "a.txt", b"MINE").await;
    file(&volume, EE_NFD, b"ONE").await;
    file(&volume, EE_MIXED, b"TWO").await;

    let skipped = run(&id, vec![row(&volume, "a.txt", EE_NFC).await]).await;

    assert_eq!(skipped, 1);
    assert_eq!(names_in(&volume).await, sorted(&["a.txt", EE_MIXED, EE_NFD]));
}

/// The one way to fix a name other clients can't open: the look-alike is the
/// row's own source, so it's a respell, never a clash.
#[tokio::test]
async fn renaming_an_entry_to_its_own_other_spelling_lands() {
    let (id, volume) = volume("respell", true).await;
    file(&volume, CAFE_NFD, b"MINE").await;

    let skipped = run(&id, vec![row(&volume, CAFE_NFD, CAFE_NFC).await]).await;

    assert_eq!(skipped, 0);
    assert_eq!(names_in(&volume).await, vec![CAFE_NFC.to_string()]);
    assert_eq!(read(&volume, CAFE_NFC).await, b"MINE");
}

/// A look-alike another row of the batch moves away is vacated, not taken: the
/// swap lands both rows and the folder ends with one `café.txt`.
///
/// macOS only: the batch plans on `normalize_for_comparison`, which folds form on
/// macOS alone. Elsewhere the planner can't see that the two spellings are one
/// name, and the guard refuses the row rather than let it land beside the entry
/// it waits on.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn a_look_alike_the_batch_itself_moves_away_is_not_a_clash() {
    let (id, volume) = volume("swap", true).await;
    file(&volume, CAFE_NFD, b"FIRST").await;
    file(&volume, "b.txt", b"SECOND").await;

    let rows = vec![
        row(&volume, CAFE_NFD, "b.txt").await,
        row(&volume, "b.txt", CAFE_NFC).await,
    ];
    let skipped = run(&id, rows).await;

    assert_eq!(skipped, 0);
    assert_eq!(names_in(&volume).await, sorted(&["b.txt", CAFE_NFC]));
    assert_eq!(read(&volume, "b.txt").await, b"FIRST");
    assert_eq!(read(&volume, CAFE_NFC).await, b"SECOND");
}

/// The chain twin of the swap: the look-alike's own row moves it to a free name
/// first, then the other row takes the name.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn a_look_alike_the_batch_moves_away_first_frees_the_name_for_a_chain() {
    let (id, volume) = volume("chain", true).await;
    file(&volume, CAFE_NFD, b"FIRST").await;
    file(&volume, "y.txt", b"SECOND").await;

    let rows = vec![
        row(&volume, "y.txt", CAFE_NFC).await,
        row(&volume, CAFE_NFD, "x.txt").await,
    ];
    let skipped = run(&id, rows).await;

    assert_eq!(skipped, 0);
    assert_eq!(names_in(&volume).await, sorted(&["x.txt", CAFE_NFC]));
    assert_eq!(read(&volume, CAFE_NFC).await, b"SECOND");
}
