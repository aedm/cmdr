//! What the closing flush makes durable, and what it answers when it can't.
//!
//! Driven through `flush_created_destinations` over a real scratch tree, with
//! `test_hook` recording (and, where a test needs it, failing) every directory
//! sync. A move deletes its sources only on `Ok`, so these pin which answers keep
//! a user's originals.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::test_hook;
use super::{FlushFailure, flush_created_destinations};
use crate::file_system::write_operations::event_sinks::CollectorEventSink;
use crate::file_system::write_operations::state::WriteOperationState;
use crate::file_system::write_operations::types::WriteOperationType;
use crate::test_support::TestDir;

/// A landed tree the way a transfer leaves it: `dest/top.txt`, 20 files in
/// `dest/tree/sub/`, and `dest/only/inner/`, an empty directory whose parent
/// `only` holds nothing but it.
struct Landed {
    _dir: TestDir,
    dest: PathBuf,
    files: Vec<PathBuf>,
    dirs: Vec<PathBuf>,
}

fn landed_tree() -> Landed {
    let dir = TestDir::new("durability");
    let dest = dir.join("dest");
    let tree = dest.join("tree");
    let sub = tree.join("sub");
    let only = dest.join("only");
    let inner = only.join("inner");
    fs::create_dir_all(&sub).expect("create dest/tree/sub");
    fs::create_dir_all(&inner).expect("create dest/only/inner");
    let top = dest.join("top.txt");
    fs::write(&top, b"top").expect("write top.txt");
    let mut files = vec![top];
    for i in 0..20 {
        let leaf = sub.join(format!("leaf-{i}.txt"));
        fs::write(&leaf, b"leaf").expect("write a leaf");
        files.push(leaf);
    }
    Landed {
        _dir: dir,
        dest,
        files,
        dirs: vec![tree, sub, only, inner],
    }
}

fn flush(files: &[PathBuf], dirs: &[PathBuf], already_synced: &HashSet<PathBuf>) -> Result<(), FlushFailure> {
    let events = CollectorEventSink::new();
    let state = Arc::new(WriteOperationState::new(Duration::from_millis(200)));
    flush_created_destinations(
        &events,
        "op-durability",
        WriteOperationType::Copy,
        &state,
        files.len(),
        files.len(),
        0,
        0,
        files,
        dirs,
        already_synced,
    )
}

fn flush_landed(landed: &Landed) -> Result<(), FlushFailure> {
    flush(&landed.files, &landed.dirs, &landed.files.iter().cloned().collect())
}

/// The directories that gained an entry in [`landed_tree`]: the parent of every
/// file and the parent of every created directory.
fn dirs_that_gained_entries(dest: &Path) -> Vec<PathBuf> {
    sorted(vec![
        dest.to_path_buf(),
        dest.join("tree"),
        dest.join("tree").join("sub"),
        dest.join("only"),
    ])
}

fn sorted(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.sort();
    paths
}

fn fail_at(target: PathBuf, errno: i32) -> impl FnMut(&Path) -> Option<io::Result<()>> {
    move |dir| (dir == target).then(|| Err(io::Error::from_raw_os_error(errno)))
}

/// A chunked copy already `sync_data`d every file, but that says nothing about
/// the directory entries naming them. Every directory that gained an entry is
/// fsynced, each exactly once however many files it holds, and an empty leaf
/// directory (`inner`) costs nothing of its own: its parent's fsync covers it.
#[test]
fn every_directory_that_gained_an_entry_is_fsynced_once_even_when_file_data_was_already_synced() {
    let landed = landed_tree();
    let log = test_hook::record(|_| None);

    let result = flush_landed(&landed);

    assert_eq!(result, Ok(()));
    assert_eq!(
        sorted(log.dirs()),
        dirs_that_gained_entries(&landed.dest),
        "one fsync per directory that gained an entry, whatever `already_synced` says"
    );
}

/// A directory fsync that fails for a real reason means the entry may not be on
/// disk, so the flush answers the path and errno a caller needs to keep the
/// sources. It still tries every other directory first: a copy keeps going on a
/// failure and should end as durable as it can.
#[test]
fn a_directory_fsync_failing_with_eio_answers_that_directory_and_errno() {
    let landed = landed_tree();
    let failing = landed.dest.join("tree").join("sub");
    let log = test_hook::record(fail_at(failing.clone(), libc::EIO));

    let result = flush_landed(&landed);

    assert_eq!(
        result,
        Err(FlushFailure {
            path: failing,
            errno: Some(libc::EIO),
        })
    );
    assert_eq!(
        sorted(log.dirs()),
        dirs_that_gained_entries(&landed.dest),
        "the failure doesn't stop the other directories from being flushed"
    );
}

/// Some filesystems refuse to fsync a directory at all. Blocking on that would
/// stop every move there from ever deleting its sources, so `ENOTSUP` and
/// `EINVAL` count as done (with a `warn`), and the rest of the flush still runs.
#[test]
fn a_directory_refusing_fsync_with_enotsup_or_einval_still_answers_ok() {
    for errno in [libc::ENOTSUP, libc::EINVAL] {
        let landed = landed_tree();
        let log = test_hook::record(fail_at(landed.dest.join("tree"), errno));

        let result = flush_landed(&landed);

        assert_eq!(result, Ok(()), "errno {errno} is a refusal, not a failure");
        assert_eq!(
            sorted(log.dirs()),
            dirs_that_gained_entries(&landed.dest),
            "errno {errno}: every directory is still attempted"
        );
    }
}

/// A created file whose data the strategy didn't sync, and that the flush can't
/// reach, has no proof of being on disk: that's an `Err`, never a skipped warn.
#[test]
fn a_created_file_the_flush_cant_reach_answers_its_path() {
    let landed = landed_tree();
    let _log = test_hook::record(|_| None);
    let vanished = landed.dest.join("vanished.txt");
    let mut files = landed.files.clone();
    files.push(vanished.clone());

    let result = flush(&files, &landed.dirs, &HashSet::new());

    assert_eq!(
        result,
        Err(FlushFailure {
            path: vanished,
            errno: Some(libc::ENOENT),
        })
    );
}

/// A symlink has no data of its own to sync, but it's still an entry in its
/// directory, so that directory is flushed.
#[test]
fn a_created_symlink_skips_the_data_sync_but_flushes_its_directory() {
    let dir = TestDir::new("durability-symlink");
    let links = dir.join("links");
    fs::create_dir_all(&links).unwrap();
    let link = links.join("dangling");
    std::os::unix::fs::symlink(dir.join("nowhere"), &link).unwrap();
    let log = test_hook::record(|_| None);

    let result = flush(std::slice::from_ref(&link), &[], &HashSet::new());

    assert_eq!(result, Ok(()));
    assert_eq!(log.dirs(), vec![links]);
}
