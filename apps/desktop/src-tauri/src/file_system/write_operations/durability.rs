//! Targeted durability, so "complete" means "durable on disk", not "buffered in
//! the OS page cache". Two passes, one per kind of write: an operation that
//! produced BYTES `fdatasync`s each created destination and `fsync`s every
//! directory that gained an entry (`flush_created_destinations`), while one that
//! only moved directory ENTRIES flushes the directories themselves
//! (`flush_touched_directories`). Both announce the `Flushing` phase first. Also
//! holds the drive-index size lookup used to render directory sizes in conflict
//! UI.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::event_sinks::OperationEventSink;
use super::state::WriteOperationState;

/// What a flush couldn't make durable: the path it was syncing and the OS error
/// number (`None` when the failure carried none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FlushFailure {
    pub path: PathBuf,
    pub errno: Option<i32>,
}

impl std::fmt::Display for FlushFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.errno {
            Some(errno) => write!(f, "{}: {}", self.path.display(), io::Error::from_raw_os_error(errno)),
            None => write!(f, "{}: no OS error code", self.path.display()),
        }
    }
}

/// Emits a `Flushing`-phase progress event, then makes a transfer's output
/// durable: `fdatasync`s every created file whose data isn't already on disk,
/// and `fsync`s every directory that gained an entry. Blocks until it's done.
///
/// - **File data.** `already_synced` holds the files the copy strategy already
///   made durable (chunked copy's inline `sync_data`) or for which a flush is
///   moot (APFS clonefile / reflink, which share copy-on-write extents with the
///   source). Their data sync is skipped, so a long chunked batch doesn't pay a
///   second `fcntl(F_FULLFSYNC)` per file. A symlink has no data of its own.
/// - **Directory entries.** A synced file is still only reachable once the entry
///   naming it is on disk, and FAT/exFAT keep no journal to replay a lost one.
///   So the parent of every created file AND of every created directory is
///   fsynced, each distinct directory once, whatever `already_synced` says. A
///   directory that only received a `mkdir` is covered by its parent's fsync.
///
/// Every path is tried even after a failure, so the output ends as durable as it
/// can, and the first failure is the answer. A move deletes its sources only on
/// `Ok`; a copy deletes nothing and only logs an `Err`.
#[allow(
    clippy::too_many_arguments,
    reason = "These are the natural operation-wide values a progress emit needs; bundling them into a struct adds ceremony without cleaning anything up, matching WriteProgressEvent::new."
)]
pub(super) fn flush_created_destinations(
    events: &dyn OperationEventSink,
    operation_id: &str,
    operation_type: super::types::WriteOperationType,
    state: &Arc<WriteOperationState>,
    files_done: usize,
    files_total: usize,
    bytes_done: u64,
    bytes_total: u64,
    created_files: &[PathBuf],
    created_dirs: &[PathBuf],
    already_synced: &HashSet<PathBuf>,
) -> Result<(), FlushFailure> {
    emit_flushing_phase(
        events,
        operation_id,
        operation_type,
        state,
        files_done,
        files_total,
        bytes_done,
        bytes_total,
    );

    let mut first_failure: Option<FlushFailure> = None;
    let mut note_failure = |path: &Path, e: &io::Error| {
        first_failure.get_or_insert_with(|| FlushFailure {
            path: path.to_path_buf(),
            errno: e.raw_os_error(),
        });
    };

    for file in created_files.iter().filter(|file| !already_synced.contains(*file)) {
        if let Err(e) = sync_file_data(file) {
            log::warn!(
                target: "write_durability",
                "flush: couldn't sync the data of {}: {e}",
                file.display()
            );
            note_failure(file, &e);
        }
    }

    for dir in directories_gaining_entries(created_files, created_dirs) {
        match fsync_dir(&dir) {
            Ok(()) => {}
            Err(e) if refuses_directory_fsync(&e) => log::warn!(
                target: "write_durability",
                "flush: {} refuses directory fsync, counting its entries as done: {e}",
                dir.display()
            ),
            Err(e) => {
                log::warn!(
                    target: "write_durability",
                    "flush: couldn't fsync the directory {}: {e}",
                    dir.display()
                );
                note_failure(&dir, &e);
            }
        }
    }

    first_failure.map_or(Ok(()), Err)
}

/// Whether a directory fsync failed because the filesystem doesn't do directory
/// fsync at all (`ENOTSUP`, `EINVAL`). Treating that as a failure would stop
/// every move onto such a filesystem from ever deleting its sources. Any other
/// errno (macOS's distinct `EOPNOTSUPP` and `ENOSYS` included) stays a failure.
fn refuses_directory_fsync(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::ENOTSUP | libc::EINVAL))
}

/// `fdatasync`s one created file. A symlink carries no data of its own, and
/// opening one would follow it, so it's done without opening anything.
fn sync_file_data(file: &Path) -> io::Result<()> {
    if fs::symlink_metadata(file)?.file_type().is_symlink() {
        return Ok(());
    }
    fs::File::open(file)?.sync_data()
}

/// The distinct directories whose entries a transfer added, in first-seen
/// order: the parent of every file and of every directory it created.
fn directories_gaining_entries(created_files: &[PathBuf], created_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen: HashSet<&Path> = HashSet::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for parent in created_files.iter().chain(created_dirs).filter_map(|p| p.parent()) {
        if seen.insert(parent) {
            dirs.push(parent.to_path_buf());
        }
    }
    dirs
}

