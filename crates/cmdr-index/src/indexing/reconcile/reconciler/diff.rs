//! The per-directory diff: one directory's live listing against its DB rows.
//!
//! The one place a reconcile decides what changed, shared by the local `read_dir`
//! walk and the network `Volume`-trait walk so the two can't drift.

use std::collections::HashSet;

use crate::indexing::metadata::MetadataSnapshot;
use crate::indexing::store;
use crate::indexing::writer::{IndexWriter, WriteMessage};

/// A live directory child, normalized to the fields the per-dir diff needs.
/// The two walk sources build this identically: the local path from an
/// [`FsChild`](super::FsChild), the network path from a `Volume` listing's `FileEntry`. The diff
/// is then source-agnostic — the same add/remove/modify/type-change logic for both.
pub(crate) struct LiveChild {
    pub name: String,
    pub is_directory: bool,
    pub is_symlink: bool,
    pub snap: MetadataSnapshot,
}

/// Whether this diff may delete the DB rows its live listing didn't mention.
///
/// A missing row only means "gone" when the observation behind it was whole: the
/// listing saw everything, AND the drive was still listed after the read. Either one
/// failing makes a missing row prove nothing at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MissingRows {
    /// The listing was complete and its drive still listed after it, so a row the
    /// listing lacks is really gone.
    Delete,
    /// The listing came back short, or the drive stopped being listed. The diff
    /// upserts everything it DID see and deletes nothing.
    Keep,
}

/// Outcome of diffing ONE directory's live children against its DB rows.
pub(crate) struct DirDiff<'a> {
    pub added: u64,
    pub removed: u64,
    pub updated: u64,
    /// `(child_dir_id, child_name)` for every EXISTING child dir that matched a
    /// live dir — the caller recurses into these (their id is already known).
    /// This is UNCONDITIONAL of whether the dir changed: an unchanged dir still
    /// recurses, because "unchanged at the parent's level" says nothing about
    /// whether its own subtree was ever listed. Gating this on `changed` is the
    /// reconcile-stops-at-the-root bug (see the fn doc).
    pub matched_child_dirs: Vec<(i64, String)>,
    /// Names of NEW child dirs created this pass — the caller flushes the writer,
    /// resolves their ids, then recurses (the id isn't known until the insert
    /// commits). ⚠️ Wider than the new-ROW set below: a child that was a file and
    /// is now a directory is an UPDATE whose subtree still has to be walked.
    pub new_child_dir_names: Vec<String>,
    /// Every child this pass CREATED a row for, borrowed from the listing it was
    /// diffed against. Empty for an unchanged directory, so the no-op-cheap
    /// property costs nothing here.
    ///
    /// A caller only filling the index can ignore it. A caller with a LIVE
    /// consumer can't: a row the index already held is the index's to report, and
    /// a row this pass wrote is nobody else's — so a walk that doesn't hand these
    /// over answers as if the ground it just covered were empty.
    pub added_children: Vec<&'a LiveChild>,
}

