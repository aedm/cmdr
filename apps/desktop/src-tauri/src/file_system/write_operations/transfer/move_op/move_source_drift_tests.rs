//! What the cross-filesystem move's source sweep does with an original that
//! changed after its copy.
//!
//! Phase 2 copies every file, Phase 4 deletes the originals, and between them
//! lie the rest of the copy, the flush, and however long a big tree takes. An app
//! that saves into the source in that window leaves the NEW bytes only in the
//! source: the destination holds what the copy read. So the sweep checks each
//! original against what it looked like when its copy started, and one that no
//! longer matches stays where it is.
//!
//! The window is reproduced by a sink that rewrites a source file on the sweep's
//! opening tick, which the engine emits after the flush and before its first
//! delete.

use std::sync::Mutex;

use super::cross_fs::move_with_staging;
use super::test_support::{make_state, outcomes_for, removal_flags_for};
use super::*;
use crate::file_system::write_operations::event_sinks::CollectorEventSink;
use crate::file_system::write_operations::types::{
    ConflictInfo, DryRunResult, ScanProgressEvent, WriteCancelledEvent, WriteCompleteEvent, WriteConflictEvent,
    WriteConflictResolvedEvent, WriteErrorEvent, WriteOperationPhase, WriteProgressEvent, WriteSettledEvent,
};
use crate::ignore_poison::IgnorePoison;

/// Collects every event, and runs `before_sweep` once, on the first
/// `Deleting`-phase tick: the moment the copy is flushed and nothing has been
/// deleted yet.
struct EditBeforeSweep {
    inner: CollectorEventSink,
    before_sweep: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl EditBeforeSweep {
    fn new(before_sweep: impl FnOnce() + Send + 'static) -> Self {
        Self {
            inner: CollectorEventSink::new(),
            before_sweep: Mutex::new(Some(Box::new(before_sweep))),
        }
    }
}

impl OperationEventSink for EditBeforeSweep {
    fn emit_progress(&self, event: WriteProgressEvent) {
        if event.phase == WriteOperationPhase::Deleting
            && let Some(edit) = self.before_sweep.lock_ignore_poison().take()
        {
            edit();
        }
        self.inner.emit_progress(event);
    }
    fn emit_complete(&self, event: WriteCompleteEvent) {
        self.inner.emit_complete(event);
    }
    fn emit_cancelled(&self, event: WriteCancelledEvent) {
        self.inner.emit_cancelled(event);
    }
    fn emit_error(&self, event: WriteErrorEvent) {
        self.inner.emit_error(event);
    }
    fn emit_conflict(&self, event: WriteConflictEvent) {
        self.inner.emit_conflict(event);
    }
    fn emit_conflict_resolved(&self, event: WriteConflictResolvedEvent) {
        self.inner.emit_conflict_resolved(event);
    }
    fn emit_source_item_done(&self, event: WriteSourceItemDoneEvent) {
        self.inner.emit_source_item_done(event);
    }
    fn emit_scan_progress(&self, event: ScanProgressEvent) {
        self.inner.emit_scan_progress(event);
    }
    fn emit_scan_conflict(&self, conflict: ConflictInfo) {
        self.inner.emit_scan_conflict(conflict);
    }
    fn emit_dry_run_complete(&self, result: DryRunResult) {
        self.inner.emit_dry_run_complete(result);
    }
    fn emit_settled(&self, event: WriteSettledEvent) {
        self.inner.emit_settled(event);
    }
}

/// A cross-FS move of `source` into `dst_dir` that runs `before_sweep` between
/// the copy and the source sweep.
fn move_editing_before_sweep(
    source: &Path,
    dst_dir: &Path,
    op_id: &str,
    before_sweep: impl FnOnce() + Send + 'static,
) -> EditBeforeSweep {
    let events = EditBeforeSweep::new(before_sweep);
    let state = make_state(0);
    let result = move_with_staging(
        &events,
        op_id,
        &state,
        std::slice::from_ref(&source.to_path_buf()),
        dst_dir,
        &WriteOperationConfig::default(),
        0,
    );
    assert!(result.is_ok(), "the move must succeed: {result:?}");
    assert!(
        events.before_sweep.lock_ignore_poison().is_none(),
        "precondition: the edit ran before the sweep"
    );
    events
}

/// Rewrites `path` with `bytes` and moves its mtime on, so the rewrite is visible
/// on a filesystem that stores coarse timestamps too. A same-size rewrite is the
/// hard case: the size alone can't tell it apart.
fn save_over(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    let later = fs::metadata(path).unwrap().modified().unwrap() + std::time::Duration::from_secs(5);
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(later)
        .unwrap();
}

/// CRITICAL data-loss regression (#139). An app saves over a file after its
/// copy finished and before the sweep reached it. The destination holds the old
/// bytes, so deleting the original throws the save away for good.
#[test]
fn an_original_saved_over_after_its_copy_survives_the_sweep() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let src_root = tmp.path().join("src");
    let dst_root = tmp.path().join("dst");
    let source = src_root.join("Work");
    fs::create_dir_all(source.join("notes")).unwrap();
    fs::create_dir_all(&dst_root).unwrap();
    let edited = source.join("notes/draft.txt");
    let untouched = source.join("report.txt");
    fs::write(&edited, b"first draft").unwrap();
    fs::write(&untouched, b"final report").unwrap();

