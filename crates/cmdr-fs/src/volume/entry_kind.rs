//! [`EntryKind`]: what the entry AT a path is, with a link reported as the link.

use crate::entry::FileEntry;

/// What the entry AT a path is, without following a link there
/// ([`Volume::entry_kind`](super::Volume::entry_kind)).
///
/// The question an operation asks before it descends into, merges with, or
/// recursively deletes an entry. `FileEntry::is_directory` can't answer it: a
/// listing reports a link to a folder as a directory, and whatever descends
/// through that link lists, moves, or deletes the TARGET's children, a folder
/// the user never selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A regular file, or anything else that is neither a directory nor a link.
    File,
    /// A real directory.
    Directory,
    /// A symbolic link, whatever it points at (a folder, a file, or nothing).
    Symlink,
}

impl EntryKind {
    /// The kind an entry's own two flags describe: the link bit wins, because
    /// a listing sets `is_directory` on a link whose target is a folder.
    pub fn of(entry: &FileEntry) -> Self {
        if entry.is_symlink {
            Self::Symlink
        } else if entry.is_directory {
            Self::Directory
        } else {
            Self::File
        }
    }
}
