//! Calculate folder sizes on demand: Total Commander's ⌥⇧⏎ ("count the space
//! subfolders occupy") and Space on a folder.
//!
//! The drive index answers folder sizes for the volumes it covers; this walks the
//! folders it can't answer for (SFTP, WebDAV, S3, archives, a folder the index
//! excludes) through the same cancellable copy scan the transfer dialog uses
//! (`Volume::scan_for_copy_batch_with_boundary`), one folder at a time.
//!
//! Readings reach the pane exactly like the index's: written into the listing
//! cache first, then sent as `listing-index-sizes-changed`, so the size cell, its
//! hourglass, the status bar, and a size sort all read them with no new frontend
//! path. A folder being walked shows its running total as a lower bound (`≥`)
//! with the hourglass; a finished one shows its exact size.
//!
//! One count per listing: starting another supersedes the first, [`cancel`]
//! stops it (Esc), and closing the listing stops it. A walk the user stopped
//! keeps what it counted as a lower bound; a superseded one leaves the row to its
//! successor. A refresh that re-reads the folder drops the readings, as Total
//! Commander's does.

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use cmdr_fs::volume::{ListingProgress, ScanBoundary, ScanStop, ScanStopSignal, VolumeError};
use cmdr_index::store::DirStats;
use serde::{Deserialize, Serialize};

use crate::file_system::listing::cached_listing::LISTING_CACHE;
use crate::file_system::volume::Volume;
use crate::ignore_poison::{IgnorePoison, RwLockIgnorePoison};

use super::refresh::RowSizes;
use super::{FolderSizes, ListingIndexSizesChanged};

/// How often a folder being walked sends its running total. Also the first
/// send: a folder that finishes sooner goes straight to its exact size.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// Why a count didn't start. Typed, so the frontend never reads a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum CountFolderSizesError {
    /// The pane's listing is no longer cached (it moved on).
    Gone { listing_id: String },
    /// No volume answers for the listing's folder (unplugged, disconnected).
    NotConnected { volume_id: String },
}

/// How a count ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FolderSizeCountOutcome {
    /// Folders whose exact size landed in a pane that still shows them.
    pub counted: usize,
    /// Stopped early: by [`cancel`] (Esc), a newer count, or the listing closing.
    pub cancelled: bool,
}

/// The cooperative stop one count answers to.
#[derive(Default)]
pub(crate) struct CountStop {
    cancelled: AtomicBool,
    /// A newer count of the same listing took over: this one must not touch the
    /// rows any more, or its "stopped" reading would land over the newer one's.
    superseded: AtomicBool,
}

impl CountStop {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    fn supersede(&self) {
        self.superseded.store(true, Ordering::Relaxed);
        self.cancel();
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    fn is_superseded(&self) -> bool {
        self.superseded.load(Ordering::Relaxed)
    }
}

impl ScanStopSignal for CountStop {
    fn is_stopping_or_paused(&self) -> bool {
        self.is_cancelled()
    }

    fn stop_or_park<'a>(&'a self) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async move { self.is_cancelled() })
    }

    fn stop_or_park_blocking(&self) -> bool {
        self.is_cancelled()
    }
}

/// The count running for each listing.
static RUNNING: LazyLock<Mutex<HashMap<String, Arc<CountStop>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Registers a new count for `listing_id`, superseding the one running there.
/// Done FIRST, before any await, so an Esc that comes while the volume resolves
/// already has something to stop.
pub(crate) fn register(listing_id: &str) -> Arc<CountStop> {
    let stop = Arc::new(CountStop::default());
    if let Some(previous) = RUNNING
        .lock_ignore_poison()
        .insert(listing_id.to_string(), Arc::clone(&stop))
    {
        previous.supersede();
    }
    stop
}

fn unregister(listing_id: &str, stop: &Arc<CountStop>) {
    let mut running = RUNNING.lock_ignore_poison();
    if running
        .get(listing_id)
        .is_some_and(|current| Arc::ptr_eq(current, stop))
    {
        running.remove(listing_id);
    }
}

/// Stops the count running for `listing_id`. Reports whether one was running.
pub(crate) fn cancel(listing_id: &str) -> bool {
    match RUNNING.lock_ignore_poison().remove(listing_id) {
        Some(stop) => {
            stop.cancel();
            true
        }
        None => false,
    }
}

/// What a count will walk: the listing's volume and folder, and the folder rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CountPlan {
    pub volume_id: String,
    pub dir: PathBuf,
    pub folders: Vec<String>,
}

