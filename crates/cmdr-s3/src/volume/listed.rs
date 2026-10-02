//! What the last listing (or stat) of a file said about it, kept for the
//! server-side copy that follows, so a one-request copy needs no HEAD of its
//! source (`server_copy.rs`): the engine lists every folder it copies, and the
//! scan stats every file it was handed, right before.
//!
//! - **Taken on use**: a copy reads its entry once; a later one asks again.
//! - **Bounded**: past [`LISTED_CACHE_CAP`] entries the lot clears, and those
//!   copies HEAD their source.
//! - **Stale is safe**: the copy pins the listed ETag
//!   (`x-amz-copy-source-if-match`). Where the provider enforces it, a source
//!   replaced since fails the pin and the copy asks the source once; where it
//!   doesn't, the copy's own ETag proves what landed.

use cmdr_fs::ignore_poison::IgnorePoison as _;

use super::S3Volume;
use super::listing::{Child, FILE_SUFFIX};
use super::paths::child_of;
use crate::transport::Answer;

/// A file's size and ETag, as its folder's listing (or its HEAD) gave them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ListedObject {
    pub size: u64,
    pub etag: String,
}

/// About 150 bytes an entry, so a few MB at most.
const LISTED_CACHE_CAP: usize = 20_000;

impl S3Volume {
    /// Keeps what a listing of `parent` said about each of its files, by the
    /// server-side path a copy resolves the file to (a `<name> (file)` row's
    /// real key). A file listed without an ETag is left out.
    pub(super) fn note_listed(&self, parent: &str, children: &[Child]) {
        let mut listed = self.inner.listed.lock_ignore_poison();
        for child in children {
            let Child::Object {
                name,
                size,
                etag: Some(etag),
                beside_folder,
                ..
            } = child
            else {
                continue;
            };
            let real = if *beside_folder {
                name.strip_suffix(FILE_SUFFIX).unwrap_or(name)
            } else {
                name
            };
            if listed.len() >= LISTED_CACHE_CAP {
                listed.clear();
            }
            listed.insert(
                child_of(parent, real),
                ListedObject {
                    size: *size,
                    etag: etag.clone(),
                },
            );
        }
    }

    /// Keeps what a HEAD of the file at `remote` said.
    pub(super) fn note_headed(&self, remote: &str, head: &Answer) {
        let (Some(size), Some(etag)) = (head.object_length(), head.header("etag")) else {
            return;
        };
        let mut listed = self.inner.listed.lock_ignore_poison();
        if listed.len() >= LISTED_CACHE_CAP {
            listed.clear();
        }
        listed.insert(
            remote.to_string(),
            ListedObject {
                size,
                etag: etag.to_string(),
            },
        );
    }

    /// What the last listing said about the file at `remote`, taken out.
    pub(super) fn take_listed(&self, remote: &str) -> Option<ListedObject> {
        self.inner.listed.lock_ignore_poison().remove(remote)
    }
}
