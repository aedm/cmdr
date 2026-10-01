//! A `ListObjectsV2` page as a folder's children. Pure: no request, no path
//! spelling, so the rules about what S3 hands back are testable on their own.
//!
//! S3 has no folders, only keys. With `delimiter=/`, a listing of `photos/`
//! answers `CommonPrefixes` (each a "folder") and `Contents` (the objects
//! directly inside). This module is where those become what a pane shows.

use std::time::SystemTime;

use crate::xml::ObjectPage;

/// One child of the folder a page lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Child {
    /// A common prefix, or a folder marker: something with keys under it.
    Folder { name: String },
    /// An object directly in the folder.
    Object {
        name: String,
        size: u64,
        /// `LastModified`: the upload time. ❗ A listing carries no user
        /// metadata, so `x-amz-meta-mtime` is only on `get_metadata`'s HEAD.
        modified: Option<SystemTime>,
        /// Glacier Flexible Retrieval or Deep Archive: reads fail until
        /// restored.
        archived: bool,
    },
}

impl Child {
    pub(super) fn name(&self) -> &str {
        match self {
            Self::Folder { name } | Self::Object { name, .. } => name,
        }
    }

    pub(super) fn is_folder(&self) -> bool {
        matches!(self, Self::Folder { .. })
    }
}

/// The children `page` names under `prefix` (`""` for a bucket's top, else
/// ending in `/`), folders first, each in the page's order.
///
/// - **The folder's own marker** (the key `prefix` itself) is left out.
/// - **A key with a `/` past the prefix** names the folder it sits under: a
///   child's marker (`photos/empty/`), or a deeper key from a server that
///   ignored the delimiter. Each folder appears once.
/// - **A name nothing can address** is left out: empty (from `a//b`), `.`, and
///   `..`, which every URL parser resolves away (`encoding::KeyError`).
/// - ❗ **An object and a folder of one name keep the folder.** S3 allows both
///   (`notes` beside `notes/…`), but one name in a pane is one path, and two
///   entries on one path break everything keyed on it. The folder holds more.
///   The object stays reachable by its path; it just isn't listed.
pub(super) fn children_of(page: &ObjectPage, prefix: &str) -> Vec<Child> {
    let mut folders: Vec<String> = Vec::new();
    let mut add_folder = |name: &str| {
        if addressable(name) && !folders.iter().any(|known| known == name) {
            folders.push(name.to_string());
        }
    };
    for common in &page.prefixes {
        if let Some(rest) = common.strip_prefix(prefix) {
            add_folder(rest.split('/').next().unwrap_or_default());
        }
    }
    let mut objects = Vec::new();
    for object in &page.objects {
        let Some(rest) = object.key.strip_prefix(prefix) else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        if let Some((folder, _)) = rest.split_once('/') {
            add_folder(folder);
            continue;
        }
        if addressable(rest) {
            objects.push(Child::Object {
                name: rest.to_string(),
                size: object.size,
                modified: object.last_modified,
                archived: object.storage_class.is_archived(),
            });
        }
    }
    objects.retain(|object| !folders.iter().any(|folder| folder == object.name()));
    folders
        .into_iter()
        .map(|name| Child::Folder { name })
        .chain(objects)
        .collect()
}

/// Whether a name can be a path segment Cmdr sends.
fn addressable(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".."
}

/// What a folder holds, read off one listing of `prefix` (ending in `/`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FolderContents {
    /// No key at all under the prefix: no folder by that name.
    Nothing,
    /// Only the folder's own zero-byte marker (`prefix` itself).
    MarkerOnly,
    /// Something besides the marker: a file, or a folder below.
    Holds,
}

/// Judges a page listing `prefix` with `delimiter=/` and `max-keys` of at
/// least two, which is enough to tell a marker alone from a marker and a child.
pub(super) fn folder_contents(page: &ObjectPage, prefix: &str) -> FolderContents {
    let others = page.objects.iter().filter(|object| object.key != prefix).count() + page.prefixes.len();
    if others > 0 {
        FolderContents::Holds
    } else if page.objects.is_empty() {
        FolderContents::Nothing
    } else {
        FolderContents::MarkerOnly
    }
}

#[cfg(test)]
#[path = "listing_test.rs"]
mod listing_test;
