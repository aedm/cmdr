//! Writing an object: `write_from_stream` (a single streamed PUT, or the
//! multipart upload in `multipart_upload.rs`) and `create_file`.
//!
//! ❗ **Every write goes to its FINAL key.** S3 publishes an object only when
//! its PUT or `CompleteMultipartUpload` finishes, and a replaced object stays
//! readable until then, so `publishes_writes_whole` answers `true` and the
//! transfer engine sends no `.cmdr-tmp-*` staging here
//! (`write_operations/transfer/volume/DETAILS.md` § "Whole-publish
//! destinations"). A rename to land a temp would be a server-side copy plus a
//! delete.
//!
//! ❗ **`CreateNew` never clobbers silently.** Where the provider's
//! conditional header is allowlisted (`profile.rs`), the server refuses an
//! occupied key atomically (412 → `AlreadyExists`). Everywhere else the write
//! checks the key just before it starts (HEAD), and after every write a HEAD
//! compares what's there with what we sent ([`judge_landing`]): another
//! writer's object at the name after ours is reported as `AlreadyExists`. A
//! writer that lands BETWEEN our check and our write is the one window nothing
//! can see without bucket versioning; `DETAILS.md` § "No-overwrite writes".
//!
//! ❗ **A cut-off PUT is cleaned up after**, because not every server keeps
//! S3's promise to publish nothing short of `Content-Length`: VersityGW stores
//! whatever arrived before the connection dropped, under the user's name
//! (`apps/desktop/test/s3-servers/README.md`). So every PUT carries a token of
//! its own (`x-amz-meta-cmdr-write`), and a PUT that was cancelled or cut off
//! removes the object at its key ONLY when that object carries its token
//! ([`S3Volume::remove_cut_off_put`]). That covers a write to a free name; an
//! overwrite of an existing object there never goes as a PUT at all
//! ([`ShortStream::OnePart`]).

use std::ops::ControlFlow;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use cmdr_fs::entry::FileEntry;
use cmdr_fs::ignore_poison::IgnorePoison;
use cmdr_fs::pluralize::pluralize_grouped;
use cmdr_fs::volume::patching::patch_created;
use cmdr_fs::volume::{StreamLength, StreamWriteProgress, VolumeError, VolumeReadStream, WriteMode};
use log::{debug, warn};
use tokio_util::sync::CancellationToken;

use super::S3Volume;
use super::errors::map_s3_error;
use super::paths::{Target, target_of};
use super::upload_body::{BodyCounts, BodyStop, BodyStopSlot, streamed_body};
use crate::error::S3Error;
use crate::multipart::{PartPlan, ShortTail, TooLarge, plan_parts_with_floor};
use crate::ops::{self, ObjectMetadata, Overwrite};
use crate::request::{Body, S3Request};
use crate::transport::{Answer, QUERY_BUDGET, S3Client, map_transport_error};

/// How often a streamed upload reports progress while its body is in flight.
pub(super) const PROGRESS_TICK: Duration = Duration::from_millis(200);

/// How a write of a given length goes out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UploadShape {
    /// One streamed PUT of this many bytes.
    Single(u64),
    /// A multipart upload with this plan.
    Parts(PartPlan),
    /// A multipart upload of a stream whose length shows only at its end; a
    /// stream that ends inside the first part goes out as one PUT after all.
    Open,
}

/// What a write that fits one part does with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ShortStream {
    /// One PUT: three requests for a small file is waste.
    OnePut,
    /// A one-part multipart upload, which publishes nothing until its
    /// completion: an overwrite on a server that may publish a cut-off PUT.
    /// An empty stream still goes as one PUT, having nothing to cut short.
    OnePart,
}

/// The shape for `length`: one PUT when it fits one part with a short tail
/// folded in (fewer billed requests), else parts cut by the provider's `tail`
/// rule. `part_floor` is [`MIN_PART_SIZE`](crate::multipart::MIN_PART_SIZE)
/// in production.
pub(super) fn shape_for(length: StreamLength, part_floor: u64, tail: ShortTail) -> Result<UploadShape, TooLarge> {
    let Some(size) = length.known() else {
        return Ok(UploadShape::Open);
    };
    if plan_parts_with_floor(size, part_floor, ShortTail::Fold)?.part_count <= 1 {
        return Ok(UploadShape::Single(size));
    }
    Ok(UploadShape::Parts(plan_parts_with_floor(size, part_floor, tail)?))
}

/// What a HEAD after a write found at the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Landed {
    pub size: Option<u64>,
    pub etag: Option<String>,
}