/// The folder rows a pane shows that a count should walk, in row order.
///
/// Where the drive index covers the volume (`indexed`), it owns the sizes it has,
/// even a lower bound it's still completing: only a folder it says nothing about
/// (excluded, or inside an archive) is walked, else the two would overwrite each
/// other. Elsewhere, every folder without an exact size. With `only` (Space on a
/// folder), just those paths, recounted on an unindexed volume. Symlinked folders
/// are skipped, as the index skips them.
pub(crate) fn plan(
    listing_id: &str,
    include_hidden: bool,
    only: Option<&[String]>,
    indexed: impl Fn(&str) -> bool,
) -> Result<CountPlan, CountFolderSizesError> {
    let cache = LISTING_CACHE.read_ignore_poison();
    let listing = cache.get(listing_id).ok_or_else(|| CountFolderSizesError::Gone {
        listing_id: listing_id.to_string(),
    })?;
    listing.touch();
    let index_owns_sizes = indexed(&listing.volume_id);
    let folders = listing
        .rows(include_hidden)
        .iter()
        .filter(|entry| entry.is_directory && !entry.is_symlink)
        .filter(|entry| only.is_none_or(|paths| paths.contains(&entry.path)))
        .filter(|entry| {
            if index_owns_sizes {
                entry.recursive_size.is_none()
            } else {
                only.is_some() || entry.recursive_size_complete != Some(true) || entry.recursive_size.is_none()
            }
        })
        .map(|entry| entry.path.clone())
        .collect();
    Ok(CountPlan {
        volume_id: listing.volume_id.clone(),
        dir: listing.path.as_path().to_path_buf(),
        folders,
    })
}

/// Counts the folders [`plan`] picks, resolving the listing's volume the way a
/// copy scan does (an archive or `.git` route included). `sink` receives each
/// event, after its reading is in the listing cache.
pub(crate) async fn count(
    listing_id: &str,
    include_hidden: bool,
    only: Option<&[String]>,
    sink: &(dyn Fn(ListingIndexSizesChanged) + Sync),
) -> Result<FolderSizeCountOutcome, CountFolderSizesError> {
    let stop = register(listing_id);
    let outcome = async {
        let indexed_volumes = crate::index_host::index().volume_ids();
        let plan = plan(listing_id, include_hidden, only, |volume_id| {
            indexed_volumes.iter().any(|indexed| indexed == volume_id)
        })?;
        let resolved = crate::file_system::volume::manager::get_volume_manager()
            .resolve(&plan.volume_id, &plan.dir)
            .await;
        let volume = resolved.volume.ok_or_else(|| CountFolderSizesError::NotConnected {
            volume_id: plan.volume_id.clone(),
        })?;
        Ok(count_with(listing_id, volume, &plan.folders, sink, &stop).await)
    }
    .await;
    unregister(listing_id, &stop);
    outcome
}

/// Walks `folders` on `volume` one at a time, publishing a running total while
/// each is walked and its exact size when it's done. `stop` comes from
/// [`register`]; the caller unregisters it.
pub(crate) async fn count_with(
    listing_id: &str,
    volume: Arc<dyn Volume>,
    folders: &[String],
    sink: &(dyn Fn(ListingIndexSizesChanged) + Sync),
    stop: &Arc<CountStop>,
) -> FolderSizeCountOutcome {
    let mut counted = 0;
    for folder in folders {
        if stop.is_cancelled() {
            return FolderSizeCountOutcome {
                counted,
                cancelled: true,
            };
        }
        // What the row showed, to put back if the walk can't finish.
        let Some(before) = row_sizes(listing_id, folder) else {
            match listing_is_open(listing_id) {
                // The row went away (deleted, renamed): the rest still get counted.
                true => continue,
                // The pane moved on: nobody is left to show a size to.
                false => {
                    stop.cancel();
                    return FolderSizeCountOutcome {
                        counted,
                        cancelled: true,
                    };
                }
            }
        };

        let started = Instant::now();
        let last_sent = Mutex::new(None::<Instant>);
        let on_progress = |progress: ListingProgress| {
            if stop.is_cancelled() {
                return;
            }
            let mut last = last_sent.lock_ignore_poison();
            let due = last.map_or(started.elapsed() >= PROGRESS_EVERY, |at| at.elapsed() >= PROGRESS_EVERY);
            if due {
                *last = Some(Instant::now());
                if publish(listing_id, folder, reading(folder, progress, Walk::Running), sink) == Wrote::NoListing {
                    stop.cancel();
                }
            }
        };
        let boundary = ScanBoundary::new(Some(&on_progress)).stopping_at(ScanStop::new(Arc::clone(stop) as _));
        match volume
            .scan_for_copy_batch_with_boundary(&[PathBuf::from(folder)], &boundary)
            .await
        {
            Ok(scan) if !stop.is_cancelled() => {
                let total = ListingProgress {
                    files: scan.aggregate.file_count,
                    dirs: scan.aggregate.dir_count,
                    bytes: scan.aggregate.total_bytes,
                };
                let mut stats = reading(folder, total, Walk::Done);
                stats.recursive_physical_size = scan.aggregate.dedup_bytes;
                if publish(listing_id, folder, stats, sink) == Wrote::Row {
                    counted += 1;
                }
            }
            // Finished, but a stop landed meanwhile: handled like a stopped walk.
            Ok(_) | Err(VolumeError::Cancelled(_)) => {
                if !stop.is_superseded() {
                    let _ = publish(
                        listing_id,
                        folder,
                        reading(folder, boundary.counts(), Walk::Stopped),
                        sink,
                    );
                }
                return FolderSizeCountOutcome {
                    counted,
                    cancelled: true,
                };
            }
            Err(err) => {
                // A folder we can't read (permissions, gone) gets back what it showed
                // before; the rest still get counted.
                log::debug!(target: "folder_sizes", "couldn't count {folder}: {err}");
                restore(listing_id, folder, before, sink);
            }
        }
    }
    FolderSizeCountOutcome {
        counted,
        cancelled: false,
    }
}

