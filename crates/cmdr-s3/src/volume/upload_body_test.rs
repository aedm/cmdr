//! What a body hands the transport, and when it refuses to.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use cmdr_fs::ignore_poison::IgnorePoison;
use cmdr_fs::volume::liveness::Liveness;
use cmdr_fs::volume::{StreamLength, VolumeError, VolumeReadStream};
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use super::{BodyCounts, BodyStop, BodyStopSlot, buffered_body, streamed_body};

/// A source that yields exactly these pieces, then an optional error.
struct Pieces {
    pieces: Vec<Vec<u8>>,
    fail_after: bool,
}

impl VolumeReadStream for Pieces {
    fn next_chunk(&mut self) -> Pin<Box<dyn Future<Output = Option<Result<Vec<u8>, VolumeError>>> + Send + '_>> {
        Box::pin(async move {
            if self.pieces.is_empty() {
                if self.fail_after {
                    self.fail_after = false;
                    return Some(Err(VolumeError::IoError {
                        message: "the disk went away".to_string(),
                        raw_os_error: None,
                    }));
                }
                return None;
            }
            Some(Ok(self.pieces.remove(0)))
        })
    }

    fn total_size(&self) -> StreamLength {
        StreamLength::Unknown
    }

    fn bytes_read(&self) -> u64 {
        0
    }
}

fn pieces(sizes: &[usize]) -> Box<dyn VolumeReadStream> {
    Box::new(Pieces {
        pieces: sizes.iter().map(|&n| vec![7u8; n]).collect(),
        fail_after: false,
    })
}

/// Drains a body, answering what it handed over and whether it failed.
async fn drain(
    source: Box<dyn VolumeReadStream>,
    size: u64,
    stop: CancellationToken,
) -> (u64, bool, BodyCounts, BodyStopSlot) {
    let counts = BodyCounts::default();
    let stopped: BodyStopSlot = Arc::default();
    let mut body = streamed_body(
        source,
        size,
        counts.clone(),
        stop,
        Arc::clone(&stopped),
        Arc::new(Liveness::new()),
    );
    let mut sent = 0u64;
    let mut failed = false;
    while let Some(piece) = body.next().await {
        match piece {
            Ok(bytes) => sent += bytes.len() as u64,
            Err(_) => {
                failed = true;
                break;
            }
        }
    }
    (sent, failed, counts, stopped)
}

#[tokio::test]
async fn a_source_of_the_promised_length_goes_out_whole() {
    let (sent, failed, counts, _) = drain(pieces(&[6, 6]), 12, CancellationToken::new()).await;
    assert_eq!((sent, failed), (12, false));
    assert_eq!(counts.handed.load(Ordering::Relaxed), 12);
    assert!(counts.ended.load(Ordering::Relaxed));
}

/// ❗ The case the read-ahead exists for: a source whose pieces meet the
/// promise exactly and then keep going. The body fails BEFORE the last promised
/// byte goes out, so the request can't complete and S3 publishes nothing.
#[tokio::test]
async fn an_overlong_source_fails_the_body_before_its_last_promised_byte() {
    let (sent, failed, _, stopped) = drain(pieces(&[6, 6, 1]), 12, CancellationToken::new()).await;
    assert!(failed);
    assert!(sent < 12, "the request must not have its full length: sent {sent}");
    assert!(matches!(*stopped.lock_ignore_poison(), Some(BodyStop::Overlong)));
}

#[tokio::test]
async fn a_first_piece_past_the_promise_sends_nothing() {
    let (sent, failed, _, stopped) = drain(pieces(&[20]), 10, CancellationToken::new()).await;
    assert_eq!((sent, failed), (0, true));
    assert!(matches!(*stopped.lock_ignore_poison(), Some(BodyStop::Overlong)));
}

#[tokio::test]
async fn a_short_source_ends_the_body_and_says_it_ended() {
    let (sent, failed, counts, stopped) = drain(pieces(&[6]), 12, CancellationToken::new()).await;
    assert_eq!((sent, failed), (6, false));
    assert!(counts.ended.load(Ordering::Relaxed));
    assert_eq!(counts.fetched.load(Ordering::Relaxed), 6);
    assert!(stopped.lock_ignore_poison().is_none());
}

#[tokio::test]
async fn a_failing_source_fails_the_body_with_its_own_error() {
    let source = Box::new(Pieces {
        pieces: vec![vec![1; 4]],
        fail_after: true,
    });
    let (_, failed, _, stopped) = drain(source, 12, CancellationToken::new()).await;
    assert!(failed);
    assert!(matches!(*stopped.lock_ignore_poison(), Some(BodyStop::Source(_))));
}

#[tokio::test]
async fn a_cancelled_body_sends_nothing_more() {
    let stop = CancellationToken::new();
    stop.cancel();
    let (sent, failed, _, _) = drain(pieces(&[6, 6]), 12, stop).await;
    assert_eq!((sent, failed), (0, true));
}

#[tokio::test]
async fn a_buffered_body_goes_out_in_pieces_and_can_be_sent_again() {
    let bytes = bytes::Bytes::from(vec![3u8; 2 * 1024 * 1024 + 5]);
    for _ in 0..2 {
        let handed = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let pieces: Vec<_> = buffered_body(
            bytes.clone(),
            Arc::clone(&handed),
            CancellationToken::new(),
            Arc::new(Liveness::new()),
        )
        .collect()
        .await;
        assert_eq!(pieces.len(), 3);
        assert_eq!(handed.load(Ordering::Relaxed), bytes.len() as u64);
    }
}
