//! What a cross-FS move makes durable before it deletes a source, and what it
//! keeps when it can't.
//!
//! A Mac → USB move copies chunked, so every file's data is already synced when
//! the closing flush runs; what's left is the directory entries, and FAT/exFAT
//! keep no journal to replay a lost one. These drive `move_with_staging` with the
//! `durability::test_hook` recorder, which sees every directory the flush syncs
//! and can fail one with a chosen errno.

use std::io;

use super::cross_fs::move_with_staging;
use super::test_support::make_state;
use super::*;
use crate::file_system::write_operations::durability::test_hook;
use crate::file_system::write_operations::event_sinks::CollectorEventSink;
use crate::file_system::write_operations::types::ConflictResolution;
use crate::ignore_poison::IgnorePoison;
use crate::test_support::TestDir;

/// A move of `top.txt`, `tree/` (five files in `tree/sub/`), and `only/` (holding
/// nothing but the empty `only/inner/`) from `src` into an empty `dst`.
struct NestedMove {
    _dir: TestDir,
    src: PathBuf,
    dst: PathBuf,
    sources: Vec<PathBuf>,
}

fn nested_move() -> NestedMove {
    let dir = TestDir::new("move-durability");
    let src = dir.join("src");
    let dst = dir.join("dst");
    fs::create_dir_all(src.join("tree").join("sub")).expect("create src/tree/sub");
    fs::create_dir_all(src.join("only").join("inner")).expect("create src/only/inner");
    fs::create_dir_all(&dst).expect("create dst");
    fs::write(src.join("top.txt"), b"top").expect("write top.txt");
    for i in 0..5 {
        fs::write(src.join("tree").join("sub").join(format!("leaf-{i}.txt")), b"leaf").expect("write a leaf");
    }
    let sources = vec![src.join("top.txt"), src.join("tree"), src.join("only")];
    NestedMove {
        _dir: dir,
        src,
        dst,
        sources,
    }
}

/// Every final directory in [`nested_move`] that gained an entry: `dst` (the
/// three top-level landings), `tree` (`sub`), `tree/sub` (the files), and `only`
/// (`inner`). Never a staging path, and never `inner`, which gained nothing.
fn final_dirs_that_gained_entries(dst: &Path) -> Vec<PathBuf> {
    sorted(vec![
        dst.to_path_buf(),
        dst.join("tree"),
        dst.join("tree").join("sub"),
        dst.join("only"),
    ])
}

fn source_files(src: &Path) -> Vec<PathBuf> {
    let mut files = vec![src.join("top.txt")];
    files.extend((0..5).map(|i| src.join("tree").join("sub").join(format!("leaf-{i}.txt"))));
    files
}

fn sorted(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.sort();
    paths
}

fn fail_at(target: PathBuf, errno: i32) -> impl FnMut(&Path) -> Option<io::Result<()>> {
    move |dir| (dir == target).then(|| Err(io::Error::from_raw_os_error(errno)))
}

fn run(
    sources: &[PathBuf],
    dst: &Path,
    resolution: ConflictResolution,
    op_id: &str,
) -> (Arc<CollectorEventSink>, Result<(), WriteOperationError>) {
    let events = Arc::new(CollectorEventSink::new());
    let state = make_state(200);
    let config = WriteOperationConfig {
        conflict_resolution: resolution,
        ..WriteOperationConfig::default()
    };
    let result = move_with_staging(&*events, op_id, &state, sources, dst, &config, 0);
    (events, result)
}

/// The shape of a flush that fsynced where entries actually landed: every
/// synced path is a real directory now, none sits under the staging folder.
fn assert_synced_only_real_final_directories(synced: &[PathBuf]) {
    for dir in synced {
        assert!(
            is_real_directory(dir),
            "{} was fsynced but isn't a directory",
            dir.display()
        );
        assert!(
            !dir.to_string_lossy().contains(".cmdr-staging-"),
            "{} is a staging path; its entries have moved on",
            dir.display()
        );
    }
}