    let edited_in_sink = edited.clone();
    let events = move_editing_before_sweep(&source, &dst_root, "op-drift-folder", move || {
        // Same length as "first draft", so only the timestamp gives it away.
        save_over(&edited_in_sink, b"final draft");
    });

    assert_eq!(
        fs::read(&edited).unwrap(),
        b"final draft",
        "the original keeps the bytes saved after its copy"
    );
    assert_eq!(
        fs::read(dst_root.join("Work/notes/draft.txt")).unwrap(),
        b"first draft",
        "precondition: the destination holds what the copy read"
    );
    assert!(!untouched.exists(), "an original nobody touched still goes");
    assert!(dst_root.join("Work/report.txt").is_file());
    assert!(
        source.is_dir(),
        "the source folder stays, it still holds the saved file"
    );

    assert_eq!(
        outcomes_for(&events.inner, &source).last().copied(),
        Some(SourceItemOutcome::Skipped),
        "a source left standing ends on Skipped"
    );
    assert!(!removal_flags_for(&events.inner, &source).contains(&true));

    let complete = events.inner.complete.lock_ignore_poison();
    let left = complete[0]
        .appeared_during_move
        .as_ref()
        .expect("the move kept a changed original, so it must say so");
    assert_eq!(left.changed_count, 1, "the saved-over original is counted");
    assert_eq!(left.item_count, 0, "nothing appeared");
    assert_eq!(left.folder_name, "Work");
    assert_eq!(left.folder_count, 1);
}

/// A single-file source the user saved over mid-move stays, whatever its new size.
#[test]
fn a_top_level_file_saved_over_after_its_copy_survives_the_sweep() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let src_root = tmp.path().join("Documents");
    let dst_root = tmp.path().join("dst");
    fs::create_dir_all(&src_root).unwrap();
    fs::create_dir_all(&dst_root).unwrap();
    let source = src_root.join("budget.xlsx");
    fs::write(&source, b"old numbers").unwrap();

    let source_in_sink = source.clone();
    let events = move_editing_before_sweep(&source, &dst_root, "op-drift-file", move || {
        save_over(&source_in_sink, b"new numbers, and more of them");
    });

    assert_eq!(fs::read(&source).unwrap(), b"new numbers, and more of them");
    assert_eq!(fs::read(dst_root.join("budget.xlsx")).unwrap(), b"old numbers");
    assert_eq!(
        outcomes_for(&events.inner, &source).last().copied(),
        Some(SourceItemOutcome::Skipped)
    );
    let complete = events.inner.complete.lock_ignore_poison();
    let left = complete[0]
        .appeared_during_move
        .as_ref()
        .expect("a kept original is news");
    assert_eq!(left.changed_count, 1);
    assert_eq!(
        left.folder_name, "Documents",
        "a file source is named by the folder it sits in"
    );
}

/// An editor that saves by writing a temp and renaming it over the original
/// leaves a different file at the same path. Same size and the same mtime (a
/// coarse clock, or a tool that copies the old timestamp across) still can't
/// hide it: the node changed.
#[cfg(unix)]
#[test]
fn an_original_replaced_by_rename_survives_even_with_the_same_size_and_mtime() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let src_root = tmp.path().join("src");
    let dst_root = tmp.path().join("dst");
    let source = src_root.join("Work");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&dst_root).unwrap();
    let edited = source.join("notes.txt");
    fs::write(&edited, b"version one").unwrap();

    let edited_in_sink = edited.clone();
    move_editing_before_sweep(&source, &dst_root, "op-drift-rename", move || {
        let mtime = fs::metadata(&edited_in_sink).unwrap().modified().unwrap();
        let temp = edited_in_sink.with_file_name(".notes.txt.swp");
        fs::write(&temp, b"version two").unwrap();
        fs::File::options()
            .write(true)
            .open(&temp)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        fs::rename(&temp, &edited_in_sink).unwrap();
    });

    assert_eq!(fs::read(&edited).unwrap(), b"version two");
    assert_eq!(fs::read(dst_root.join("Work/notes.txt")).unwrap(), b"version one");
}