/// What that HEAD proves about the write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Landing {
    /// Our object, at the size we sent.
    Verified,
    /// Another object holds the name, and the write was `CreateNew`: another
    /// writer took the name after ours landed. Theirs survives; ours is gone.
    TakenAfterUs,
    /// Another object holds the name after a `CreateOrReplace`: a newer write
    /// won, which is what any filesystem does.
    ReplacedAfterUs,
    /// Nothing is at the name.
    Vanished,
    /// Our object, at a size we didn't send.
    WrongSize { found: Option<u64> },
}

/// Judges a HEAD taken right after a write of `expected` bytes that the server
/// answered with ETag `ours`. ETags compare without quotes or case.
pub(super) fn judge_landing(expected: u64, ours: Option<&str>, found: Option<&Landed>, mode: WriteMode) -> Landing {
    let Some(found) = found else {
        return Landing::Vanished;
    };
    if let (Some(ours), Some(theirs)) = (ours, found.etag.as_deref())
        && normalize_etag(ours) != normalize_etag(theirs)
    {
        return match mode {
            WriteMode::CreateNew => Landing::TakenAfterUs,
            WriteMode::CreateOrReplace => Landing::ReplacedAfterUs,
        };
    }
    if found.size == Some(expected) {
        Landing::Verified
    } else {
        Landing::WrongSize { found: found.size }
    }
}

pub(super) fn normalize_etag(etag: &str) -> String {
    etag.trim().trim_matches('"').to_ascii_lowercase()
}

/// The overwrite a write mode asks of a conditional builder.
pub(super) fn overwrite_for(mode: WriteMode) -> Overwrite {
    match mode {
        WriteMode::CreateNew => Overwrite::Refuse,
        WriteMode::CreateOrReplace => Overwrite::Replace,
    }
}

/// The progress callback, with what it shows held monotonic: a part sent
/// again after a failure doesn't move the bar backwards.
pub(super) struct Progress<'a> {
    report: &'a (dyn Fn(StreamWriteProgress) -> ControlFlow<()> + Sync),
    expected: StreamLength,
    shown: AtomicU64,
}

impl<'a> Progress<'a> {
    pub(super) fn new(
        report: &'a (dyn Fn(StreamWriteProgress) -> ControlFlow<()> + Sync),
        expected: StreamLength,
    ) -> Self {
        Self {
            report,
            expected,
            shown: AtomicU64::new(0),
        }
    }

    /// Reports `sent` bytes (or the most shown so far, if more), answering
    /// whether the caller wants to stop.
    pub(super) fn at(&self, sent: u64) -> ControlFlow<()> {
        let shown = self.shown.fetch_max(sent, Ordering::Relaxed).max(sent);
        (self.report)(StreamWriteProgress {
            bytes_written: shown,
            expected_length: self.expected,
        })
    }
}

/// The source yielded a different byte count than it promised.
pub(super) fn size_mismatch(remote: &str, got: u64, size: u64) -> VolumeError {
    VolumeError::IoError {
        message: format!(
            "{remote}: the source promised {} and yielded {}",
            pluralize_grouped(size, "byte"),
            pluralize_grouped(got, "byte")
        ),
        raw_os_error: None,
    }
}

/// How many just-written objects' entries to keep for the pane patch that
/// follows each write.
const WRITTEN_CACHE_CAP: usize = 256;

impl S3Volume {
    /// Streams `stream` to `dest`'s key. Returns the bytes written.
    pub(super) async fn write_from_stream_impl(
        &self,
        dest: &Path,
        mode: WriteMode,
        length: StreamLength,
        stream: Box<dyn VolumeReadStream>,
        on_progress: &(dyn Fn(StreamWriteProgress) -> ControlFlow<()> + Sync),
    ) -> Result<u64, VolumeError> {
        let remote = self.to_remote_path(dest)?;
        let Target::Key { bucket, key } = target_of(&remote) else {
            return Err(VolumeError::IsADirectory(remote));
        };
        let client = self.clone_client().await?;
        let shape = shape_for(length, self.part_floor(), client.profile().short_tail).map_err(|TooLarge| {
            VolumeError::IoError {
                message: format!("{remote}: too big for one S3 object (10,000 parts of 5 GiB)"),
                raw_os_error: None,
            }
        })?;
        // The source file's own date, carried as `x-amz-meta-mtime`, and this
        // write's token, so a cut-off PUT can find its own leftover.
        let metadata = ObjectMetadata {
            mtime: stream.modified_at(),
            write_token: Some(crate::metadata::write_token()),
            carried: Vec::new(),
        };
        let progress = Progress::new(on_progress, length);
        let target = WriteTarget {
            bucket,
            key,
            remote: &remote,
            mode,
            metadata: &metadata,
        };
        debug!(target: "volume", "s3 write {remote} ({shape:?}, {mode:?})");
        // ❗ A PUT this server might publish short must not land on the
        // original: an overwrite goes as a multipart upload, which publishes
        // nothing until its completion (`DETAILS.md` § "Overwrites in parts").
        let short = if self.overwrites_in_parts(&client, &target, shape).await? {
            ShortStream::OnePart
        } else {
            ShortStream::OnePut
        };
        match (shape, short) {
            (UploadShape::Single(size), ShortStream::OnePut) => {
                self.put_streamed(&client, &target, size, stream, &progress).await
            }
            (UploadShape::Single(size), ShortStream::OnePart) => {
                self.upload_in_parts(&client, &target, Some(PartPlan::whole(size)), short, stream, &progress)
                    .await
            }
            (UploadShape::Parts(plan), _) => {
                self.upload_in_parts(&client, &target, Some(plan), short, stream, &progress)
                    .await
            }
            (UploadShape::Open, _) => {
                self.upload_in_parts(&client, &target, None, short, stream, &progress)
                    .await
            }
        }
    }