/// Emits a `Flushing`-phase progress event, then `fsync`s each of `directories`
/// once so the entry changes inside them are durable. Blocks until they're done.
///
/// This is what a `rename(2)`-based move flushes: the rename moved directory
/// ENTRIES, and the moved files' own data blocks and inodes were already durable
/// before it (see `MoveTransaction::touched_directories`, which picks the
/// directories). Syncing the files here would buy nothing and cost one
/// `fcntl(F_FULLFSYNC)` per file on macOS, a device-level barrier that dwarfs
/// this pass — `transfer/DETAILS.md` § Durability carries the measurements.
///
/// Best-effort: nothing is deleted after a same-FS move's flush, so a directory
/// that rejects `fsync` is logged, never propagated.
#[allow(
    clippy::too_many_arguments,
    reason = "The natural operation-wide values a progress emit needs, matching flush_created_destinations."
)]
pub(super) fn flush_touched_directories(
    events: &dyn OperationEventSink,
    operation_id: &str,
    operation_type: super::types::WriteOperationType,
    state: &Arc<WriteOperationState>,
    files_done: usize,
    files_total: usize,
    bytes_done: u64,
    bytes_total: u64,
    directories: &[PathBuf],
) {
    emit_flushing_phase(
        events,
        operation_id,
        operation_type,
        state,
        files_done,
        files_total,
        bytes_done,
        bytes_total,
    );

    for dir in directories {
        if let Err(e) = fsync_dir(dir) {
            log::debug!(
                target: "write_durability",
                "flush: dir fsync skipped for {}: {e}",
                dir.display()
            );
        }
    }
}

/// Announces the closing flush so the FE can show "Writing the last piece…"
/// instead of a bar frozen at 100% on slow media.
#[allow(
    clippy::too_many_arguments,
    reason = "The natural operation-wide values a progress emit needs, matching WriteProgressEvent::new."
)]
fn emit_flushing_phase(
    events: &dyn OperationEventSink,
    operation_id: &str,
    operation_type: super::types::WriteOperationType,
    state: &Arc<WriteOperationState>,
    files_done: usize,
    files_total: usize,
    bytes_done: u64,
    bytes_total: u64,
) {
    use super::types::{WriteOperationPhase, WriteProgressEvent};

    state.emit_progress_via_sink(
        events,
        WriteProgressEvent::new(
            operation_id.to_string(),
            operation_type,
            WriteOperationPhase::Flushing,
            None,
            files_done,
            files_total,
            bytes_done,
            bytes_total,
        ),
    );
}

/// Opens a directory and `fsync`s it so the entry changes inside it are durable.
fn fsync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(test)]
    if let Some(answer) = test_hook::intercept(dir) {
        return answer;
    }
    let f = fs::File::open(dir)?;
    f.sync_all()
}

/// Looks up a directory's recursive size from the drive index. Returns `None`
/// when the index doesn't cover the path (network mount, MTP, outside-scope
/// path, indexer not yet initialised). The BE intentionally never *walks*
/// the tree to compute this — `(unknown)` on the FE is the legitimate
/// fallback when the cached value isn't available.
pub(super) fn lookup_indexed_size(path: &Path) -> Option<u64> {
    crate::index_host::index()
        .dir_stats(&path.to_string_lossy())
        .ok()
        .flatten()
        .map(|s| s.recursive_size)
}

/// Test seam: records every directory `fsync_dir` is asked to sync on this
/// thread, and optionally answers for it, so a test can see which directories a
/// flush reached and make one fail with a chosen errno. Production never installs
/// a hook. Thread-local, so it only sees engines the test drives on its own
/// thread, which is how every local-move and local-copy test calls them.
#[cfg(test)]
pub(super) mod test_hook {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    type Answer = Box<dyn FnMut(&Path) -> Option<std::io::Result<()>>>;

    struct Hook {
        dirs: Rc<RefCell<Vec<PathBuf>>>,
        answer: Answer,
    }

    thread_local! {
        static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    /// The directories a flush asked to `fsync` while this log was installed.
    /// Uninstalls the hook on drop.
    pub(crate) struct DirSyncLog {
        dirs: Rc<RefCell<Vec<PathBuf>>>,
    }

    impl DirSyncLog {
        /// Every directory `fsync_dir` was called with, in call order,
        /// duplicates included.
        pub(crate) fn dirs(&self) -> Vec<PathBuf> {
            self.dirs.borrow().clone()
        }
    }

    impl Drop for DirSyncLog {
        fn drop(&mut self) {
            HOOK.with(|h| *h.borrow_mut() = None);
        }
    }

    /// Records every directory sync on this thread. `answer` decides each one:
    /// `None` runs the real `fsync`, `Some(result)` returns `result` instead.
    pub(crate) fn record(answer: impl FnMut(&Path) -> Option<std::io::Result<()>> + 'static) -> DirSyncLog {
        let dirs = Rc::new(RefCell::new(Vec::new()));
        HOOK.with(|h| {
            *h.borrow_mut() = Some(Hook {
                dirs: Rc::clone(&dirs),
                answer: Box::new(answer),
            });
        });
        DirSyncLog { dirs }
    }

    pub(super) fn intercept(dir: &Path) -> Option<std::io::Result<()>> {
        HOOK.with(|h| {
            h.borrow_mut().as_mut().and_then(|hook| {
                hook.dirs.borrow_mut().push(dir.to_path_buf());
                (hook.answer)(dir)
            })
        })
    }
}

#[cfg(test)]
#[path = "durability_tests.rs"]
mod tests;
