//! The count queue: requests join the running job, waiting rows light up after
//! the delay, and a cancel puts them back.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::Instant;

use super::jobs::{self, Joined};
use super::{HOURGLASS_DELAY, count_with, plan};
use crate::file_system::listing::caching_test_support::TestListing;
use crate::file_system::listing::metadata::FileEntry;
use crate::listing_index_sizes::ListingIndexSizesChanged;
use crate::listing_index_sizes::refresh::RowSizes;

fn folder(path: &str) -> FileEntry {
    let name = path.rsplit('/').next().expect("a path has a name").to_string();
    FileEntry::new(name, path.to_string(), true, false)
}

fn queued(paths: &[&str]) -> Vec<(String, RowSizes)> {
    paths.iter().map(|p| (p.to_string(), RowSizes::default())).collect()
}

#[test]
fn a_request_while_a_count_runs_joins_its_queue_without_repeats() {
    let id = "jobs-join";
    let Joined::Owner(job) = jobs::enqueue(id, queued(&["/a", "/b"]), Instant::now()) else {
        panic!("the first request owns the job");
    };
    // Space on /b (queued already) and /c: only /c is new.
    let Joined::Waiter { appended, .. } = jobs::enqueue(id, queued(&["/b", "/c"]), Instant::now()) else {
        panic!("a second request joins the running job");
    };
    assert_eq!(appended, 1);

    let order: Vec<String> = std::iter::from_fn(|| jobs::next(id, &job).map(|q| q.path)).collect();
    assert_eq!(order, vec!["/a", "/b", "/c"], "in request order");
    assert!(!jobs::cancel(id), "a drained job left the map");
}

#[test]
fn waiting_rows_light_up_only_after_the_delay() {
    let id = "jobs-light";
    let start = Instant::now();
    let Joined::Owner(job) = jobs::enqueue(id, queued(&["/a", "/b"]), start) else {
        panic!("owner");
    };
    assert!(
        job.due_to_light(start, HOURGLASS_DELAY).is_empty(),
        "nothing before the delay"
    );
    let lit = job.due_to_light(start + HOURGLASS_DELAY, HOURGLASS_DELAY);
    assert_eq!(lit.len(), 2);
    assert!(
        job.due_to_light(start + Duration::from_secs(5), HOURGLASS_DELAY)
            .is_empty(),
        "once each"
    );
    jobs::cancel(id);
}

#[tokio::test]
async fn a_cancel_puts_lit_waiting_rows_back_and_stops() {
    let listing = TestListing::new()
        .path("/data")
        .include_hidden(false)
        .entries(vec![folder("/data/a"), folder("/data/b")])
        .insert("jobs-cancel");
    let events: Arc<Mutex<Vec<ListingIndexSizesChanged>>> = Arc::default();
    let sink_events = Arc::clone(&events);
    let sink = move |e: ListingIndexSizesChanged| sink_events.lock().expect("test lock").push(e);
    let plan = plan(listing.id(), false, None, |_| false).expect("cached");
    // Esc before the walk starts: the job is already stopped when it runs.
    let id = listing.id().to_string();
    let volume: Arc<dyn crate::file_system::volume::Volume> = Arc::new(cmdr_fs::volume::InMemoryVolume::with_entries(
        "test",
        vec![folder("/data"), folder("/data/a"), folder("/data/b")],
    ));
    let canceller = {
        let id = id.clone();
        tokio::spawn(async move {
            while !jobs::cancel(&id) {
                tokio::task::yield_now().await;
            }
        })
    };
    let outcome = count_with(&id, plan, volume, &sink).await;
    canceller.abort();

    assert!(outcome.counted <= 2);
    let rows = listing.entries();
    assert!(
        rows.iter()
            .all(|e| e.recursive_size.is_none() || e.recursive_size_complete.is_some()),
        "every row ends either untouched or with a reading"
    );
}
