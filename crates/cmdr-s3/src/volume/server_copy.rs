//! Server-side copy inside one account: no byte travels through the Mac.
//!
//! - **Up to the part floor (64 MiB), one `CopyObject`**; past it, a multipart
//!   upload of `UploadPartCopy` ranges, even under S3's 5 GB `CopyObject`
//!   ceiling, so progress moves per part and a pause lands between parts. One
//!   part size per copy (`multipart.rs`, R2's rule).
//! - **The parts themselves** run in `part_copy.rs`: up to the profile's
//!   `copy_concurrency()` in flight, the window halved on every throttle.
//! - ❗ **A one-request copy needs no HEAD of its source**: its size and ETag
//!   come from the listing the engine just took (`listed.rs`), it goes by
//!   `COPY` (the server keeps the source's metadata and content headers), and
//!   it's pinned to that ETag with `x-amz-copy-source-if-match`.
//! - **A copy in parts HEADs its source once**: every part is pinned to its
//!   ETag, and the creation restates its metadata. It keeps the source's
//!   `x-amz-meta-mtime`; a source without one has its `Last-Modified` written
//!   as the mtime.
//! - ❗ **Parse every 200**: `CopyObject` and `UploadPartCopy` can fail inside
//!   one.
//! - **No-overwrite** follows the allowlist (`profile.rs`): R2's
//!   `cf-copy-destination-if-none-match` on `CopyObject`, else a HEAD first;
//!   VersityGW and Garage ignore `If-None-Match` on a copy. A multipart copy
//!   refuses at its completion, the way an upload does.
//! - **Cancel aborts the multipart upload**, recorded in the ledger before its
//!   first part and confirmed gone by listing (`multipart_upload.rs`).

use std::ops::ControlFlow;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use cmdr_fs::volume::{ServerCopyProgress, Volume, VolumeError, WriteMode};
use log::{debug, warn};

use super::S3Volume;
use super::errors::map_s3_error;
use super::listed::ListedObject;
use super::multipart_upload::abort_upload;
use super::paths::{Target, target_of};
use super::query::{body_error, stored_mtime};
use super::writes::{WriteTarget, normalize_etag, overwrite_for, refuse_unstorable};
use crate::error::S3Error;
use crate::metadata::{MTIME_HEADER, WRITE_TOKEN_HEADER};
use crate::multipart::{MAX_COPY_OBJECT_SIZE, PartPlan, TooLarge, plan_parts_with_floor};
use crate::ops::{self, BuildError, CopySource, ObjectMetadata};
use crate::profile::ConditionalOp;
use crate::transport::{Answer, COMPLETE_BUDGET, S3Client, map_transport_error};
use crate::xml::parse_copy_result;

/// The system headers a copy restates when it can't keep them by `COPY`.
const CARRIED_SYSTEM_HEADERS: [&str; 5] = [
    "content-type",
    "cache-control",
    "content-disposition",
    "content-encoding",
    "content-language",
];

/// What one HEAD said about a copy's source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SourceObject {
    pub size: u64,
    pub etag: Option<String>,
    /// Whether it carries its own `x-amz-meta-mtime`.
    pub has_mtime: bool,
    /// Its date: `x-amz-meta-mtime` when it parses, else `Last-Modified`.
    pub mtime: Option<SystemTime>,
    /// The headers a copy that restates metadata carries over.
    pub carried: Vec<(String, String)>,
}

impl SourceObject {
    /// Reads a source's HEAD.
    pub(super) fn from_head(head: &Answer) -> Self {
        let mut carried = Vec::new();
        for (name, value) in &head.headers {
            let name = name.as_str();
            let keep = CARRIED_SYSTEM_HEADERS.contains(&name)
                || (name.starts_with("x-amz-meta-") && name != MTIME_HEADER && name != WRITE_TOKEN_HEADER);
            if keep && let Ok(text) = value.to_str() {
                carried.push((name.to_string(), text.to_string()));
            }
        }
        Self {
            size: head.object_length().unwrap_or(0),
            etag: head.header("etag").map(str::to_string),
            has_mtime: head.header(MTIME_HEADER).is_some(),
            mtime: stored_mtime(head.header(MTIME_HEADER), head.header("last-modified")),
            carried,
        }
    }

