//! Index size updates, delivered only to the listings they touch.
//!
//! The drive index reports batches of directories whose recursive sizes changed
//! (`IndexEvent::DirsUpdated`, about once a second on a busy disk). This module keeps the set of open
//! listings (a [`ListingLifecycle`] observer), works out which of them a batch touches
//! ([`touched`]), and tells the frontend about those listings alone, with
//! `listing-index-sizes-changed`. A pane on `~/Downloads` no longer hears about a write in
//! `~/Library`.
//!
//! The work runs on its own task, fed through a channel: the batch arrives on the index writer's
//! thread, which must not wait on anything here.

mod touched;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_specta::Event;
use tokio::sync::mpsc;

use crate::file_system::listing::cached_listing::LISTING_CACHE;
use crate::file_system::volume::Volume;
use crate::ignore_poison::IgnorePoison;
use crate::listing_lifecycle::{ListingLifecycle, register_listing_lifecycle};
use cmdr_fs::ignore_poison::RwLockIgnorePoison;

pub(crate) use touched::{Touched, touched};

/// A listing's folder sizes changed in the index, so its pane should refresh them.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct ListingIndexSizesChanged {
    /// The listing whose rows moved.
    pub listing_id: String,
}

/// One open listing, as the index spells its folder.
#[derive(Debug, Clone)]
struct OpenListing {
    volume_id: String,
    /// The folder in the index's path space: the volume's listing spelling, firmlink-normalized.
    index_dir: String,
}

/// Every open listing, by id. Kept by [`IndexSizeListings`].
static OPEN: LazyLock<Mutex<HashMap<String, OpenListing>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Where [`dirs_updated`] hands batches to the worker. Unset until [`start`] runs.
static BATCHES: OnceLock<mpsc::UnboundedSender<Vec<String>>> = OnceLock::new();

/// Keeps [`OPEN`] in step with the listing cache.
struct IndexSizeListings;

impl ListingLifecycle for IndexSizeListings {
    fn id(&self) -> &'static str {
        "index-sizes"
    }

    fn listing_opened(&self, listing_id: &str, _volume: &dyn Volume, _path: &Path) {
        // The cache already holds the listing (the open path inserts before it notifies), and its
        // record carries both the volume id and the volume's one spelling of the folder.
        let Some(listing) = LISTING_CACHE
            .read_ignore_poison()
            .get(listing_id)
            .map(|listing| OpenListing {
                volume_id: listing.volume_id.clone(),
                index_dir: cmdr_fs::firmlinks::normalize_path(&listing.path.as_path().to_string_lossy()),
            })
        else {
            return;
        };
        OPEN.lock_ignore_poison().insert(listing_id.to_string(), listing);
    }

    fn listing_closed(&self, listing_id: &str) {
        OPEN.lock_ignore_poison().remove(listing_id);
    }
}

/// Registers the open-listing observer and starts the worker. Call once from setup.
pub(crate) fn start(app: AppHandle) {
    register_listing_lifecycle(Arc::new(IndexSizeListings));
    let (tx, rx) = mpsc::unbounded_channel();
    if BATCHES.set(tx).is_err() {
        return;
    }
    tauri::async_runtime::spawn(run(app, rx));
}

/// Hands one index batch to the worker. Called from the index event sink; never blocks.
pub(crate) fn dirs_updated(paths: Vec<String>) {
    if let Some(tx) = BATCHES.get() {
        // A closed channel means the app is shutting down; nothing is left to tell.
        let _ = tx.send(paths);
    }
}

async fn run(app: AppHandle, mut batches: mpsc::UnboundedReceiver<Vec<String>>) {
    while let Some(paths) = batches.recv().await {
        for listing_id in touched_listings(&paths).into_keys() {
            let _ = ListingIndexSizesChanged { listing_id }.emit(&app);
        }
    }
}

/// Every open listing `paths` touched, with what it touched.
fn touched_listings(paths: &[String]) -> HashMap<String, Touched> {
    let open = OPEN.lock_ignore_poison().clone();
    open.into_iter()
        .filter_map(|(id, listing)| touched(paths, &listing.volume_id, &listing.index_dir).map(|t| (id, t)))
        .collect()
}