/// Where a folder's walk stands, for its reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Walk {
    /// Still walking: a lower bound, with the hourglass.
    Running,
    /// Done: the exact size.
    Done,
    /// Stopped before the end: a lower bound, without the hourglass.
    Stopped,
}

fn reading(folder: &str, progress: ListingProgress, walk: Walk) -> DirStats {
    DirStats {
        path: folder.to_string(),
        recursive_size: progress.bytes,
        recursive_physical_size: progress.bytes,
        recursive_file_count: progress.files as u64,
        recursive_dir_count: progress.dirs as u64,
        recursive_has_symlinks: false,
        recursive_size_pending: walk == Walk::Running,
        recursive_size_pending_changes_in: None,
        recursive_size_complete: walk == Walk::Done,
        recursive_size_stale: false,
    }
}

/// What a cache write found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wrote {
    /// The folder's row took the reading.
    Row,
    /// The listing is open but no longer holds the folder.
    NoRow,
    /// The listing is gone: its pane moved on.
    NoListing,
}

fn listing_is_open(listing_id: &str) -> bool {
    LISTING_CACHE.read_ignore_poison().get(listing_id).is_some()
}

/// The folder row's sizes as the cache holds them, or `None` without that row.
fn row_sizes(listing_id: &str, folder: &str) -> Option<RowSizes> {
    let cache = LISTING_CACHE.read_ignore_poison();
    let listing = cache.get(listing_id)?;
    let index = listing.index_of_path(folder)?;
    Some(RowSizes::of(&listing.entries()[index], false))
}

/// Writes `stats` onto the folder's cached row, then sends it to the pane.
fn publish(listing_id: &str, folder: &str, stats: DirStats, sink: &(dyn Fn(ListingIndexSizesChanged) + Sync)) -> Wrote {
    let wrote = write_to_cache(listing_id, folder, |entry| {
        RowSizes::of(entry, false).after(Some(&stats)).apply_to(entry);
    });
    if wrote == Wrote::Row {
        sink(event(listing_id, folder, Some(stats)));
    }
    wrote
}

/// Puts back the sizes the row showed before a walk that couldn't finish. The
/// pane re-reads its window from the cache (`full`), since "no size" isn't a
/// reading an event can carry.
fn restore(listing_id: &str, folder: &str, before: RowSizes, sink: &(dyn Fn(ListingIndexSizesChanged) + Sync)) {
    if write_to_cache(listing_id, folder, |entry| before.apply_to(entry)) == Wrote::Row {
        sink(ListingIndexSizesChanged {
            listing_id: listing_id.to_string(),
            full: true,
            folders: Vec::new(),
            current_dir_changed: false,
            current_dir: None,
        });
    }
}

fn write_to_cache(
    listing_id: &str,
    folder: &str,
    write: impl FnOnce(&mut crate::file_system::listing::metadata::FileEntry),
) -> Wrote {
    let mut cache = LISTING_CACHE.write_ignore_poison();
    let Some(listing) = cache.get_mut(listing_id) else {
        return Wrote::NoListing;
    };
    let mut write = Some(write);
    let mut wrote = Wrote::NoRow;
    listing.update_index_sizes_by_path(&[folder.to_string()], |_, entry| {
        if let Some(write) = write.take() {
            write(entry);
            wrote = Wrote::Row;
        }
    });
    wrote
}

fn event(listing_id: &str, folder: &str, stats: Option<DirStats>) -> ListingIndexSizesChanged {
    ListingIndexSizesChanged {
        listing_id: listing_id.to_string(),
        full: false,
        folders: vec![FolderSizes {
            path: folder.to_string(),
            stats,
        }],
        current_dir_changed: false,
        current_dir: None,
    }
}