    /// Whether a write that would go as one PUT has to go as a multipart
    /// upload instead: a `CreateOrReplace` over an existing object (one HEAD to
    /// find out) on a provider not trusted to refuse a short body. VersityGW
    /// publishes a PUT cut off mid-body, which would replace the original with
    /// a truncated object. An empty body has nothing to cut short.
    async fn overwrites_in_parts(
        &self,
        client: &S3Client,
        target: &WriteTarget<'_>,
        shape: UploadShape,
    ) -> Result<bool, VolumeError> {
        let may_go_as_one_put = match shape {
            UploadShape::Single(size) => size > 0,
            UploadShape::Open => true,
            UploadShape::Parts(_) => false,
        };
        if target.mode != WriteMode::CreateOrReplace || !may_go_as_one_put || client.profile().refuses_short_body {
            return Ok(false);
        }
        Ok(self
            .head_object(client, target.bucket, target.key, target.remote)
            .await?
            .is_some())
    }

    /// One streamed PUT of exactly `size` bytes, with progress and cancel.
    /// ❗ A body that ends short, runs long, fails, or is cancelled aborts the
    /// request, and S3 publishes nothing: there's never a partial to clean.
    pub(super) async fn put_streamed(
        &self,
        client: &S3Client,
        target: &WriteTarget<'_>,
        size: u64,
        stream: Box<dyn VolumeReadStream>,
        progress: &Progress<'_>,
    ) -> Result<u64, VolumeError> {
        let built = ops::put_object(
            client.profile(),
            target.bucket,
            target.key,
            size,
            overwrite_for(target.mode),
            target.metadata,
        )
        .map_err(|_| VolumeError::NotFound(target.remote.to_string()))?;
        let conditional = target.mode == WriteMode::CreateNew && !built.check_first;
        if built.check_first {
            self.refuse_if_taken(client, target).await?;
        }
        let counts = BodyCounts::default();
        let stop = CancellationToken::new();
        let stopped: BodyStopSlot = std::sync::Arc::default();
        let (last_piece, mut asks) = tokio::sync::mpsc::channel(1);
        let body = streamed_body(
            stream,
            size,
            counts.clone(),
            stop.clone(),
            std::sync::Arc::clone(&stopped),
            std::sync::Arc::clone(client.liveness()),
            last_piece,
        );
        // The block scopes the in-flight request: leaving it drops the request,
        // which is what stops a cancelled upload on the wire.
        let sent = {
            let put = client.upload(built.request, body);
            let mut put = std::pin::pin!(put);
            let mut tick = tokio::time::interval(PROGRESS_TICK);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    sent = &mut put => break Some(sent),
                    // The body's last piece waits here: a Cancel that came in
                    // since the last tick stops it before S3 can publish.
                    Some(reply) = asks.recv() => {
                        let go = progress.at(counts.handed.load(Ordering::Relaxed).min(size)).is_continue();
                        let _ = reply.send(go);
                        if !go {
                            stop.cancel();
                            break None;
                        }
                    }
                    _ = tick.tick() => {
                        if progress.at(counts.handed.load(Ordering::Relaxed).min(size)).is_break() {
                            stop.cancel();
                            break None;
                        }
                    }
                }
            }
        };
        let fetched = counts.fetched.load(Ordering::Relaxed);
        let answer = match sent {
            None => {
                self.remove_cut_off_put(client, target).await;
                return Err(VolumeError::Cancelled(self.volume_id().to_string()));
            }
            Some(Err(e)) => {
                self.remove_cut_off_put(client, target).await;
                return Err(match stopped.lock_ignore_poison().take() {
                    Some(BodyStop::Source(source)) => source,
                    Some(BodyStop::Overlong) => size_mismatch(target.remote, fetched, size),
                    // ❗ A short source ends the request from OUR side, which
                    // `reqwest` reports the way a dropped connection is: ❌ not
                    // the server's fault, so not `DeviceDisconnected`.
                    None if counts.ended.load(Ordering::Relaxed) && fetched != size => {
                        size_mismatch(target.remote, fetched, size)
                    }
                    None => map_transport_error(&e, self.volume_id(), target.remote),
                });
            }
            Some(Ok(answer)) => answer,
        };
        if !answer.status.is_success() {
            let error = S3Error::from_response(answer.status, &answer.text());
            self.note_refused_condition(client, &error, crate::profile::ConditionalOp::Put, conditional);
            return Err(map_s3_error(&error, target.remote));
        }
        // Published: a cancel this late can't take it back, and the bytes are
        // whole, so the final report's answer changes nothing.
        let _ = progress.at(size);
        self.verify_landing(client, target, size, answer.header("etag")).await?;
        Ok(size)
    }

    /// After a PUT that was cancelled or cut off mid-body: removes the object
    /// at the key when it carries THIS write's token, which is a server keeping
    /// a truncated body against S3's contract (VersityGW does). ❌ Anything
    /// else at the key is left alone: it's the original, or another writer's.
    ///
    /// The server stores the cut-off body once it notices the dropped
    /// connection, which may be a moment after the drop, so a miss is checked
    /// again twice, 150 ms apart. On a server that keeps the contract this
    /// costs one HEAD that finds nothing, plus two after short waits, and only
    /// for a PUT that didn't finish.
    pub(super) async fn remove_cut_off_put(&self, client: &S3Client, target: &WriteTarget<'_>) {
        let Some(token) = target.metadata.write_token.as_deref() else {
            return;
        };
        for attempt in 0..3 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
            let Ok(head) = self.head_object(client, target.bucket, target.key, target.remote).await else {
                return;
            };
            let Some(head) = head else {
                continue;
            };
            if head.header(crate::metadata::WRITE_TOKEN_HEADER) != Some(token) {
                // The original, or another writer's: never ours to remove.
                return;
            }
            match self.delete_key(client, target.bucket, target.key, target.remote).await {
                Ok(()) => warn!(
                    target: "volume",
                    "s3: the server kept a cut-off upload of {}; removed it",
                    target.remote
                ),
                Err(e) => warn!(
                    target: "volume",
                    "s3: the server kept a cut-off upload of {} and removing it failed: {e}",
                    target.remote
                ),
            }
            return;
        }
    }

    /// `CreateNew` on a provider with no trusted conditional header: refuse a
    /// key that's taken before sending anything.
    pub(super) async fn refuse_if_taken(&self, client: &S3Client, target: &WriteTarget<'_>) -> Result<(), VolumeError> {
        match self
            .head_object(client, target.bucket, target.key, target.remote)
            .await?
        {
            Some(_) => Err(VolumeError::AlreadyExists(target.remote.to_string())),
            None => Ok(()),
        }
    }

    /// One `HeadObject`, `None` when nothing is at the key.
    pub(super) async fn head_object(
        &self,
        client: &S3Client,
        bucket: &str,
        key: &str,
        remote: &str,
    ) -> Result<Option<Answer>, VolumeError> {
        let request =
            ops::head_object(client.profile(), bucket, key).map_err(|_| VolumeError::NotFound(remote.to_string()))?;
        match self.ask(client, request, remote).await {
            Ok(answer) => Ok(Some(answer)),
            Err(VolumeError::NotFound(_)) => Ok(None),
            Err(other) => Err(other),
        }
    }

    /// A 501 to a request that carried an allowlisted conditional header
    /// (`conditional`): the server doesn't honour it, so that operation checks
    /// then writes for the rest of the session (`ProviderProfile::downgrade`).
    pub(super) fn note_refused_condition(
        &self,
        client: &S3Client,
        error: &S3Error,
        op: crate::profile::ConditionalOp,
        conditional: bool,
    ) {
        if conditional && error.is_not_implemented() {
            client.profile().downgrade(op);
        }
    }

    /// The HEAD after every write: is what's at the key what we sent?
    /// `ours` is the ETag the write answered with.
    pub(super) async fn verify_landing(
        &self,
        client: &S3Client,
        target: &WriteTarget<'_>,
        size: u64,
        ours: Option<&str>,
    ) -> Result<(), VolumeError> {
        let head = self
            .head_object(client, target.bucket, target.key, target.remote)
            .await?;
        let landed = head.as_ref().map(|answer| Landed {
            size: answer.header("content-length").and_then(|length| length.parse().ok()),
            etag: answer.header("etag").map(str::to_string),
        });
        match judge_landing(size, ours, landed.as_ref(), target.mode) {
            Landing::Verified => {
                if let Some(answer) = &head {
                    self.remember_written(target, answer);
                }
                Ok(())
            }
            Landing::TakenAfterUs => {
                warn!(
                    target: "volume",
                    "s3: another writer's object took {} right after ours landed; theirs stays",
                    target.remote
                );
                Err(VolumeError::AlreadyExists(target.remote.to_string()))
            }
            Landing::ReplacedAfterUs => {
                warn!(target: "volume", "s3: a newer write replaced {} right after ours", target.remote);
                Ok(())
            }
            Landing::Vanished => Err(VolumeError::IoError {
                message: format!("{}: the object was gone right after it was written", target.remote),
                raw_os_error: None,
            }),
            Landing::WrongSize { found } => Err(VolumeError::IoError {
                message: format!(
                    "{}: the server holds {found:?} bytes after a write of {size}",
                    target.remote
                ),
                raw_os_error: None,
            }),
        }
    }

    /// Keeps the verified object's entry for the pane patch the engine asks
    /// for next (`notify_mutation`), so that patch costs no second HEAD.
    fn remember_written(&self, target: &WriteTarget<'_>, head: &Answer) {
        let name = target.key.rsplit('/').next().unwrap_or(target.key);
        if let Some(entry) = self.object_entry(name, target.remote, head) {
            let mut written = self.inner.written.lock_ignore_poison();
            if written.len() >= WRITTEN_CACHE_CAP {
                written.clear();
            }
            written.insert(target.remote.to_string(), entry);
        }
    }

    /// The entry a write just verified at `remote`, taken out of the cache.
    pub(super) fn take_written(&self, remote: &str) -> Option<FileEntry> {
        self.inner.written.lock_ignore_poison().remove(remote)
    }

    /// New File: a small PUT that only lands on a name nothing holds, a file
    /// or a folder.
    pub(super) async fn create_file_impl(&self, path: &Path, content: &[u8]) -> Result<(), VolumeError> {
        let remote = self.to_remote_path(path)?;
        let Target::Key { bucket, key } = target_of(&remote) else {
            return Err(VolumeError::AlreadyExists(remote));
        };
        let client = self.clone_client().await?;
        // A folder of that name holds it too, and an object beside it would
        // hide under the folder in every listing.
        if self.has_keys_under(&client, bucket, key, &remote).await? {
            return Err(VolumeError::AlreadyExists(remote));
        }
        let metadata = ObjectMetadata::default();
        let target = WriteTarget {
            bucket,
            key,
            remote: &remote,
            mode: WriteMode::CreateNew,
            metadata: &metadata,
        };
        let built = ops::put_object(
            client.profile(),
            bucket,
            key,
            content.len() as u64,
            Overwrite::Refuse,
            &metadata,
        )
        .map_err(|_| VolumeError::NotFound(remote.clone()))?;
        let conditional = !built.check_first;
        if built.check_first {
            self.refuse_if_taken(&client, &target).await?;
        }
        let mut request: S3Request = built.request;
        request.body = Body::Bytes(content.to_vec());
        let answer = client
            .exchange(request, QUERY_BUDGET)
            .await
            .map_err(|e| map_transport_error(&e, self.volume_id(), &remote))?;
        if !answer.status.is_success() {
            let error = S3Error::from_response(answer.status, &answer.text());
            self.note_refused_condition(&client, &error, crate::profile::ConditionalOp::Put, conditional);
            return Err(map_s3_error(&error, &remote));
        }
        self.verify_landing(&client, &target, content.len() as u64, answer.header("etag"))
            .await?;
        patch_created(self, path).await;
        Ok(())
    }
}

/// Where a write goes and how it may treat what's there.
pub(super) struct WriteTarget<'a> {
    pub bucket: &'a str,
    pub key: &'a str,
    /// The server-side path, what every error names.
    pub remote: &'a str,
    pub mode: WriteMode,
    pub metadata: &'a ObjectMetadata,
}

#[cfg(test)]
#[path = "writes_test.rs"]
mod writes_test;
