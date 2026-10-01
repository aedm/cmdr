//! Reading what's in a place without changing it: `ListBuckets` and
//! `ListObjectsV2` for a listing, `HeadObject` / `HeadBucket` for one entry.
//!
//! ❗ Every request here costs the user money, so a listing is the listing
//! calls and nothing else (❌ no HEAD per child), and a stat is one HEAD plus,
//! only when that finds no object, one bounded folder probe.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use cmdr_fs::entry::FileEntry;
use cmdr_fs::volume::{ListingProgress, VolumeError};
use tokio_util::sync::CancellationToken;

use super::S3Volume;
use super::errors::map_s3_error;
use super::listing::{Child, children_of};
use super::paths::{Target, child_of, target_of};
use crate::error::S3Error;
use crate::metadata::{MTIME_HEADER, parse_mtime};
use crate::ops::{self, ListObjectsParams};
use crate::request::S3Request;
use crate::transport::{Answer, QUERY_BUDGET, S3Client, map_transport_error};
use crate::xml::{BodyError, parse_list_buckets, parse_list_objects};

impl S3Volume {
    /// One request, in the `Volume` vocabulary: a transport failure by its
    /// typed predicates, a non-2xx by the S3 error table, both against `path`.
    async fn ask(&self, client: &S3Client, request: S3Request, path: &str) -> Result<Answer, VolumeError> {
        let answer = client
            .exchange(request, QUERY_BUDGET)
            .await
            .map_err(|e| map_transport_error(&e, self.volume_id(), path))?;
        if answer.status.is_success() {
            Ok(answer)
        } else {
            Err(map_s3_error(
                &S3Error::from_response(answer.status, &answer.text()),
                path,
            ))
        }
    }

    /// A folder's children, every page of them, reporting the running tally
    /// after each page.
    pub(super) async fn list_directory_impl(
        &self,
        path: &Path,
        on_progress: Option<&(dyn Fn(ListingProgress) + Sync)>,
        cancel: Option<&CancellationToken>,
    ) -> Result<Vec<FileEntry>, VolumeError> {
        let remote = self.to_remote_path(path)?;
        let client = self.clone_client().await?;
        let mut gathered = Gathered::default();
        match target_of(&remote) {
            Target::Account => {
                let mut token: Option<String> = None;
                loop {
                    let request = ops::list_buckets(client.profile(), token.as_deref());
                    let answer = self.ask(&client, request, &remote).await?;
                    let page = parse_list_buckets(&answer.text()).map_err(|e| body_error(&e, &remote))?;
                    for bucket in page.buckets {
                        let child = child_of(&remote, &bucket.name);
                        if let Some(entry) = self.folder_entry(&bucket.name, &child, bucket.created) {
                            gathered.add(entry);
                        }
                    }
                    gathered.report(on_progress);
                    token = page.continuation_token;
                    if token.is_none() {
                        break;
                    }
                    stop_if_cancelled(cancel, &remote)?;
                }
            }
            Target::Bucket(bucket) | Target::Key { bucket, .. } => {
                let prefix = match target_of(&remote) {
                    Target::Key { key, .. } => format!("{key}/"),
                    _ => String::new(),
                };
                // R2 answers in NFC, so the prefix it echoes is the composed one.
                let wire_prefix = client.profile().normalize_key(&prefix).into_owned();
                let mut token: Option<String> = None;
                // Whether the listing saw ANY key under the prefix, the folder's
                // own marker included: an S3 "folder" with none doesn't exist.
                let mut found = prefix.is_empty();
                loop {
                    let params = ListObjectsParams {
                        prefix: &prefix,
                        delimiter: Some("/"),
                        continuation_token: token.as_deref(),
                        max_keys: None,
                    };
                    let request = ops::list_objects(client.profile(), bucket, &params)
                        .map_err(|_| VolumeError::NotFound(remote.clone()))?;
                    let answer = self.ask(&client, request, &remote).await?;
                    let page = parse_list_objects(&answer.text()).map_err(|e| body_error(&e, &remote))?;
                    found |= !page.prefixes.is_empty() || !page.objects.is_empty();
                    for child in children_of(&page, &wire_prefix) {
                        if let Some(entry) = self.child_entry(&remote, child) {
                            gathered.add(entry);
                        }
                    }
                    gathered.report(on_progress);
                    token = page.next_continuation_token.filter(|_| page.is_truncated);
                    if token.is_none() {
                        break;
                    }
                    stop_if_cancelled(cancel, &remote)?;
                }
                if !found {
                    return Err(VolumeError::NotFound(remote));
                }
            }
        }
        stop_if_cancelled(cancel, &remote)?;
        Ok(gathered.entries)
    }

    /// One entry: the account root without a request, a bucket by
    /// `HeadBucket`, a key by `HeadObject`, and a key that isn't an object by
    /// one bounded listing of what's under it.
    pub(super) async fn get_metadata_impl(&self, path: &Path) -> Result<FileEntry, VolumeError> {
        let remote = self.to_remote_path(path)?;
        let not_here = || VolumeError::NotFound(path.to_string_lossy().into_owned());
        match target_of(&remote) {
            Target::Account => self.folder_entry(&self.name, &remote, None).ok_or_else(not_here),
            Target::Bucket(bucket) => {
                let client = self.clone_client().await?;
                let request = ops::head_bucket(client.profile(), bucket).map_err(|_| not_here())?;
                self.ask(&client, request, &remote).await?;
                self.folder_entry(bucket, &remote, None).ok_or_else(not_here)
            }
            Target::Key { bucket, key } => {
                let client = self.clone_client().await?;
                let name = key.rsplit('/').next().unwrap_or(key);
                let request = ops::head_object(client.profile(), bucket, key).map_err(|_| not_here())?;
                match self.ask(&client, request, &remote).await {
                    Ok(answer) => self.object_entry(name, &remote, &answer).ok_or_else(not_here),
                    Err(VolumeError::NotFound(_)) => {
                        if self.has_keys_under(&client, bucket, key, &remote).await? {
                            self.folder_entry(name, &remote, None).ok_or_else(not_here)
                        } else {
                            Err(not_here())
                        }
                    }
                    Err(other) => Err(other),
                }
            }
        }
    }

