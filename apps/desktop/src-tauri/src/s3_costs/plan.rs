//! Turns a dialog's operation and its scan into the billed work per provider.
//! Pure: the sides arrive as empty `Workload`s, one per S3 end, and the files as
//! the scan saw them.

use cmdr_s3::cost::Workload;
use serde::Deserialize;

use crate::file_system::volume::ScannedFile;
use crate::file_system::write_operations::ScanCostFacts;

/// The operation a dialog is about to start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum CostedOperation {
    Copy,
    Move,
    Delete,
}

/// Each end of the operation: an empty workload when it's an S3 place, and
/// whether the two ends copy on the server (`S3Volume::copies_on_server_from`).
pub(super) struct Sides {
    pub source: Option<Workload>,
    pub destination: Option<Workload>,
    pub server_copy: bool,
}

/// The billed work per S3 provider the operation touches: the source's first,
/// then the destination's. A server-side copy bills one account, so it's one
/// workload.
pub(super) fn plan(operation: CostedOperation, sides: Sides, facts: &ScanCostFacts) -> Vec<Workload> {
    let Sides {
        mut source,
        mut destination,
        server_copy,
    } = sides;
    let files = files_of(facts);

    if operation == CostedOperation::Delete {
        if let Some(source) = source.as_mut() {
            delete_tree(source, &files, facts.dirs);
            // A volume delete lists each folder again as it recurses.
            (0..facts.dirs).for_each(|_| source.list_folder());
        }
        return source.into_iter().collect();
    }

    if server_copy && let (Some(both), Some(_)) = (source.as_mut(), destination.as_ref()) {
        for file in &files {
            both.copy_on_server(file.size);
        }
        write_folder_markers(both, facts.dirs);
        destination = None;
    } else {
        if let Some(source) = source.as_mut() {
            files.iter().for_each(|file| source.download(file.size));
        }
        if let Some(destination) = destination.as_mut() {
            files.iter().for_each(|file| destination.upload(file.size));
            write_folder_markers(destination, facts.dirs);
        }
    }
    if operation == CostedOperation::Move
        && let Some(source) = source.as_mut()
    {
        delete_tree(source, &files, facts.dirs);
    }
    source.into_iter().chain(destination).collect()
}

/// Every file the scan saw, or, when it kept no per-file list, its byte total
/// spread evenly over its file count (no dates, so no early-deletion charge).
fn files_of(facts: &ScanCostFacts) -> Vec<ScannedFile> {
    if let Some(files) = &facts.per_file {
        return files.clone();
    }
    let count = facts.files as u64;
    if count == 0 {
        return Vec::new();
    }
    let (each, rest) = (facts.bytes / count, facts.bytes % count);
    (0..count)
        .map(|index| ScannedFile {
            size: each + u64::from(index < rest),
            modified_at: None,
        })
        .collect()
}

/// A tree's objects into `DeleteObjects` batches, then each emptied folder.
fn delete_tree(work: &mut Workload, files: &[ScannedFile], dirs: usize) {
    for file in files {
        work.delete_object(file.size, file.modified_at);
    }
    (0..dirs).for_each(|_| work.delete_folder());
}

/// Each copied folder gets a zero-byte marker at the destination, so it
/// survives emptying (`crates/cmdr-s3/DETAILS.md` § "Folders, delete, and
/// rename").
fn write_folder_markers(work: &mut Workload, dirs: usize) {
    (0..dirs).for_each(|_| work.upload(0));
}
