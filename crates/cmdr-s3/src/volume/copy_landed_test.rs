//! A one-request server-side copy (`CopyObject`): its source facts come from
//! the listing, it keeps the source's metadata by `COPY`, and a copy whose
//! answer was lost proves its landing by the result: the destination's size
//! and ETag equal the source's.
//!
//! Without that proof a lost answer was a transport failure: the engine fell
//! back to streaming the file, and the streamed write's no-overwrite check then
//! refused the name the copy itself had just taken, so the operation stopped
//! with `DestinationExists` on a fresh key (live, Hetzner, one 1,005-object
//! rename in six, 2026-10-02).

use std::future::Future;
use std::ops::ControlFlow;
use std::pin::Pin;
use std::time::Duration;

use cmdr_fs::volume::{ServerCopyProgress, Volume, VolumeError, WriteMode};

use super::S3Volume;
use super::fake_s3::FakeS3;
use crate::metadata::{MTIME_HEADER, WRITE_TOKEN_HEADER};
use crate::params::S3Provider;

struct Straight;

impl ServerCopyProgress for Straight {
    fn advanced(&self, _done: u64, _total: u64) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }

    fn checkpoint(&self) -> Pin<Box<dyn Future<Output = ControlFlow<()>> + Send + '_>> {
        Box::pin(async { ControlFlow::Continue(()) })
    }
}

fn source_meta() -> Vec<String> {
    vec![
        format!("{MTIME_HEADER}: 1354040105"),
        format!("{WRITE_TOKEN_HEADER}: an-earlier-write"),
    ]
}

fn seed_source(s3: &FakeS3, len: usize) {
    let meta = source_meta();
    let lines: Vec<&str> = meta.iter().map(String::as_str).collect();
    s3.seed_with_meta("src.txt", len, &lines);
}

async fn copy(volume: &S3Volume, mode: WriteMode) -> Result<u64, VolumeError> {
    volume
        .copy_on_server(
            volume,
            &volume.root().join("src.txt"),
            &volume.root().join("dst.txt"),
            mode,
            &Straight,
        )
        .await
}

/// What the engine does before it copies: it lists the folder.
async fn list(volume: &S3Volume) {
    volume.list_directory(volume.root(), None).await.expect("lists");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_one_request_copy_whose_answer_never_comes_reports_the_copy_it_published() {
    let s3 = FakeS3::start(Duration::ZERO).await;
    seed_source(&s3, 1_000);
    let volume = s3.volume();
    list(&volume).await;
    s3.hang_up_after_commit();

    let outcome = copy(&volume, WriteMode::CreateNew).await;

    assert_eq!(s3.object("dst.txt").map(|s| s.len), Some(1_000), "the copy landed");
    assert!(matches!(outcome, Ok(1_000)), "{outcome:?}");
}

/// The same on a provider that ignores the source pin and has no
/// no-overwrite header, the shape the Hetzner failure had.
#[tokio::test(flavor = "multi_thread")]
async fn the_lost_answer_proof_holds_on_hetzner() {
    let s3 = FakeS3::start(Duration::ZERO).await;
    seed_source(&s3, 1_000);
    s3.ignore_copy_source_pin();
    let volume = s3.volume_for(S3Provider::Hetzner {
        location: "fsn1".into(),
    });
    list(&volume).await;
    s3.hang_up_after_commit();

    let outcome = copy(&volume, WriteMode::CreateNewInFreshFolder).await;

    assert!(matches!(outcome, Ok(1_000)), "{outcome:?}");
}

/// Where the copy's ETag can't match the source's by design (a multipart
/// source copied as one object), the proof fails and the copy reports the
/// failure: a move keeps its source.
#[tokio::test(flavor = "multi_thread")]
async fn a_lost_answer_whose_etag_cant_match_reports_the_failure() {
    let s3 = FakeS3::start(Duration::ZERO).await;
    seed_source(&s3, 1_000);
    s3.fresh_copy_etags();
    let volume = s3.volume();
    list(&volume).await;
    s3.hang_up_after_commit();

    let outcome = copy(&volume, WriteMode::CreateNew).await;

    assert!(outcome.is_err(), "{outcome:?}");
    assert!(s3.object("src.txt").is_some(), "the source stays");
}

/// A listed source costs no HEAD: its size and ETag come from the listing.
#[tokio::test(flavor = "multi_thread")]
async fn a_listed_source_is_copied_without_a_head() {
    let s3 = FakeS3::start(Duration::ZERO).await;
    seed_source(&s3, 10);
    let volume = s3.volume();
    list(&volume).await;

    let outcome = copy(&volume, WriteMode::CreateNew).await;

    assert!(matches!(outcome, Ok(10)), "{outcome:?}");
    assert_eq!(s3.requests_about("src.txt"), 0, "no request named the source");
}

/// `COPY` keeps the source's metadata server-side: its date, and its own
/// write token, which never matches a new write's.
#[tokio::test(flavor = "multi_thread")]
async fn a_one_request_copy_keeps_the_sources_metadata() {
    let s3 = FakeS3::start(Duration::ZERO).await;
    seed_source(&s3, 10);
    let volume = s3.volume();
    list(&volume).await;

    let outcome = copy(&volume, WriteMode::CreateNew).await;

    assert!(matches!(outcome, Ok(10)), "{outcome:?}");
    assert_eq!(s3.object("dst.txt").expect("copied").meta, source_meta());
}

/// A source replaced since its listing fails the pin where the provider
/// enforces it; the copy asks the source once and copies what's there now.
#[tokio::test(flavor = "multi_thread")]
async fn a_source_replaced_since_its_listing_copies_the_current_version() {
    let s3 = FakeS3::start(Duration::ZERO).await;
    seed_source(&s3, 10);
    let volume = s3.volume();
    list(&volume).await;
    s3.replace("src.txt", 20);

    let outcome = copy(&volume, WriteMode::CreateNew).await;

    assert!(matches!(outcome, Ok(20)), "{outcome:?}");
    assert_eq!(s3.object("dst.txt").map(|s| s.len), Some(20));
}

/// Where the provider ignores the pin, the copy lands the current version and
/// its own ETag proves it.
#[tokio::test(flavor = "multi_thread")]
async fn a_source_replaced_since_its_listing_copies_the_current_version_on_hetzner() {
    let s3 = FakeS3::start(Duration::ZERO).await;
    seed_source(&s3, 10);
    s3.ignore_copy_source_pin();
    let volume = s3.volume_for(S3Provider::Hetzner {
        location: "fsn1".into(),
    });
    list(&volume).await;
    s3.replace("src.txt", 20);

    let outcome = copy(&volume, WriteMode::CreateNew).await;

    assert!(matches!(outcome, Ok(20)), "{outcome:?}");
    assert_eq!(s3.object("dst.txt").map(|s| s.len), Some(20));
}

/// R2's `412` names either condition; a source that still matches its pin
/// means the destination is taken.
#[tokio::test(flavor = "multi_thread")]
async fn an_r2_copy_onto_a_taken_name_is_already_exists() {
    let s3 = FakeS3::start(Duration::ZERO).await;
    seed_source(&s3, 10);
    s3.seed("dst.txt", 5);
    let volume = s3.volume();
    list(&volume).await;

    let outcome = copy(&volume, WriteMode::CreateNew).await;

    assert!(matches!(outcome, Err(VolumeError::AlreadyExists(_))), "{outcome:?}");
    assert_eq!(s3.object("dst.txt").map(|s| s.len), Some(5), "theirs stays");
}
