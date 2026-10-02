//! A Cancel that arrives after a PUT's whole body went out, against a fake S3
//! that commits the object and then answers slowly (R2 did, live, 2026-10-02:
//! `live_hostile_cancel_uploads`). The publish can't be taken back by then, so
//! the write must report the file it finished, ❌ never `Cancelled` over an
//! object that's already replaced, and ❌ never remove it as a "cut-off" PUT.

use std::ops::ControlFlow;
use std::time::Duration;

use cmdr_fs::volume::{Volume, VolumeError, VolumeReadStream, WriteMode};

use super::S3Volume;
use super::fake_s3::FakeS3;
use super::testing::BytesSource;

const MIB: usize = 1024 * 1024;

/// Writes `len` bytes to `key`, asking Cancel once every byte was handed over.
async fn write_cancelled_at_the_end(
    volume: &S3Volume,
    key: &str,
    mode: WriteMode,
    len: usize,
) -> Result<u64, VolumeError> {
    let source = BytesSource::new(vec![7u8; len]);
    let length = source.total_size();
    let total = len as u64;
    volume
        .write_from_stream(&volume.root().join(key), mode, length, Box::new(source), &|progress| {
            if progress.bytes_written >= total {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_after_the_whole_body_went_out_keeps_the_overwrite_it_finished() {
    let s3 = FakeS3::start(Duration::from_millis(700)).await;
    s3.seed("kept.bin", 32);
    let volume = s3.volume();
    let outcome = write_cancelled_at_the_end(&volume, "kept.bin", WriteMode::CreateOrReplace, 2 * MIB).await;
    let stored = s3.object("kept.bin");
    // Pre-fix: `Cancelled`, and the "cut-off" cleanup found our token on the
    // finished object and deleted it, so neither the original nor the new
    // bytes survived.
    assert_eq!(
        stored.map(|s| s.len),
        Some(2 * MIB),
        "the finished overwrite stays ({outcome:?})"
    );
    assert!(
        matches!(outcome, Ok(n) if n == (2 * MIB) as u64),
        "a publish that can't be taken back reports the file: {outcome:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_after_the_whole_body_went_out_keeps_the_new_file_it_finished() {
    let s3 = FakeS3::start(Duration::from_millis(700)).await;
    let volume = s3.volume();
    let outcome = write_cancelled_at_the_end(&volume, "fresh.bin", WriteMode::CreateOrReplace, 2 * MIB).await;
    assert!(matches!(outcome, Ok(n) if n == (2 * MIB) as u64), "{outcome:?}");
    assert_eq!(s3.object("fresh.bin").map(|s| s.len), Some(2 * MIB));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_before_the_last_piece_still_publishes_nothing() {
    let s3 = FakeS3::start(Duration::from_millis(700)).await;
    s3.seed("kept.bin", 32);
    let volume = s3.volume();
    let source = BytesSource::new(vec![7u8; 2 * MIB]);
    let length = source.total_size();
    let outcome = volume
        .write_from_stream(
            &volume.root().join("kept.bin"),
            WriteMode::CreateOrReplace,
            length,
            Box::new(source),
            &|progress| {
                if progress.bytes_written >= MIB as u64 {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            },
        )
        .await;
    assert!(matches!(outcome, Err(VolumeError::Cancelled(_))), "{outcome:?}");
    let stored = s3.object("kept.bin").expect("the original stays");
    assert_eq!((stored.len, stored.etag.as_str()), (32, "\"original\""));
}
