//! The paths the app addresses a place with, down to the bucket and key a
//! request names.
//!
//! The translation is `cmdr_fs::volume::remote_paths`, shared with SFTP and
//! WebDAV: the app spelling carries the account's `s3://<key id>@<host>:<port>`
//! prefix, the server-side tree under it is `/<bucket>/<key…>`, and a place is
//! rooted at `/` (the account, listing buckets) or `/<bucket>`. A bare
//! server-absolute path is ❌ REFUSED rather than anchored; that module's header
//! has why.

use std::path::Path;

use cmdr_fs::volume::VolumeError;

use super::S3Volume;

/// What a server-side path names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Target<'a> {
    /// `/`: the account, whose children are its buckets.
    Account,
    /// `/<bucket>`: a bucket's top.
    Bucket(&'a str),
    /// `/<bucket>/<key>`: an object, or a folder (a prefix) under the bucket.
    Key { bucket: &'a str, key: &'a str },
}

/// Splits a normalized server-side path (`RemoteRoot::to_remote_path`'s
/// answer: absolute, no trailing slash, no `.` or `..`).
pub(super) fn target_of(remote: &str) -> Target<'_> {
    let rest = remote.trim_start_matches('/');
    match rest.split_once('/') {
        _ if rest.is_empty() => Target::Account,
        Some((bucket, key)) if !key.is_empty() => Target::Key { bucket, key },
        Some((bucket, _)) => Target::Bucket(bucket),
        None => Target::Bucket(rest),
    }
}

impl S3Volume {
    /// The server-side path for `path`, or `NotFound` when `path` isn't on this
    /// place.
    pub(super) fn to_remote_path(&self, path: &Path) -> Result<String, VolumeError> {
        self.root
            .to_remote_path(path)
            .ok_or_else(|| VolumeError::NotFound(path.to_string_lossy().into_owned()))
    }
}

/// `parent/name` in server-side spelling.
pub(super) fn child_of(parent: &str, name: &str) -> String {
    if parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}/{name}")
    }
}

#[cfg(test)]
#[path = "paths_test.rs"]
mod paths_test;