    /// The metadata a copy in parts writes at `CreateMultipartUpload`, beside
    /// a token of its own: the source's date (its `x-amz-meta-mtime`, else its
    /// `Last-Modified`), its content headers, and its other user metadata,
    /// never another write's token.
    pub(super) fn restated(&self) -> ObjectMetadata {
        ObjectMetadata {
            mtime: self.mtime,
            write_token: None,
            carried: self.carried.clone(),
        }
    }
}

/// What a copy knows about its source: its listing's size and ETag, or its
/// HEAD, which a copy in parts needs for the metadata it restates.
pub(super) enum SourceFacts {
    Listed(ListedObject),
    Headed(SourceObject),
}

impl SourceFacts {
    pub(super) fn size(&self) -> u64 {
        match self {
            Self::Listed(listed) => listed.size,
            Self::Headed(object) => object.size,
        }
    }

    /// The version the copy pins (`x-amz-copy-source-if-match`) and proves a
    /// lost answer by.
    pub(super) fn etag(&self) -> Option<&str> {
        match self {
            Self::Listed(listed) => Some(&listed.etag),
            Self::Headed(object) => object.etag.as_deref(),
        }
    }
}

/// Where a copy reads from, resolved to a bucket, a key, and what's known of it.
pub(super) struct CopyFrom<'a> {
    pub bucket: &'a str,
    pub key: &'a str,
    /// The source's server-side path, what an error about it names.
    pub remote: &'a str,
    pub facts: SourceFacts,
}

impl S3Volume {
    /// The engine's server-side copy: `from` on `source` to `to` here, when
    /// `source` is a place of this same account and the provider can copy
    /// between the two buckets. `NotSupported` sends the engine to streaming.
    pub(super) async fn copy_on_server_impl(
        &self,
        source: &dyn Volume,
        from: &Path,
        to: &Path,
        mode: WriteMode,
        progress: &dyn ServerCopyProgress,
    ) -> Result<u64, VolumeError> {
        let Some(peer) = source.as_any().downcast_ref::<S3Volume>() else {
            return Err(VolumeError::NotSupported);
        };
        // ❗ One account (endpoint and key id): a copy names its source by
        // bucket and key, which mean something else on another account.
        if peer.inner.account() != self.inner.account() {
            return Err(VolumeError::NotSupported);
        }
        let from_remote = peer.to_remote_path(from)?;
        let to_remote = self.to_remote_path(to)?;
        let (
            Target::Key {
                bucket: from_bucket,
                key: from_key,
            },
            Target::Key {
                bucket: to_bucket,
                key: to_key,
            },
        ) = (target_of(&from_remote), target_of(&to_remote))
        else {
            return Err(VolumeError::IsADirectory(from_remote));
        };
        let client = self.clone_client().await?;
        refuse_unstorable(&client, to_key, &to_remote)?;
        if from_bucket != to_bucket && !client.profile().cross_bucket_copy() {
            // A provider that copies within one bucket only: stream it instead.
            return Err(VolumeError::NotSupported);
        }
        // The engine listed the source's folder (or stat'd the file) right
        // before, so its facts cost no HEAD here (`listed.rs`).
        let facts = match peer.take_listed(&from_remote) {
            Some(listed) => SourceFacts::Listed(listed),
            None => SourceFacts::Headed(self.head_source(&client, from_bucket, from_key, &from_remote).await?),
        };
        let copy_from = CopyFrom {
            bucket: from_bucket,
            key: from_key,
            remote: &from_remote,
            facts,
        };
        self.copy_key(&client, copy_from, to_bucket, to_key, &to_remote, mode, progress)
            .await
    }

    /// The source's HEAD, `NotFound` when nothing is at its key.
    async fn head_source(
        &self,
        client: &S3Client,
        bucket: &str,
        key: &str,
        remote: &str,
    ) -> Result<SourceObject, VolumeError> {
        let head = self
            .head_object(client, bucket, key, remote)
            .await?
            .ok_or_else(|| VolumeError::NotFound(remote.to_string()))?;
        Ok(SourceObject::from_head(&head))
    }