    /// Whether anything sits under `key/`: one `ListObjectsV2` capped at one
    /// key, which a folder marker answers too.
    async fn has_keys_under(
        &self,
        client: &S3Client,
        bucket: &str,
        key: &str,
        remote: &str,
    ) -> Result<bool, VolumeError> {
        let prefix = format!("{key}/");
        let params = ListObjectsParams {
            prefix: &prefix,
            delimiter: Some("/"),
            continuation_token: None,
            max_keys: Some(1),
        };
        let request = ops::list_objects(client.profile(), bucket, &params)
            .map_err(|_| VolumeError::NotFound(remote.to_string()))?;
        let answer = self.ask(client, request, remote).await?;
        let page = parse_list_objects(&answer.text()).map_err(|e| body_error(&e, remote))?;
        Ok(!page.prefixes.is_empty() || !page.objects.is_empty())
    }

    /// A listing child as a `FileEntry` at its app path.
    fn child_entry(&self, parent: &str, child: Child) -> Option<FileEntry> {
        let remote = child_of(parent, child.name());
        match child {
            Child::Folder { name } => self.folder_entry(&name, &remote, None),
            Child::Object {
                name, size, modified, ..
            } => {
                let app_path = self.root.to_app_path(&remote)?;
                let mut entry = FileEntry::new(name, app_path.to_string_lossy().into_owned(), false, false);
                entry.size = Some(size);
                entry.modified_at = modified.and_then(unix_secs);
                Some(entry)
            }
        }
    }

    /// A folder (a bucket, a prefix, the account root) at its app path.
    /// `created` is a bucket's creation date. `None` when a misbehaving server
    /// named something off this place's root.
    fn folder_entry(&self, name: &str, remote: &str, created: Option<SystemTime>) -> Option<FileEntry> {
        let app_path = self.root.to_app_path(remote)?;
        let mut entry = FileEntry::new(name.to_string(), app_path.to_string_lossy().into_owned(), true, false);
        entry.created_at = created.and_then(unix_secs);
        Some(entry)
    }

    /// An object from its HEAD: the size from `Content-Length`, the date from
    /// `x-amz-meta-mtime` (the source's own mtime, rclone's key and format)
    /// when it's there, else `Last-Modified` (the upload time).
    fn object_entry(&self, name: &str, remote: &str, head: &Answer) -> Option<FileEntry> {
        let app_path = self.root.to_app_path(remote)?;
        let mut entry = FileEntry::new(name.to_string(), app_path.to_string_lossy().into_owned(), false, false);
        entry.size = head.header("content-length").and_then(|length| length.parse().ok());
        entry.modified_at = modified_from_head(head.header(MTIME_HEADER), head.header("last-modified"));
        Some(entry)
    }
}

/// The modification date a HEAD answers with: `x-amz-meta-mtime` when it parses,
/// else `Last-Modified`, in Unix seconds.
pub(super) fn modified_from_head(mtime: Option<&str>, last_modified: Option<&str>) -> Option<u64> {
    mtime
        .and_then(parse_mtime)
        .or_else(|| last_modified.and_then(|text| httpdate::parse_http_date(text).ok()))
        .and_then(unix_secs)
}

/// Seconds since the epoch, or `None` for a date before it (S3 has none).
fn unix_secs(at: SystemTime) -> Option<u64> {
    at.duration_since(UNIX_EPOCH).ok().map(|since| since.as_secs())
}

/// A 2xx body that wasn't the document asked for, in the `Volume` vocabulary.
fn body_error(error: &BodyError, path: &str) -> VolumeError {
    match error {
        BodyError::Embedded(error) => map_s3_error(error, path),
        BodyError::Malformed => VolumeError::IoError {
            message: "the server answered 2xx with a body that isn't the S3 document asked for".to_string(),
            raw_os_error: None,
        },
    }
}

/// A listing's entries so far, and the tally the pane's "Loaded N files..."
/// readout shows.
#[derive(Default)]
struct Gathered {
    entries: Vec<FileEntry>,
    tally: ListingProgress,
}

impl Gathered {
    fn add(&mut self, entry: FileEntry) {
        if entry.is_directory {
            self.tally.dirs += 1;
        } else {
            self.tally.files += 1;
            self.tally.bytes += entry.size.unwrap_or(0);
        }
        self.entries.push(entry);
    }

    /// The running tally, after a page. ❗ Never per entry.
    fn report(&self, on_progress: Option<&(dyn Fn(ListingProgress) + Sync)>) {
        if let Some(on_progress) = on_progress {
            on_progress(self.tally);
        }
    }
}

/// Cancelled between pages: a listing is many requests, and each one costs.
fn stop_if_cancelled(cancel: Option<&CancellationToken>, remote: &str) -> Result<(), VolumeError> {
    if cancel.is_some_and(CancellationToken::is_cancelled) {
        return Err(VolumeError::Cancelled(remote.to_string()));
    }
    Ok(())
}

#[cfg(test)]
#[path = "query_test.rs"]
mod query_test;
