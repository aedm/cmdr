//! Turns a dialog's operation and its scan into the billed work per provider.
//! Pure: the sides arrive as empty `Workload`s, one per S3 end, and the files as
//! the scan saw them.

use cmdr_s3::cost::Workload;
use serde::Deserialize;

use crate::file_system::volume::ScannedFile;
use crate::file_system::write_operations::{ConflictResolution, ScanCostFacts};

/// The operation a dialog is about to start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum CostedOperation {
    Copy,
    Move,
    Delete,
}

/// A file the dialog's conflict check found at the destination under a name
/// a copied file takes: the two files' sizes and dates, as the check's one
/// destination listing saw them (on S3, the destination's upload time).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct KnownClash {
    pub source_size: u64,
    pub dest_size: u64,
    /// Unix seconds.
    pub source_modified: Option<u64>,
    /// Unix seconds.
    pub dest_modified: Option<u64>,
}

/// The clashes the conflict check found, and the policy the dialog will
/// answer them with.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ClashPlan {
    pub resolution: ConflictResolution,
    pub clashes: Vec<KnownClash>,
}

/// An existing destination file the operation writes over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Overwrite {
    /// The bytes written over it.
    pub incoming_size: u64,
    pub replaced: ScannedFile,
}

/// The clashes `plan.resolution` overwrites, decided the way the transfer
/// does (`transfer/volume/conflict.rs`): strictly smaller, strictly older
/// with both dates known. Stop asks about each one as it comes, so none is
/// assumed; Skip and Rename overwrite nothing.
pub(super) fn overwritten(plan: &ClashPlan) -> Vec<Overwrite> {
    let overwrites = |clash: &KnownClash| match plan.resolution {
        ConflictResolution::Overwrite => true,
        ConflictResolution::OverwriteSmaller => clash.dest_size < clash.source_size,
        ConflictResolution::OverwriteOlder => matches!(
            (clash.source_modified, clash.dest_modified),
            (Some(source), Some(dest)) if dest < source
        ),
        ConflictResolution::Stop | ConflictResolution::Skip | ConflictResolution::Rename => false,
    };
    plan.clashes
        .iter()
        .filter(|clash| overwrites(clash))
        .map(|clash| Overwrite {
            incoming_size: clash.source_size,
            replaced: ScannedFile {
                size: clash.dest_size,
                modified_at: clash.dest_modified,
            },
        })
        .collect()
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
/// workload. `overwrites` are the destination files the copy writes over.
pub(super) fn plan(
    operation: CostedOperation,
    sides: Sides,
    facts: &ScanCostFacts,
    overwrites: &[Overwrite],
) -> Vec<Workload> {
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
        // The copy replaces each one in a single request.
        for overwrite in overwrites {
            both.replace_object(overwrite.replaced.size, overwrite.replaced.modified_at);
        }
        destination = None;
    } else {
        if let Some(source) = source.as_mut() {
            files.iter().for_each(|file| source.download(file.size));
        }
        if let Some(destination) = destination.as_mut() {
            files.iter().for_each(|file| destination.upload(file.size));
            write_folder_markers(destination, facts.dirs);
            for overwrite in overwrites {
                destination.replace_object(overwrite.replaced.size, overwrite.replaced.modified_at);
                destination.upload_over(overwrite.incoming_size);
            }
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
