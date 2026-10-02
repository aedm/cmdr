//! A server-side copy in parts: up to the profile's `copy_concurrency()`
//! `UploadPartCopy` requests in flight, the window halved on every throttle and
//! grown back by one per landed part ([`Window`]), each part pinned to the
//! source's ETag. The upload around it (creation, completion, abort) is
//! `server_copy.rs`'s.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use cmdr_fs::volume::{ServerCopyProgress, VolumeError};
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use log::warn;

use super::S3Volume;
use super::multipart_upload::{retry_after, upload_refusal};
use super::query::body_error;
use super::server_copy::CopyFrom;
use super::writes::WriteTarget;
use crate::error::{S3Error, S3ErrorCode};
use crate::multipart::PartPlan;
use crate::ops::{self, CopySource};
use crate::transport::{COMPLETE_BUDGET, S3Client, map_transport_error};
use crate::xml::build::CompletedPart;
use crate::xml::parse_copy_result;

/// How many parts may be in flight: AIMD, halved on a throttle, one more per
/// part that lands, never below one or above the ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Window {
    pub width: usize,
    ceiling: usize,
}

impl Window {
    pub(super) fn new(ceiling: usize) -> Self {
        Self {
            width: ceiling.max(1),
            ceiling: ceiling.max(1),
        }
    }

    pub(super) fn landed(&mut self) {
        self.width = (self.width + 1).min(self.ceiling);
    }

    pub(super) fn throttled(&mut self) {
        self.width = (self.width / 2).max(1);
    }
}

/// One part's copy in flight.
type PartFuture = std::pin::Pin<Box<dyn Future<Output = Result<(CompletedPart, u64), PartFailure>> + Send>>;

/// How one part's copy failed, for the loop that decides what's next.
struct PartFailure {
    number: u32,
    attempt: u32,
    error: VolumeError,
    /// The server asked us to slow down: halve the window.
    throttle: bool,
    /// Worth sending again after a back-off.
    retryable: bool,
}

/// One part's copy, owning what it needs so it can run beside the others.
struct PartCopy {
    client: Arc<S3Client>,
    bucket: String,
    key: String,
    upload_id: String,
    number: u32,
    attempt: u32,
    source_bucket: String,
    source_key: String,
    range: (u64, u64),
    source_etag: Option<String>,
    /// The source's path, what a `SourceChanged` names.
    source_remote: String,
    remote: String,
    volume_id: String,
    /// Set when the server says the upload is gone (`NoSuchUpload`).
    gone: Arc<AtomicBool>,
    /// A back-off before sending, for a part sent again.
    delay: Option<Duration>,
}

impl PartCopy {
    async fn send(self) -> Result<(CompletedPart, u64), PartFailure> {
        if let Some(delay) = self.delay {
            tokio::time::sleep(delay).await;
        }
        let length = self.range.1 - self.range.0 + 1;
        let fail = |error: VolumeError, throttle: bool, retryable: bool| PartFailure {
            number: self.number,
            attempt: self.attempt,
            error,
            throttle,
            retryable,
        };
        let source = CopySource {
            bucket: &self.source_bucket,
            key: &self.source_key,
        };
        let request = ops::upload_part_copy(
            self.client.profile(),
            &self.bucket,
            &self.key,
            &self.upload_id,
            self.number,
            source,
            self.range,
            self.source_etag.as_deref(),
        )
        .map_err(|_| fail(VolumeError::NotFound(self.remote.clone()), false, false))?;
        let answer = match self.client.exchange(request, COMPLETE_BUDGET).await {
            Ok(answer) => answer,
            Err(e) => {
                return Err(fail(
                    map_transport_error(&e, &self.volume_id, &self.remote),
                    false,
                    true,
                ));
            }
        };
        if !answer.status.is_success() {
            let error = S3Error::from_response(answer.status, &answer.text());
            return Err(fail(self.refusal(&error), error.is_throttle(), error.is_retryable()));
        }
        // ❗ A part copy can fail inside a 200.
        match parse_copy_result(&answer.text()) {
            Ok(copied) => {
                let etag = copied.etag.ok_or_else(|| {
                    fail(
                        VolumeError::IoError {
                            message: format!("{}: part {} answered without an ETag", self.remote, self.number),
                            raw_os_error: None,
                        },
                        false,
                        false,
                    )
                })?;
                Ok((
                    CompletedPart {
                        number: self.number,
                        etag,
                    },
                    length,
                ))
            }
            Err(crate::xml::BodyError::Embedded(error)) => {
                Err(fail(self.refusal(&error), error.is_throttle(), error.is_retryable()))
            }
            Err(other) => Err(fail(body_error(&other, &self.remote), false, false)),
        }
    }

    fn refusal(&self, error: &S3Error) -> VolumeError {
        part_refusal(
            error,
            self.source_etag.is_some(),
            &self.source_remote,
            &self.remote,
            &self.gone,
        )
    }
}

/// A refused part copy in the `Volume` vocabulary. ❗ A failed precondition is
/// the source's ETag pin (the only precondition a part copy carries): the
/// source changed since its HEAD, ❌ never the destination being taken. So is
/// `InvalidRange`: every range comes from that HEAD's size, so the source
/// shrank, and on a provider that ignores the pin (Spaces, Hetzner) that's how
/// a smaller replacement shows.
pub(super) fn part_refusal(
    error: &S3Error,
    pinned: bool,
    source_remote: &str,
    remote: &str,
    gone: &AtomicBool,
) -> VolumeError {
    if (pinned && error.is_precondition_failed()) || error.code == S3ErrorCode::InvalidRange {
        return VolumeError::SourceChanged(source_remote.to_string());
    }
    upload_refusal(error, remote, gone)
}