    /// Copies one object to `to_key` in `to_bucket`, whole or in parts by
    /// size, refusing an occupied key under `CreateNew`. Verified by a HEAD,
    /// like every write. Returns the bytes copied.
    ///
    /// ❗ Listed facts go stale: a one-request copy whose pin fails asks the
    /// source once and copies what's there now, and a copy in parts asks it
    /// for the metadata it restates anyway.
    #[allow(
        clippy::too_many_arguments,
        reason = "one copy's whole context: the client, the source, the destination's bucket, key, and path, the mode, and the progress hook"
    )]
    pub(super) async fn copy_key(
        &self,
        client: &Arc<S3Client>,
        mut from: CopyFrom<'_>,
        to_bucket: &str,
        to_key: &str,
        to_remote: &str,
        mode: WriteMode,
        progress: &dyn ServerCopyProgress,
    ) -> Result<u64, VolumeError> {
        loop {
            let size = from.facts.size();
            let in_parts = client.profile().copies_in_parts;
            if !in_parts && size > MAX_COPY_OBJECT_SIZE {
                // No `UploadPartCopy` (GCS), and too big for one `CopyObject`:
                // the engine streams it.
                return Err(VolumeError::NotSupported);
            }
            let listed = matches!(from.facts, SourceFacts::Listed(_));
            if self.copies_whole(size) || !in_parts {
                match self
                    .copy_whole(client, &from, to_bucket, to_key, to_remote, mode, progress)
                    .await
                {
                    Err(VolumeError::SourceChanged(_)) if listed => {}
                    outcome => return outcome,
                }
            } else if let SourceFacts::Headed(object) = &from.facts {
                let plan = plan_parts_with_floor(size, self.part_floor(), client.profile().short_tail).map_err(
                    |TooLarge| VolumeError::IoError {
                        message: format!("{to_remote}: too big for one S3 object (10,000 parts of 5 GiB)"),
                        raw_os_error: None,
                    },
                )?;
                // This copy's own token rides in the creation metadata, so a
                // completion whose answer is lost can still prove the
                // destination is ours and whole (`landed_whole`). The source's
                // token is never carried (`from_head`).
                let metadata = ObjectMetadata {
                    write_token: Some(crate::metadata::write_token()),
                    ..object.restated()
                };
                let target = WriteTarget {
                    bucket: to_bucket,
                    key: to_key,
                    remote: to_remote,
                    mode,
                    metadata: &metadata,
                };
                return self.copy_in_parts(client, &from, &target, plan, progress).await;
            }
            from.facts = SourceFacts::Headed(self.head_source(client, from.bucket, from.key, from.remote).await?);
        }
    }

    /// Whether an object of `size` copies in one `CopyObject`: up to the part
    /// floor. Past it, a copy runs in parts (with progress and pause), and a
    /// rename of it is the transfer engine's job (`RenameWork::CopyThenDelete`).
    pub(super) fn copies_whole(&self, size: u64) -> bool {
        size <= self.part_floor()
    }

    /// One `CopyObject`, by `COPY`: the server carries the source's metadata
    /// and content headers, so nothing is restated and no HEAD of the source
    /// is needed. Pinned to the source's known ETag.
    ///
    /// ❗ No write token: a copy whose answer is lost proves its landing by the
    /// result, the destination's size and ETag equal to the source's
    /// ([`Self::landed_after_all`]). The source's own `x-amz-meta-cmdr-write`
    /// rides along, harmless: a token is fresh per write, so it never matches
    /// a later write's.
    #[allow(
        clippy::too_many_arguments,
        reason = "one copy's whole context, as `copy_key` carries it"
    )]
    async fn copy_whole(
        &self,
        client: &S3Client,
        from: &CopyFrom<'_>,
        to_bucket: &str,
        to_key: &str,
        to_remote: &str,
        mode: WriteMode,
        progress: &dyn ServerCopyProgress,
    ) -> Result<u64, VolumeError> {
        let size = from.facts.size();
        let pin = from.facts.etag();
        let source = CopySource {
            bucket: from.bucket,
            key: from.key,
        };
        let metadata = ObjectMetadata::default();
        let target = WriteTarget {
            bucket: to_bucket,
            key: to_key,
            remote: to_remote,
            mode,
            metadata: &metadata,
        };
        let mut retried_without_header = false;
        loop {
            let built = match ops::copy_object(client.profile(), source, pin, to_bucket, to_key, overwrite_for(mode)) {
                Ok(built) => built,
                Err(BuildError::CrossBucketCopy) => return Err(VolumeError::NotSupported),
                Err(_) => return Err(VolumeError::NotFound(to_remote.to_string())),
            };
            let conditional = mode.refuses_occupied() && !built.check_first;
            if built.check_first {
                self.refuse_if_taken(client, &target).await?;
            }
            // The copy is what publishes: a pause waits here, a Cancel stops it.
            if progress.checkpoint().await.is_break() || progress.advanced(0, size).is_break() {
                return Err(VolumeError::Cancelled(self.volume_id().to_string()));
            }
            let answer = match client.exchange(built.request, COMPLETE_BUDGET).await {
                Ok(answer) => answer,
                Err(e) => {
                    let failure = map_transport_error(&e, self.volume_id(), to_remote);
                    return self
                        .landed_after_all(client, &target, from, progress)
                        .await
                        .ok_or(failure);
                }
            };
            if !answer.status.is_success() {
                let error = S3Error::from_response(answer.status, &answer.text());
                if conditional && error.is_not_implemented() && !retried_without_header {
                    client.profile().downgrade(ConditionalOp::Copy);
                    retried_without_header = true;
                    continue;
                }
                if pin.is_some()
                    && error.is_precondition_failed()
                    && self.source_moved_on(client, from, conditional).await
                {
                    return Err(VolumeError::SourceChanged(from.remote.to_string()));
                }
                let failure = map_s3_error(&error, to_remote);
                // A server fault may come after the copy applied (S3 says a
                // 500 can mean either).
                if error.is_retryable() {
                    return self
                        .landed_after_all(client, &target, from, progress)
                        .await
                        .ok_or(failure);
                }
                return Err(failure);
            }
            // ❗ A copy can fail inside a 200.
            let copied = parse_copy_result(&answer.text()).map_err(|e| body_error(&e, to_remote))?;
            let _ = progress.advanced(size, size);
            let ours = copied.etag.as_deref();
            let size = if client.profile().enforces_copy_source_pin || same_etag(ours, pin) {
                self.verify_landing(client, &target, size, ours).await?;
                size
            } else {
                // The pin may have been ignored and a newer source copied: what
                // the copy answered is the proof, at whatever size it holds.
                self.verify_landing_as_found(client, &target, size, ours).await?
            };
            debug!(target: "volume", "s3 copied {} bytes to {to_remote} in one request", size);
            return Ok(size);
        }
    }

    /// A `412` to a pinned copy names either the pin or the no-overwrite
    /// condition. Without the latter it's the pin; with both, one HEAD of the
    /// source says which: a source no longer at its pinned ETag moved on.
    async fn source_moved_on(&self, client: &S3Client, from: &CopyFrom<'_>, conditional: bool) -> bool {
        if !conditional {
            return true;
        }
        match self.head_object(client, from.bucket, from.key, from.remote).await {
            Ok(Some(head)) => !same_etag(head.header("etag"), from.facts.etag()),
            Ok(None) => true,
            Err(_) => false,
        }
    }

    /// After a `CopyObject` whose answer never came (or came as a fault): the
    /// source's size and ETag at the key mean the server applied it, so it's
    /// reported as copied. Even a file that was already there with those bytes
    /// is the result the copy wanted. Where the ETags can't match by design
    /// (a multipart source copied as one object gets a fresh ETag), this
    /// answers `None` and the copy reports its failure: a move keeps its
    /// source. One HEAD, ❌ never a delete.
    async fn landed_after_all(
        &self,
        client: &S3Client,
        target: &WriteTarget<'_>,
        from: &CopyFrom<'_>,
        progress: &dyn ServerCopyProgress,
    ) -> Option<u64> {
        let size = from.facts.size();
        let pin = from.facts.etag()?;
        let head = self
            .head_object(client, target.bucket, target.key, target.remote)
            .await
            .ok()
            .flatten()?;
        if head.object_length() != Some(size) || !same_etag(head.header("etag"), Some(pin)) {
            return None;
        }
        warn!(
            target: "volume",
            "s3: the answer to a copy into {} never came, but the server applied it",
            target.remote
        );
        self.remember_written(target, &head);
        let _ = progress.advanced(size, size);
        Some(size)
    }

    /// A multipart upload of `UploadPartCopy` ranges, recorded in the ledger
    /// before its first part and aborted on any failure or cancel.
    async fn copy_in_parts(
        &self,
        client: &Arc<S3Client>,
        from: &CopyFrom<'_>,
        target: &WriteTarget<'_>,
        plan: PartPlan,
        progress: &dyn ServerCopyProgress,
    ) -> Result<u64, VolumeError> {
        if target.mode.refuses_occupied()
            && client.profile().no_overwrite(ConditionalOp::CompleteMultipart)
                == crate::profile::NoOverwrite::CheckThenWrite
        {
            self.refuse_if_taken(client, target).await?;
        }
        if progress.checkpoint().await.is_break() {
            return Err(VolumeError::Cancelled(self.volume_id().to_string()));
        }
        let (upload_id, mut guard) = self.start_recorded_upload(client, target).await?;
        let gone = Arc::new(AtomicBool::new(false));
        let copied = self
            .copy_parts(client, from, target, &upload_id, plan, progress, &gone)
            .await;
        // ❗ The completion publishes, so a Cancel that came in while the last
        // part was in flight is honoured before it.
        let copied = copied.and_then(|parts| match progress.advanced(plan.total, plan.total) {
            ControlFlow::Break(()) => Err(VolumeError::Cancelled(self.volume_id().to_string())),
            ControlFlow::Continue(()) => Ok(parts),
        });
        let copied = match copied {
            Ok(parts) => self.source_unchanged(client, from).await.map(|()| parts),
            Err(e) => Err(e),
        };
        let outcome = match copied {
            Ok(parts) => match self.complete(client, target, &upload_id, &parts, &gone).await {
                Ok(etag) => Ok(etag),
                // ❗ The link can die after the server completed: our whole copy
                // is at the key and the upload is gone, so the copy landed.
                Err(e) => match self.landed_whole(client, target, plan.total).await {
                    Some(head) => {
                        self.inner.ledger.finished(&guard.upload);
                        guard.settled = true;
                        let _ = progress.advanced(plan.total, plan.total);
                        warn!(
                            target: "volume",
                            "s3: the answer to a copy into {} never came, but the server completed it",
                            target.remote
                        );
                        self.remember_written(target, &head);
                        return Ok(plan.total);
                    }
                    None => Err(e),
                },
            },
            Err(e) => Err(e),
        };
        match outcome {
            Ok(etag) => {
                self.inner.ledger.finished(&guard.upload);
                guard.settled = true;
                self.verify_landing(client, target, plan.total, etag.as_deref()).await?;
                debug!(
                    target: "volume",
                    "s3 copied {} bytes to {} in {} parts",
                    plan.total, target.remote, plan.part_count
                );
                Ok(plan.total)
            }
            Err(e) => {
                abort_upload(client, &self.inner.ledger, &guard.upload).await;
                guard.settled = true;
                if gone.load(Ordering::Relaxed)
                    && target.mode.refuses_occupied()
                    && matches!(
                        self.head_object(client, target.bucket, target.key, target.remote).await,
                        Ok(Some(_))
                    )
                {
                    return Err(VolumeError::AlreadyExists(target.remote.to_string()));
                }
                Err(e)
            }
        }
    }

    /// ❗ Where the provider ignores the parts' ETag pin (Hetzner, Spaces), a
    /// source replaced mid-copy would be stitched from two versions, and a
    /// move would then delete the new source. So right before the completion
    /// that publishes, one HEAD asks whether the source is still the version
    /// the copy started from; anything else is `SourceChanged`, and the caller
    /// aborts. One request per multipart copy, none where the pin holds. A
    /// replacement after this HEAD and before the completion stays blind.
    async fn source_unchanged(&self, client: &S3Client, from: &CopyFrom<'_>) -> Result<(), VolumeError> {
        if client.profile().enforces_copy_source_pin {
            return Ok(());
        }
        let now = self.head_object(client, from.bucket, from.key, from.remote).await?;
        let unchanged = match (&now, from.facts.etag()) {
            (None, _) => false,
            // A source that answered no ETag at the start has nothing to compare.
            (Some(_), None) => true,
            (Some(head), Some(then)) => head.header("etag").map(normalize_etag) == Some(normalize_etag(then)),
        };
        if unchanged {
            return Ok(());
        }
        warn!(target: "volume", "s3: {} changed during a copy; nothing was published", from.remote);
        Err(VolumeError::SourceChanged(from.remote.to_string()))
    }
}

/// Whether two ETags name one version, quotes and case aside.
fn same_etag(one: Option<&str>, other: Option<&str>) -> bool {
    matches!((one, other), (Some(one), Some(other)) if normalize_etag(one) == normalize_etag(other))
}

#[cfg(test)]
#[path = "server_copy_test.rs"]
mod server_copy_test;
