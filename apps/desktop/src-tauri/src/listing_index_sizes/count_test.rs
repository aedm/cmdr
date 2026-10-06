//! Calculating folder sizes on demand: which folders a count walks, what the
//! pane hears while it walks, and how it ends.

use std::sync::{Arc, Mutex};

use cmdr_fs::volume::InMemoryVolume;

use super::ListingIndexSizesChanged;
use super::count::{CountFolderSizesError, cancel, count_with, plan, register};
use crate::file_system::listing::caching_test_support::{TestListing, TestListingGuard};
use crate::file_system::listing::metadata::FileEntry;
use crate::file_system::volume::Volume;

const DIR: &str = "/data";

fn file(path: &str, size: u64) -> FileEntry {
    let name = path.rsplit('/').next().expect("a path has a name").to_string();
    FileEntry {
        size: Some(size),
        ..FileEntry::new(name, path.to_string(), false, false)
    }
}

fn folder(path: &str) -> FileEntry {
    let name = path.rsplit('/').next().expect("a path has a name").to_string();
    FileEntry::new(name, path.to_string(), true, false)
}

/// A pane on `/data` showing two folders and a file.
fn pane(tag: &str, rows: Vec<FileEntry>) -> TestListingGuard {
    TestListing::new()
        .path(DIR)
        .include_hidden(false)
        .entries(rows)
        .insert(tag)
}

fn volume() -> Arc<dyn Volume> {
    Arc::new(InMemoryVolume::with_entries(
        "test",
        vec![
            folder("/data"),
            folder("/data/photos"),
            file("/data/photos/a.jpg", 100),
            file("/data/photos/b.jpg", 200),
            folder("/data/photos/raw"),
            file("/data/photos/raw/c.cr3", 1_000),
            folder("/data/empty"),
            file("/data/notes.txt", 5),
        ],
    ))
}

fn rows() -> Vec<FileEntry> {
    vec![
        folder("/data/empty"),
        file("/data/notes.txt", 5),
        folder("/data/photos"),
    ]
}

fn collect() -> (
    Arc<Mutex<Vec<ListingIndexSizesChanged>>>,
    impl Fn(ListingIndexSizesChanged) + Sync,
) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    (events, move |event| sink_events.lock().expect("test lock").push(event))
}

#[test]
fn a_count_walks_every_folder_row_without_an_exact_size() {
    let mut known = folder("/data/known");
    known.recursive_size = Some(9);
    known.recursive_size_complete = Some(true);
    let mut lower_bound = folder("/data/partial");
    lower_bound.recursive_size = Some(9);
    lower_bound.recursive_size_complete = Some(false);
    let mut link = folder("/data/link");
    link.is_symlink = true;
    let listing = pane(
        "count-plan",
        vec![known, lower_bound, link, folder("/data/new"), file("/data/f.txt", 1)],
    );

    let plan = plan(listing.id(), false, None, |_| false).expect("listing is cached");

    assert_eq!(
        plan.folders,
        vec!["/data/partial", "/data/new"],
        "in the pane's row order"
    );
    assert_eq!(plan.dir.to_string_lossy(), DIR);
}

#[test]
fn space_on_a_folder_recounts_just_that_folder() {
    let mut known = folder("/data/known");
    known.recursive_size = Some(9);
    known.recursive_size_complete = Some(true);
    let listing = pane("count-plan-only", vec![known, folder("/data/other")]);

    let plan = plan(listing.id(), false, Some(&["/data/known".to_string()]), |_| false).expect("listing is cached");

    assert_eq!(plan.folders, vec!["/data/known"]);
}

#[test]
fn a_gone_listing_is_refused() {
    assert_eq!(
        plan("no-such-listing", false, None, |_| false),
        Err(CountFolderSizesError::Gone {
            listing_id: "no-such-listing".to_string()
        })
    );
}