/// Diff one directory's live listing against its DB children and emit only the
/// differences (`UpsertEntryV2` for adds + changes, `DeleteEntryById` /
/// `DeleteSubtreeById` for vanished rows). Shared by the local `read_dir` walk
/// and the network `Volume`-trait walk so the diff logic lives in ONE place.
///
/// Writes NOTHING for an unchanged row (the no-op-cheap property the perf bench
/// relied on): a matched row is re-UPSERTed only when its size/mtime (file) or
/// mtime (dir/symlink) actually differs, so a rescan over an unchanged tree
/// issues zero entry-row writes and never touches the catastrophic
/// `INSERT OR REPLACE`/`platform_case` path. That holds for hardlinks too: a
/// row the writer deduped to NULL sizes compares on mtime alone, so the pass
/// converges instead of re-sending it forever.
///
/// The recursion set (`matched_child_dirs`) is DECOUPLED from that write
/// decision: every matched child dir is returned for the caller to descend into,
/// changed or not. The walk must re-list each existing child dir's subtree on a
/// reconcile — a child being unchanged at THIS dir's level proves nothing about
/// whether its subtree was ever scanned. (Re-gating recursion on `changed` is the
/// reconcile-stops-at-the-root prod bug: a share with only its top dirs indexed
/// would match them, write nothing, recurse nowhere, and "complete" instantly
/// over an unscanned tree.)
pub(crate) fn diff_dir_against_db<'a>(
    dir_id: i64,
    live_children: &'a [LiveChild],
    db_children: &[store::EntryRow],
    missing: MissingRows,
    writer: &IndexWriter,
) -> DirDiff<'a> {
    let mut added: u64 = 0;
    let mut removed: u64 = 0;
    let mut updated: u64 = 0;
    let mut matched_child_dirs: Vec<(i64, String)> = Vec::new();
    let mut new_child_dir_names: Vec<String> = Vec::new();
    let mut added_children: Vec<&'a LiveChild> = Vec::new();

    let mut db_by_name: std::collections::HashMap<String, &store::EntryRow> =
        std::collections::HashMap::with_capacity(db_children.len());
    for row in db_children {
        db_by_name.insert(store::normalize_for_comparison(&row.name), row);
    }

    let mut matched_db_keys: HashSet<String> = HashSet::with_capacity(live_children.len());

    for child in live_children {
        let norm_name = store::normalize_for_comparison(&child.name);
        let is_dir = child.is_directory;
        let is_symlink = child.is_symlink;
        let snap = &child.snap;

        if let Some(db_row) = db_by_name.get(&norm_name) {
            matched_db_keys.insert(norm_name);

            let changed = if is_dir || is_symlink {
                snap.modified_at != db_row.modified_at
            } else {
                // A NULL DB size on a multi-link file is the writer's hardlink
                // dedup, not a mismatch: the bytes live on another occurrence of
                // the inode. Comparing sizes there re-upserts the row on every
                // pass forever (the writer re-nulls it, the next diff re-sends
                // it), so mtime is the only signal. `nlink == 1` still compares
                // on size, which is what restores a real size once the other
                // links are gone. `verifier.rs` makes the same call.
                let is_deduped_hardlink = db_row.logical_size.is_none() && matches!(snap.nlink, Some(n) if n > 1);
                (!is_deduped_hardlink && snap.logical_size != db_row.logical_size)
                    || snap.modified_at != db_row.modified_at
            };

            if changed {
                // Type change (file↔dir): delete first so counts propagate correctly.
                // Dir→file: DeleteSubtreeById removes children + propagates negative deltas.
                // File→dir: DeleteEntryById propagates negative file_count delta.
                // Then UpsertEntryV2 inserts fresh with the correct type and positive deltas.
                if db_row.is_directory != is_dir {
                    if db_row.is_directory {
                        let _ = writer.send(WriteMessage::DeleteSubtreeById(db_row.id));
                    } else {
                        let _ = writer.send(WriteMessage::DeleteEntryById(db_row.id));
                    }
                }

                let _ = writer.send(WriteMessage::UpsertEntryV2 {
                    parent_id: dir_id,
                    name: child.name.clone(),
                    is_directory: is_dir,
                    is_symlink,
                    logical_size: snap.logical_size,
                    physical_size: snap.physical_size,
                    modified_at: snap.modified_at,
                    inode: snap.inode,
                    nlink: snap.nlink,
                });
                updated += 1;
            }

            // Recurse into this child if it's a (non-symlink) dir on disk:
            // - was already a dir in the DB → recurse the existing id;
            // - was a file, now a dir (type change) → the old row was deleted and
            //   a fresh dir inserted above, so treat it like a new dir: resolve the
            //   new id after a flush, then recurse (its children must be walked).
            if is_dir && !is_symlink {
                if db_row.is_directory {
                    matched_child_dirs.push((db_row.id, child.name.clone()));
                } else {
                    new_child_dir_names.push(child.name.clone());
                }
            }
        } else {
            let _ = writer.send(WriteMessage::UpsertEntryV2 {
                parent_id: dir_id,
                name: child.name.clone(),
                is_directory: is_dir,
                is_symlink,
                logical_size: snap.logical_size,
                physical_size: snap.physical_size,
                modified_at: snap.modified_at,
                inode: snap.inode,
                nlink: snap.nlink,
            });
            // UpsertEntryV2 auto-propagates deltas in the writer.
            added += 1;
            added_children.push(child);

            if is_dir && !is_symlink {
                new_child_dir_names.push(child.name.clone());
            }
        }
    }

    // Everything above this line is an ADD or an UPDATE, driven by something the
    // listing actually saw, so it runs whatever `missing` says. Only the reaping of
    // rows the listing DIDN'T mention depends on the observation having been whole.
    if missing == MissingRows::Delete {
        for row in db_children {
            let norm_name = store::normalize_for_comparison(&row.name);
            if !matched_db_keys.contains(&norm_name) {
                if row.is_directory {
                    let _ = writer.send(WriteMessage::DeleteSubtreeById(row.id));
                } else {
                    let _ = writer.send(WriteMessage::DeleteEntryById(row.id));
                }
                removed += 1;
            }
        }
    }

    DirDiff {
        added,
        removed,
        updated,
        matched_child_dirs,
        new_child_dir_names,
        added_children,
    }
}
