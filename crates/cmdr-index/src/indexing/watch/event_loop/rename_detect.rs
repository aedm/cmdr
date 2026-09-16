//! Rename detection by inode: the pre-pass `process_live_batch` runs between its
//! dir-creation phase and everything else, and the path split it needs.
//!
//! Its own file because it answers one question the rest of the live loop doesn't
//! ask: "is this `item_renamed` event the same entry under a new name?" A wrong
//! answer here costs a renamed directory its `dir_stats` and its whole subtree
//! until a full scan heals it, which is why it reads as one piece.

use std::collections::HashSet;

use rusqlite::Connection;

use super::super::watcher;
use crate::indexing::IndexPathSpace;
use crate::indexing::metadata;
use crate::indexing::store::{self, IndexStore};
use crate::indexing::writer::{IndexWriter, WriteMessage};

/// Inspect every `item_renamed` event in `events`. For each path that still
/// exists on disk and has an inode that already maps to a DB entry at a
/// *different* `(parent_id, name)`, send `MoveEntryV2` and remove the event.
///
/// Returns the NEW paths of the renames it handled, so the caller can decide
/// whether to flush before Phase 2 and can report them as renames.
///
/// ⚠️ **The paths, ❌ not a bare count.** A matched event is `retain`ed out of
/// `events`, so after this call only the FAILED matches are still in the batch.
/// Anything downstream reading the corrected stream alone would therefore see
/// the noise and none of the signal, and a rename-only batch would look empty.
/// These are the successes, and they are only available here.
///
/// Events whose stat fails are *not* removed (they're either the OLD-path
/// side of a successful match, which silently no-ops in Phase 2 once the row
/// has moved, or true removals/unrelated noise that Phase 2 needs to see).
pub(super) fn detect_renames_by_inode(
    events: &mut Vec<(String, watcher::FsChangeEvent)>,
    space: &IndexPathSpace,
    conn: &Connection,
    writer: &IndexWriter,
    pending_origins: &mut HashSet<String>,
    max_event_id: &mut u64,
) -> Vec<String> {
    let mut handled: Vec<String> = Vec::new();

    events.retain(|(path, event)| {
        if !event.flags.item_renamed {
            return true;
        }

        // A volume whose inodes aren't trustworthy (FAT/exFAT) stores `inode: None`
        // for every entry, so `find_entry_by_inode` below can never match — the
        // pre-pass is inert there and renames fall back to the safe create/delete
        // path. Short-circuit up front so a FAT volume skips the per-event stat +
        // query entirely (the raw `symlink_metadata` inode here is the unstable
        // derived-cluster value, so it must NOT drive a match).
        if !space.inodes_trustworthy() {
            return true;
        }

        let metadata = match std::fs::symlink_metadata(path) {
            Ok(m) => m,
            // Path doesn't exist (or is unreadable). Could be the OLD-path
            // event of a successful rename, or a true removal. Phase 2
            // handles both.
            Err(_) => return true,
        };

        let is_dir = metadata.is_dir();
        let is_symlink = metadata.is_symlink();
        let snap = metadata::extract_metadata(&metadata, is_dir, is_symlink);

        // Symlinks carry no inode. Fall through to the create/delete path.
        let inode = match snap.inode {
            Some(i) => i,
            None => return true,
        };

        let existing_id = match IndexStore::find_entry_by_inode(conn, inode) {
            Ok(Some(id)) => id,
            // No DB row for this inode. Phase 2 will create one.
            Ok(None) => return true,
            Err(e) => {
                log::warn!(target: "indexing::event_loop", "rename pre-pass: find_entry_by_inode({inode}) failed: {e}");
                return true;
            }
        };

        let (new_parent_path, new_name) = match split_parent_and_name(path) {
            Some(p) => p,
            None => return true,
        };

        // `new_parent_path` is FS-event-derived (absolute); strip the mount root for
        // a mount-rooted drive at the resolve. `pending_origins.insert` below keeps it
        // absolute (it drives the FE emit).
        let new_parent_id = match space.resolve_abs(conn, &new_parent_path) {
            Ok(Some(id)) => id,
            // New parent isn't in the DB yet; let Phase 2 handle it via the
            // existing create/modify path. Without a parent ID we can't move.
            Ok(None) => return true,
            Err(e) => {
                log::warn!(
                    target: "indexing::event_loop",
                    "rename pre-pass: resolve_path({new_parent_path}) failed: {e}",
                );
                return true;
            }
        };

        // Defensive no-op: if the entry is already at the target location
        // (e.g. an inode collision on a non-rename event), skip.
        if let Ok(Some(old_entry)) = IndexStore::get_entry_by_id(conn, existing_id)
            && old_entry.parent_id == new_parent_id
                && store::normalize_for_comparison(&old_entry.name) == store::normalize_for_comparison(&new_name)
            {
                return true;
            }

        if let Err(e) = writer.send(WriteMessage::MoveEntryV2 {
            entry_id: existing_id,
            new_parent_id,
            new_name: new_name.clone(),
        }) {
            log::warn!(target: "indexing::event_loop", "rename pre-pass: MoveEntryV2 send failed: {e}");
            return true;
        }

        log::debug!(
            target: "indexing::event_loop",
            "rename pre-pass: matched inode={inode} → MoveEntryV2 id={existing_id} new_parent={new_parent_id} name={new_name}",
        );

        // The new parent's listing gained an entry, so it is an origin. The old
        // parent is already covered by the OLD-path event still in `pending_events`
        // (the reconciler reports it from `process_live_event` when its
        // `resolve_path` no-ops). A consumer that expands downward reaches the moved
        // directory itself through this parent, which is what makes a rename INTO a
        // `node_modules` still flip its whole subtree's floor status.
        pending_origins.insert(new_parent_path);
        *max_event_id = (*max_event_id).max(event.event_id);
        handled.push(path.clone());
        false
    });

    handled
}

/// Split `/a/b/c` into (`/a/b`, `c`). Returns `None` for paths whose trailing
/// component is empty (the root `/`).
pub(super) fn split_parent_and_name(path: &str) -> Option<(String, String)> {
    let trimmed = path.strip_suffix('/').unwrap_or(path);
    if trimmed.is_empty() {
        return None;
    }
    let idx = trimmed.rfind('/')?;
    let name = &trimmed[idx + 1..];
    if name.is_empty() {
        return None;
    }
    let parent = if idx == 0 {
        "/".to_string()
    } else {
        trimmed[..idx].to_string()
    };
    Some((parent, name.to_string()))
}
