//! The bytes an upload sends, as the [`UploadBody`] the transport streams.
//!
//! Two sources: a single PUT streams straight from the copy's source
//! ([`streamed_body`]), and a multipart part streams from the buffer it was
//! filled into ([`buffered_body`]), so a part that fails can be sent again.
//!
//! ❗ **The streamed body reads one piece AHEAD**, the way
//! `crates/cmdr-webdav/src/volume/writes.rs` does, and here it buys more than an
//! honest error: a source that turns out LONGER than the `Content-Length` it
//! promised is caught before its last promised byte goes out, so the body
//! fails, hyper aborts the request, and S3 publishes nothing. Without it, hyper
//! stops polling once the length is met, and S3 would store a truncated object
//! under the user's name. Hence two counters that mean different things:
//! `fetched` (out of the source: the guard's number) and `handed` (onto the
//! wire: progress's number). ❌ Never collapse them.
//!
//! ❗ **The last piece waits for a go-ahead** ([`LastPieceAsk`]): the upload
//! asks its progress callback, which is where a user's Cancel arrives, before
//! the byte that would let S3 publish goes out. Without it, a cancel landing
//! between two progress ticks lost to a fast finish and published the object
//! the user had just called off.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use bytes::Bytes;
use cmdr_fs::ignore_poison::IgnorePoison;
use cmdr_fs::volume::liveness::Liveness;
use cmdr_fs::volume::{VolumeError, VolumeReadStream};
use tokio_util::sync::CancellationToken;

use crate::transport::UploadBody;

/// How much of a buffered part goes out per piece: small enough that progress
/// and the silence watch hear about it every few hundred milliseconds even on a
/// slow link.
const PIECE: usize = 1024 * 1024;

/// What a streamed body counts, shared with the upload that watches it.
#[derive(Clone, Default)]
pub(super) struct BodyCounts {
    /// Bytes pulled OUT of the source, the read-ahead piece included: the size
    /// guard's number, and the one a finished write returns.
    pub fetched: Arc<AtomicU64>,
    /// Bytes handed TO the transport: progress's number. A piece counts when it
    /// goes out, ❌ never when it's merely in hand, or the bar runs ahead of
    /// the wire.
    pub handed: Arc<AtomicU64>,
    /// Whether the source reached its own end, which tells a short source (its
    /// fault) from a connection cut mid-body (the server's).
    pub ended: Arc<AtomicBool>,
}

/// Why a streamed body stopped on its own side.
#[derive(Debug)]
pub(super) enum BodyStop {
    /// The source failed; its error is the one worth reporting.
    Source(VolumeError),
    /// The source had more bytes than the length the request promised.
    Overlong,
}

/// Where a streamed body leaves the reason it stopped, for the upload to read
/// once the request is over.
pub(super) type BodyStopSlot = Arc<std::sync::Mutex<Option<BodyStop>>>;

/// How a streamed body asks whether its last piece may go out: it sends a
/// reply slot, and the upload answers `true` to go on, `false` to stop. A
/// dropped slot or a closed channel is a stop.
pub(super) type LastPieceAsk = tokio::sync::mpsc::Sender<tokio::sync::oneshot::Sender<bool>>;

/// The read-ahead state behind [`streamed_body`].
struct StreamedSource {
    stream: Box<dyn VolumeReadStream>,
    size: u64,
    /// The next piece, already in hand. `None` with `primed` means the source
    /// is done.
    pending: Option<Vec<u8>>,
    primed: bool,
    counts: BodyCounts,
    stop: CancellationToken,
    stopped: BodyStopSlot,
    liveness: Arc<Liveness>,
    last_piece: LastPieceAsk,
}

/// Whether the upload lets the last piece go out.
async fn last_piece_may_go(ask: &LastPieceAsk) -> bool {
    let (reply, answer) = tokio::sync::oneshot::channel();
    if ask.send(reply).await.is_err() {
        return false;
    }
    answer.await.unwrap_or(false)
}