/// The fix's core: a chunked move's files are all in `already_synced`, and that
/// used to skip their parent-directory fsync too, so a Mac → USB move fsynced no
/// directory before deleting the originals. Now every final directory that
/// gained an entry, including one holding only a subdirectory, is fsynced once.
#[test]
fn a_move_fsyncs_every_final_directory_that_gained_an_entry_once_before_deleting_sources() {
    let fixture = nested_move();
    let log = test_hook::record(|_| None);

    let (_events, result) = run(
        &fixture.sources,
        &fixture.dst,
        ConflictResolution::Stop,
        "op-move-durability-dirs",
    );

    assert!(result.is_ok(), "expected Ok, got {result:?}");
    assert_eq!(sorted(log.dirs()), final_dirs_that_gained_entries(&fixture.dst));
    for file in source_files(&fixture.src) {
        assert!(!file.exists(), "{} should be moved", file.display());
    }
}

/// A flush that can't prove a directory durable must leave the move undone: every
/// source stays, everything that landed stays, no source is reported removed, and
/// the move answers `IoError` naming the directory.
#[test]
fn a_directory_fsync_failing_with_eio_keeps_every_source_and_what_landed() {
    let fixture = nested_move();
    let failing = fixture.dst.join("tree").join("sub");
    let _log = test_hook::record(fail_at(failing.clone(), libc::EIO));

    let (events, result) = run(
        &fixture.sources,
        &fixture.dst,
        ConflictResolution::Stop,
        "op-move-durability-eio",
    );

    match &result {
        Err(WriteOperationError::IoError { path, .. }) => assert_eq!(path, &failing.display().to_string()),
        other => panic!("expected IoError naming {}, got {other:?}", failing.display()),
    }
    for file in source_files(&fixture.src) {
        assert!(file.exists(), "source {} must survive a failed flush", file.display());
    }
    assert!(
        fixture.src.join("only").join("inner").is_dir(),
        "the empty source folder must survive too"
    );
    assert!(fixture.dst.join("top.txt").exists(), "what landed stays");
    assert!(fixture.dst.join("tree").join("sub").join("leaf-0.txt").exists());
    assert!(
        events.complete.lock_ignore_poison().is_empty(),
        "a failed flush never completes"
    );
    assert_eq!(
        events.errors.lock_ignore_poison().len(),
        1,
        "the failure is emitted once"
    );
    assert!(
        events
            .source_items_done
            .lock_ignore_poison()
            .iter()
            .all(|done| !done.source_removed),
        "no source may be reported removed"
    );
}

/// A destination filesystem that refuses directory fsync (`ENOTSUP`, `EINVAL`)
/// can't be allowed to block every move onto it, so the move completes.
#[test]
fn a_directory_refusing_fsync_with_enotsup_or_einval_still_completes_the_move() {
    for errno in [libc::ENOTSUP, libc::EINVAL] {
        let fixture = nested_move();
        let log = test_hook::record(fail_at(fixture.dst.join("tree"), errno));

        let (events, result) = run(
            &fixture.sources,
            &fixture.dst,
            ConflictResolution::Stop,
            "op-move-durability-refusal",
        );

        assert!(result.is_ok(), "errno {errno}: expected Ok, got {result:?}");
        assert_eq!(
            sorted(log.dirs()),
            final_dirs_that_gained_entries(&fixture.dst),
            "errno {errno}: the flush still reached every directory"
        );
        for file in source_files(&fixture.src) {
            assert!(!file.exists(), "errno {errno}: {} should be moved", file.display());
        }
        assert_eq!(events.complete.lock_ignore_poison().len(), 1);
    }
}

