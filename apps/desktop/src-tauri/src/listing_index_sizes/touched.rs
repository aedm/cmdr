//! What one index update means for one open listing: nothing, some of its rows, or all of them.
//!
//! The index reports a batch of directories whose recursive sizes changed. A live batch carries each
//! changed directory's whole ancestor chain up to `/` (`paths::path_prefix::with_ancestor_closure`),
//! a network or phone watcher reports just the changed directory's parent, and three whole-volume
//! moments (a full scan finishing, a replay overflowing, a network scan finishing) send `["/"]` or the
//! volume id. So for a listing at `dir`:
//!
//! - `dir` itself in the batch: its own recursive size moved, which is the `..` row.
//! - A path strictly under `dir`: the row for the child on the way down moved (its recursive size
//!   includes that path). That's the case a pane on `~` meets for a write deep in `~/Library`: the
//!   `Library` row's size really changes.
//! - An ANCESTOR of `dir` in the batch says nothing about it: every live batch names `/`, so reading
//!   an ancestor as "refresh everything" would refresh every pane on every write anywhere.
//! - `["/"]` alone, or the listing's volume id: the whole-volume moments, every row.

use std::collections::BTreeSet;

/// The rows of one listing an index update touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Touched {
    /// Every row, and the listing's own size.
    Whole,
    /// Some rows.
    Rows {
        /// The listing's own recursive size moved (the `..` row).
        own: bool,
        /// Names of the child rows whose recursive sizes moved.
        children: BTreeSet<String>,
    },
}

impl Touched {
    /// Folds a later update into this one, so a listing that sat out a few batches refreshes once
    /// for all of them.
    pub(crate) fn merge(&mut self, later: Touched) {
        match (&mut *self, later) {
            (Touched::Whole, _) => {}
            (_, Touched::Whole) => *self = Touched::Whole,
            (
                Touched::Rows { own, children },
                Touched::Rows {
                    own: later_own,
                    children: later_children,
                },
            ) => {
                *own |= later_own;
                children.extend(later_children);
            }
        }
    }
}

/// What `paths` touched in the listing at `dir` (in the index's path space) on `volume_id`, or `None`
/// when nothing it shows moved.
pub(crate) fn touched(paths: &[String], volume_id: &str, dir: &str) -> Option<Touched> {
    if paths.len() == 1 && paths[0] == "/" {
        return Some(Touched::Whole);
    }
    let mut own = false;
    let mut children = BTreeSet::new();
    for path in paths {
        if path == volume_id {
            return Some(Touched::Whole);
        }
        if path == dir {
            own = true;
            continue;
        }
        if let Some(child) = child_on_the_way_to(path, dir) {
            // The listing's own size includes everything under it, even when the batch (a network
            // or phone watcher's single parent) doesn't name `dir` itself.
            own = true;
            children.insert(child.to_string());
        }
    }
    (own || !children.is_empty()).then_some(Touched::Rows { own, children })
}

/// The name of `dir`'s child that `path` sits in (or is), when `path` is strictly under `dir`.
/// Component-aware: `/a/bc` is not under `/a/b`.
fn child_on_the_way_to<'a>(path: &'a str, dir: &str) -> Option<&'a str> {
    let rest = if dir == "/" {
        path.strip_prefix('/')?
    } else {
        path.strip_prefix(dir)?.strip_prefix('/')?
    };
    let name = rest.split('/').next()?;
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(list: &[&str]) -> Vec<String> {
        list.iter().map(|p| p.to_string()).collect()
    }

    fn rows(own: bool, children: &[&str]) -> Option<Touched> {
        Some(Touched::Rows {
            own,
            children: children.iter().map(|c| c.to_string()).collect(),
        })
    }

    #[test]
    fn a_write_deep_under_a_child_touches_that_childs_row() {
        // What a pane on `~` meets all day: a cache write deep in `~/Library`.
        let batch = paths(&[
            "/Users/me/Library/Caches/app/blobs",
            "/Users/me/Library/Caches/app",
            "/Users/me/Library/Caches",
            "/Users/me/Library",
            "/Users/me",
            "/Users",
            "/",
        ]);
        assert_eq!(touched(&batch, "root", "/Users/me"), rows(true, &["Library"]));
    }

    #[test]
    fn a_write_elsewhere_touches_nothing_even_though_every_batch_names_the_root() {
        let batch = paths(&[
            "/Users/me/Library/Caches",
            "/Users/me/Library",
            "/Users/me",
            "/Users",
            "/",
        ]);
        assert_eq!(touched(&batch, "root", "/Users/me/Downloads"), None);
    }

    #[test]
    fn a_write_directly_in_the_folder_touches_only_its_own_size() {
        let batch = paths(&["/Users/me/Downloads", "/Users/me", "/Users", "/"]);
        assert_eq!(touched(&batch, "root", "/Users/me/Downloads"), rows(true, &[]));
    }

    #[test]
    fn a_pane_on_the_root_sees_every_top_level_folder_a_batch_names() {
        let batch = paths(&[
            "/Users/me/x",
            "/Users/me",
            "/Users",
            "/Applications/Foo.app",
            "/Applications",
            "/",
        ]);
        assert_eq!(touched(&batch, "root", "/"), rows(true, &["Applications", "Users"]));
    }

    #[test]
    fn a_single_parent_from_a_network_watcher_still_finds_the_child() {
        // SMB and MTP watchers report only the changed directory's parent, no ancestor chain.
        let batch = paths(&["/Volumes/share/photos/2026/summer"]);
        assert_eq!(touched(&batch, "smb-1", "/Volumes/share"), rows(true, &["photos"]));
    }

    #[test]
    fn the_whole_volume_moments_touch_every_row() {
        assert_eq!(touched(&paths(&["/"]), "root", "/Users/me"), Some(Touched::Whole));
        assert_eq!(
            touched(&paths(&["smb-1"]), "smb-1", "/Volumes/share"),
            Some(Touched::Whole)
        );
    }

    #[test]
    fn another_volumes_id_touches_nothing() {
        assert_eq!(touched(&paths(&["smb-2"]), "smb-1", "/Volumes/share"), None);
    }

    #[test]
    fn matching_is_component_aware() {
        let batch = paths(&["/Users/me/Downloads-old/x", "/Users/me/Downloads-old"]);
        assert_eq!(touched(&batch, "root", "/Users/me/Downloads"), None);
    }

    #[test]
    fn merging_folds_rows_and_whole_wins() {
        let mut first = Touched::Rows {
            own: false,
            children: ["a".to_string()].into(),
        };
        first.merge(Touched::Rows {
            own: true,
            children: ["b".to_string()].into(),
        });
        assert_eq!(Some(first.clone()), rows(true, &["a", "b"]));

        first.merge(Touched::Whole);
        assert_eq!(first, Touched::Whole);
        first.merge(Touched::Rows {
            own: false,
            children: BTreeSet::new(),
        });
        assert_eq!(first, Touched::Whole);
    }
}