impl S3Volume {
    /// Copies every part, up to the window's width at once, asking the
    /// progress hook's checkpoint before starting each one. Answers the parts
    /// in order. ❗ Leaves the upload for the caller to complete or abort.
    #[allow(
        clippy::too_many_arguments,
        reason = "one copy's whole context: the client, the source, the target, the upload id, the plan, progress, and the upload-gone flag"
    )]
    pub(super) async fn copy_parts(
        &self,
        client: &Arc<S3Client>,
        from: &CopyFrom<'_>,
        target: &WriteTarget<'_>,
        upload_id: &str,
        plan: PartPlan,
        progress: &dyn ServerCopyProgress,
        gone: &Arc<AtomicBool>,
    ) -> Result<Vec<CompletedPart>, VolumeError> {
        let part = |number: u32, attempt: u32, delay: Option<Duration>| PartCopy {
            client: Arc::clone(client),
            bucket: target.bucket.to_string(),
            key: target.key.to_string(),
            upload_id: upload_id.to_string(),
            number,
            attempt,
            source_bucket: from.bucket.to_string(),
            source_key: from.key.to_string(),
            range: plan.range(number),
            source_etag: from.facts.etag().map(str::to_string),
            source_remote: from.remote.to_string(),
            remote: target.remote.to_string(),
            volume_id: self.volume_id().to_string(),
            gone: Arc::clone(gone),
            delay,
        };
        let mut waiting: VecDeque<u32> = (1..=plan.part_count).collect();
        let mut in_flight: FuturesUnordered<PartFuture> = FuturesUnordered::new();
        let volume_id = self.volume_id();
        let mut window = Window::new(client.profile().copy_concurrency());
        let mut parts: Vec<CompletedPart> = Vec::with_capacity(plan.part_count as usize);
        let mut done_bytes = 0u64;
        loop {
            while in_flight.len() < window.width
                && let Some(&number) = waiting.front()
            {
                // ❗ A pause lands here, between parts, while the parts already
                // in flight finish: their requests keep being driven.
                let flow = {
                    let gate = progress.checkpoint();
                    tokio::pin!(gate);
                    loop {
                        tokio::select! {
                            biased;
                            flow = &mut gate => break flow,
                            Some(finished) = in_flight.next(), if !in_flight.is_empty() => {
                                let mut copy = CopyState { parts: &mut parts, done_bytes: &mut done_bytes, window: &mut window, in_flight: &mut in_flight };
                                settle_part(finished, &mut copy, &part, plan.total, progress, volume_id)?;
                            }
                        }
                    }
                };
                if flow.is_break() {
                    return Err(VolumeError::Cancelled(self.volume_id().to_string()));
                }
                waiting.pop_front();
                in_flight.push(Box::pin(part(number, 1, None).send()));
            }
            let Some(finished) = in_flight.next().await else {
                break;
            };
            let mut copy = CopyState {
                parts: &mut parts,
                done_bytes: &mut done_bytes,
                window: &mut window,
                in_flight: &mut in_flight,
            };
            settle_part(finished, &mut copy, &part, plan.total, progress, volume_id)?;
        }
        parts.sort_by_key(|part| part.number);
        Ok(parts)
    }
}

/// The copy loop's running state, as one settle step updates it.
struct CopyState<'a> {
    parts: &'a mut Vec<CompletedPart>,
    done_bytes: &'a mut u64,
    window: &'a mut Window,
    in_flight: &'a mut FuturesUnordered<PartFuture>,
}

/// Folds one finished part into the copy: a landed part moves the bar and
/// widens the window; a throttle halves it and sends the part again after a
/// back-off, as does any other retryable failure; anything else ends the copy.
fn settle_part(
    finished: Result<(CompletedPart, u64), PartFailure>,
    copy: &mut CopyState<'_>,
    part: &impl Fn(u32, u32, Option<Duration>) -> PartCopy,
    total: u64,
    progress: &dyn ServerCopyProgress,
    volume_id: &str,
) -> Result<(), VolumeError> {
    match finished {
        Ok((landed, length)) => {
            *copy.done_bytes += length;
            copy.parts.push(landed);
            copy.window.landed();
            if progress.advanced(*copy.done_bytes, total).is_break() {
                return Err(VolumeError::Cancelled(volume_id.to_string()));
            }
            Ok(())
        }
        Err(failure) if failure.retryable => {
            if failure.throttle {
                copy.window.throttled();
            }
            let Some(wait) = retry_after(failure.attempt) else {
                return Err(failure.error);
            };
            warn!(
                target: "volume",
                "s3: part {} of a server-side copy failed (attempt {}): {}; again in {wait:?}, {} in flight at most",
                failure.number, failure.attempt, failure.error, copy.window.width
            );
            copy.in_flight
                .push(Box::pin(part(failure.number, failure.attempt + 1, Some(wait)).send()));
            Ok(())
        }
        Err(failure) => Err(failure.error),
    }
}