/// A folder Renamed onto a same-named FILE lands at `tree (1)`, so its entries
/// live there, not under `dst/tree` (the user's file). Fsyncing the path the
/// staging prefix alone would suggest hits `ENOTDIR` under the file and would
/// fail a move that landed fine, while the real landing went unflushed.
#[test]
fn a_folder_renamed_onto_a_same_named_file_flushes_where_it_landed() {
    let dir = TestDir::new("move-durability-rename");
    let src = dir.join("src");
    let dst = dir.join("dst");
    fs::create_dir_all(src.join("tree").join("sub")).unwrap();
    fs::write(src.join("tree").join("sub").join("leaf.txt"), b"leaf").unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(dst.join("tree"), b"the user's file").unwrap();
    let log = test_hook::record(|_| None);

    let (_events, result) = run(
        &[src.join("tree")],
        &dst,
        ConflictResolution::Rename,
        "op-move-durability-rename",
    );

    assert!(result.is_ok(), "expected Ok, got {result:?}");
    let landed = dst.join("tree (1)");
    assert!(
        landed.join("sub").join("leaf.txt").is_file(),
        "the folder landed at `tree (1)`"
    );
    let synced = log.dirs();
    assert_synced_only_real_final_directories(&synced);
    assert!(synced.contains(&landed), "the landing's own entry (`sub`) is flushed");
    assert!(synced.contains(&landed.join("sub")), "the leaf's entry is flushed");
    assert!(!src.join("tree").exists(), "the source moved");
}

/// Inside a merge, a child folder Renamed onto a same-named file lands at
/// `sub (1)`; the flush follows it there.
#[test]
fn a_merged_child_renamed_onto_a_same_named_file_flushes_where_it_landed() {
    let dir = TestDir::new("move-durability-merge-rename");
    let src = dir.join("src");
    let dst = dir.join("dst");
    fs::create_dir_all(src.join("tree").join("sub").join("deep")).unwrap();
    fs::write(src.join("tree").join("sub").join("deep").join("leaf.txt"), b"leaf").unwrap();
    fs::create_dir_all(dst.join("tree")).unwrap();
    fs::write(dst.join("tree").join("sub"), b"the user's file").unwrap();
    let log = test_hook::record(|_| None);

    let (_events, result) = run(
        &[src.join("tree")],
        &dst,
        ConflictResolution::Rename,
        "op-move-durability-merge-rename",
    );

    assert!(result.is_ok(), "expected Ok, got {result:?}");
    let landed = dst.join("tree").join("sub (1)");
    assert!(
        landed.join("deep").join("leaf.txt").is_file(),
        "the child landed at `sub (1)`"
    );
    let synced = log.dirs();
    assert_synced_only_real_final_directories(&synced);
    assert!(synced.contains(&dst.join("tree")), "the merged folder gained `sub (1)`");
    assert!(synced.contains(&landed), "the landing's own entry (`deep`) is flushed");
    assert!(synced.contains(&landed.join("deep")), "the leaf's entry is flushed");
}

/// A folder Skipped against a same-named file never lands, so nothing under its
/// staged paths is a directory to flush; the rest of the move still completes.
#[test]
fn a_folder_skipped_against_a_same_named_file_flushes_only_what_landed() {
    let dir = TestDir::new("move-durability-skip");
    let src = dir.join("src");
    let dst = dir.join("dst");
    fs::create_dir_all(src.join("tree").join("sub")).unwrap();
    fs::write(src.join("tree").join("sub").join("leaf.txt"), b"leaf").unwrap();
    fs::write(src.join("other.txt"), b"other").unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(dst.join("tree"), b"the user's file").unwrap();
    let log = test_hook::record(|_| None);

    let (_events, result) = run(
        &[src.join("tree"), src.join("other.txt")],
        &dst,
        ConflictResolution::Skip,
        "op-move-durability-skip",
    );

    assert!(result.is_ok(), "expected Ok, got {result:?}");
    assert_eq!(
        log.dirs(),
        vec![dst.clone()],
        "only `dst` gained an entry (`other.txt`)"
    );
    assert!(
        src.join("tree").join("sub").join("leaf.txt").exists(),
        "the skipped source stays"
    );
    assert!(!src.join("other.txt").exists(), "the landed source moved");
}
