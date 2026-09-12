//! Many callers listing one folder at once, and the device answering once.
//!
//! A copy's conflict check stats every top-level source, and on MTP a stat lists
//! the parent, so a 101-photo paste out of `/DCIM/Camera` asked for the same
//! 818-entry listing sixteen at a time. Each caller checked the 5 s listing cache
//! BEFORE queueing on the device lock, so every one missed and re-listed in turn,
//! 4–9 s apiece, while the copy's first read waited behind them all (field report
//! ERR-44S2Q, Pixel 8a, v0.44.0).

use std::sync::atomic::Ordering;
use std::time::Duration;

use cmdr_fs::testing::wait_until_async;

use super::directory_ops::CONCURRENT_LIST_CALLS;
use crate::testing::{connect_virtual_device, device_lock, test_connection_manager};

/// How many callers queue behind the first listing.
const CALLERS: usize = 8;

/// Callers that queued on the device lock while one listing ran get THAT
/// listing, not one each.
///
/// The test holds the device lock until every caller is inside its listing, so
/// none of them can have seen a cached answer on the way in: what's left to pin
/// is whether they look again once it's their turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn callers_queued_behind_one_listing_share_it_instead_of_re_listing() {
    let _guard = device_lock().await;
    let device = connect_virtual_device(test_connection_manager()).await;
    let manager = test_connection_manager();

    let device_arc = manager.device_arc(&device.id).await.expect("the device is connected");
    let held = device_arc.lock().await;
    let before = manager.wire_listing_count(&device.id).await;

    let listings: Vec<_> = (0..CALLERS)
        .map(|_| {
            let id = device.id.clone();
            let storage_id = device.storage_id;
            tokio::spawn(async move {
                test_connection_manager()
                    .list_directory(&id, storage_id, "/Documents")
                    .await
            })
        })
        .collect();
    wait_until_async(Duration::from_secs(5), "every caller to be inside its listing", || {
        CONCURRENT_LIST_CALLS.load(Ordering::Relaxed) as usize == CALLERS
    })
    .await;
    drop(held);

    for listing in listings {
        let entries = listing
            .await
            .expect("the listing task must not panic")
            .expect("the listing must succeed");
        assert!(
            entries.iter().any(|entry| entry.name == "report.txt"),
            "every caller gets the folder's real contents"
        );
    }
    assert_eq!(
        manager.wire_listing_count(&device.id).await - before,
        1,
        "callers queued behind one listing (count: {CALLERS}) must share it, not re-list the folder one after another"
    );

    device.teardown(test_connection_manager()).await;
}