#[tokio::test]
async fn each_folder_lands_with_its_exact_size_in_the_cache_and_the_pane() {
    let listing = pane("count-run", rows());
    let (events, sink) = collect();

    let outcome = count_with(
        listing.id(),
        volume(),
        &["/data/photos".to_string(), "/data/empty".to_string()],
        &sink,
        &register(listing.id()),
    )
    .await;

    assert_eq!(outcome.counted, 2);
    assert!(!outcome.cancelled);

    let photos = listing
        .entries()
        .into_iter()
        .find(|e| e.name == "photos")
        .expect("row is there");
    assert_eq!(photos.recursive_size, Some(1_300));
    assert_eq!(photos.recursive_file_count, Some(3));
    assert_eq!(photos.recursive_size_complete, Some(true));

    // A walk shorter than the progress interval sends its exact size and nothing
    // before it: no flash of "≥ 0" with an hourglass.
    let events = events.lock().expect("test lock");
    let photos_events: Vec<_> = events.iter().filter(|e| e.folders[0].path == "/data/photos").collect();
    assert_eq!(photos_events.len(), 1);
    let stats = photos_events[0].folders[0].stats.as_ref().expect("a reading");
    assert!(!stats.recursive_size_pending && stats.recursive_size_complete);
    assert_eq!(stats.recursive_size, 1_300);
}

#[tokio::test]
async fn a_folder_the_pane_no_longer_shows_is_neither_sent_nor_counted() {
    let listing = pane("count-unreadable", rows());
    let (events, sink) = collect();

    let outcome = count_with(
        listing.id(),
        volume(),
        &["/data/missing".to_string(), "/data/photos".to_string()],
        &sink,
        &register(listing.id()),
    )
    .await;

    assert_eq!(outcome.counted, 1, "photos still counted");
    assert!(
        events
            .lock()
            .expect("test lock")
            .iter()
            .all(|e| e.folders[0].path != "/data/missing"),
        "a folder the pane doesn't show is never sent"
    );
}

#[tokio::test]
async fn a_cancelled_count_stops_and_says_so() {
    let listing = pane("count-cancel", rows());
    let listing_id = listing.id().to_string();
    let sink = move |_: ListingIndexSizesChanged| {
        cancel(&listing_id);
    };

    let outcome = count_with(
        listing.id(),
        volume(),
        &["/data/photos".to_string(), "/data/empty".to_string()],
        &sink,
        &register(listing.id()),
    )
    .await;

    assert!(outcome.cancelled);
    assert!(outcome.counted < 2, "it stopped before the end");
    assert!(!cancel(listing.id()), "a finished count is no longer running");
}

#[test]
fn on_an_indexed_volume_only_folders_the_index_says_nothing_about_are_walked() {
    let mut scanning = folder("/data/scanning");
    scanning.recursive_size = Some(9);
    scanning.recursive_size_complete = Some(false);
    let listing = pane("count-plan-indexed", vec![scanning, folder("/data/excluded")]);

    let plan = plan(listing.id(), false, None, |_| true).expect("listing is cached");

    assert_eq!(
        plan.folders,
        vec!["/data/excluded"],
        "the index finishes its own lower bound"
    );
}

#[tokio::test]
async fn a_superseded_count_leaves_the_row_to_its_successor() {
    let listing = pane("count-supersede", rows());
    let first = register(listing.id());
    let _second = register(listing.id());
    let (events, sink) = collect();

    let outcome = count_with(listing.id(), volume(), &["/data/photos".to_string()], &sink, &first).await;

    assert!(outcome.cancelled);
    assert!(
        events.lock().expect("test lock").is_empty(),
        "nothing written over the newer count"
    );
}

#[tokio::test]
async fn a_count_stops_when_its_pane_moves_on() {
    let listing = pane("count-gone", rows());
    let id = listing.id().to_string();
    let stop = register(&id);
    drop(listing);
    let (events, sink) = collect();

    let outcome = count_with(
        &id,
        volume(),
        &["/data/photos".to_string(), "/data/empty".to_string()],
        &sink,
        &stop,
    )
    .await;

    assert!(outcome.cancelled);
    assert_eq!(outcome.counted, 0);
    assert!(events.lock().expect("test lock").is_empty());
}
