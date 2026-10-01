//! A batch of renames on a volume where some rename copies (an object store's
//! folders and big files, `Volume::rename_work`): the whole batch runs as ONE
//! background move with the new names (David's decision), so it gets the
//! transfer engine's progress, pause, cancel, and journaling.
//!
//! - **Order**: the executor's own dependency order (`plan.rs`), so a chain
//!   `a → b, b → c` moves `b` out of the way first.
//! - **Conflicts skip**: a name something outside the batch holds keeps its
//!   owner, the executor's answer too.
//! - ❗ **A cycle stays where it is**: `a ↔ b` would need a temporary name, and
//!   a move onto a FOLDER that's still there merges into it. Its rows are left
//!   out and logged, ❌ never moved onto each other.
//! - **The sources are bound** to the fingerprints preflight captured, the way
//!   every other approved operation is (`source_binding.rs`).

use std::path::PathBuf;

use super::BulkRenameRow;
use super::plan::{RenamePlanStep, build_execution_plan};

/// What a batch moves, in order, as `(source, new name)`, and which rows a
/// cycle kept out. No-op rows (a name that doesn't change) move nowhere.
pub(super) fn move_order(rows: &[BulkRenameRow]) -> (Vec<(PathBuf, String)>, Vec<usize>) {
    let active = vec![true; rows.len()];
    let mut moves = Vec::with_capacity(rows.len());
    let mut left_out = Vec::new();
    for step in build_execution_plan(rows, &active) {
        match step {
            RenamePlanStep::Direct(index) | RenamePlanStep::CaseOnly(index) => {
                let row = &rows[index];
                let Some(name) = row.destination.file_name() else {
                    left_out.push(index);
                    continue;
                };
                moves.push((row.source.clone(), name.to_string_lossy().into_owned()));
            }
            RenamePlanStep::Cycle(indices) => left_out.extend(indices),
        }
    }
    (moves, left_out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_system::write_operations::source_binding::{RemoteContent, SourceFingerprint};

    fn row(id: &str, from: &str, to: &str) -> BulkRenameRow {
        BulkRenameRow {
            row_id: id.to_string(),
            source: PathBuf::from(from),
            destination: PathBuf::from(to),
            expected_fingerprint: SourceFingerprint::Remote {
                normalized_path: from.to_string(),
                content: RemoteContent::Directory,
            },
        }
    }

    #[test]
    fn a_chain_moves_its_last_link_first() {
        let rows = vec![row("1", "/d/a", "/d/b"), row("2", "/d/b", "/d/c")];
        let (moves, left_out) = move_order(&rows);
        assert_eq!(
            moves,
            vec![
                (PathBuf::from("/d/b"), "c".to_string()),
                (PathBuf::from("/d/a"), "b".to_string()),
            ]
        );
        assert!(left_out.is_empty());
    }

    /// ❗ A swap would merge one folder into the other: it stays out.
    #[test]
    fn a_cycle_is_left_out_and_a_noop_moves_nowhere() {
        let rows = vec![
            row("1", "/d/x", "/d/y"),
            row("2", "/d/y", "/d/x"),
            row("3", "/d/same", "/d/same"),
            row("4", "/d/old", "/d/new"),
        ];
        let (moves, mut left_out) = move_order(&rows);
        assert_eq!(moves, vec![(PathBuf::from("/d/old"), "new".to_string())]);
        left_out.sort_unstable();
        assert_eq!(left_out, vec![0, 1]);
    }
}