impl StreamedSource {
    /// Pulls one piece into `pending`, counting it as fetched the moment it's
    /// out of the source.
    async fn fetch(&mut self) -> Result<(), std::io::Error> {
        match self.stream.next_chunk().await {
            None => {
                self.counts.ended.store(true, Ordering::Relaxed);
                self.pending = None;
            }
            Some(Ok(chunk)) => {
                self.counts.fetched.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                self.pending = Some(chunk);
            }
            Some(Err(e)) => {
                let message = e.to_string();
                *self.stopped.lock_ignore_poison() = Some(BodyStop::Source(e));
                return Err(std::io::Error::other(message));
            }
        }
        Ok(())
    }
}

/// `stream` as the body of a request promising exactly `size` bytes.
///
/// Fails the body (which aborts the request, so nothing is published) when
/// `stop` is cancelled, when the source fails, when the source proves longer
/// than `size`, or when `last_piece` says no, and leaves the reason in
/// `stopped` when it's the source's. A source that ends SHORT simply ends the
/// body: hyper refuses to finish a request short of its `Content-Length`, and
/// `counts.ended` tells the upload whose fault that was.
pub(super) fn streamed_body(
    stream: Box<dyn VolumeReadStream>,
    size: u64,
    counts: BodyCounts,
    stop: CancellationToken,
    stopped: BodyStopSlot,
    liveness: Arc<Liveness>,
    last_piece: LastPieceAsk,
) -> UploadBody {
    let source = StreamedSource {
        stream,
        size,
        pending: None,
        primed: false,
        counts,
        stop,
        stopped,
        liveness,
        last_piece,
    };
    Box::pin(futures_util::stream::unfold(source, |mut source| async move {
        // A cancel is answered before anything is pulled or handed over, so a
        // cancelled upload never puts one more byte on the wire.
        if source.stop.is_cancelled() {
            return Some((Err(std::io::Error::other("cancelled")), source));
        }
        if !source.primed {
            source.primed = true;
            if let Err(failed) = source.fetch().await {
                return Some((Err(failed), source));
            }
        }
        // Nothing in hand after priming: the source is done, so is the body.
        let chunk = source.pending.take()?;
        // ❗ The read-ahead: ask for the next piece BEFORE handing this one
        // over, so an over-long source is on record while the request can
        // still be refused.
        if let Err(failed) = source.fetch().await {
            return Some((Err(failed), source));
        }
        if source.counts.fetched.load(Ordering::Relaxed) > source.size {
            *source.stopped.lock_ignore_poison() = Some(BodyStop::Overlong);
            return Some((Err(std::io::Error::other("the source is longer than promised")), source));
        }
        // ❗ The source has ended, so this is the piece that lets S3 publish:
        // only with the upload's go-ahead.
        let last = source.pending.is_none() && source.counts.ended.load(Ordering::Relaxed);
        if last && !last_piece_may_go(&source.last_piece).await {
            return Some((Err(std::io::Error::other("cancelled")), source));
        }
        source.counts.handed.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        // A piece handed over is the server draining the socket, so it's
        // there: a server that answers nothing until the body is in would
        // otherwise look silent for the whole upload.
        source.liveness.heard();
        Some((Ok(Bytes::from(chunk)), source))
    }))
}

/// `bytes` as a request body, a piece at a time, counting each piece handed
/// over into `handed`. Cheap to build again from the same `bytes` for a retry.
pub(super) fn buffered_body(
    bytes: Bytes,
    handed: Arc<AtomicU64>,
    stop: CancellationToken,
    liveness: Arc<Liveness>,
) -> UploadBody {
    Box::pin(futures_util::stream::unfold(bytes, move |mut rest| {
        let handed = Arc::clone(&handed);
        let stop = stop.clone();
        let liveness = Arc::clone(&liveness);
        async move {
            if stop.is_cancelled() {
                return Some((Err(std::io::Error::other("cancelled")), Bytes::new()));
            }
            if rest.is_empty() {
                return None;
            }
            let piece = rest.split_to(PIECE.min(rest.len()));
            handed.fetch_add(piece.len() as u64, Ordering::Relaxed);
            liveness.heard();
            Some((Ok(piece), rest))
        }
    }))
}

#[cfg(test)]
#[path = "upload_body_test.rs"]
mod upload_body_test;
