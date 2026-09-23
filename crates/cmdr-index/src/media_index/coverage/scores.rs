//! The cached importance folder scores the coverage gates read, and the threshold view
//! of them the enrichment gate checks membership against.
//!
//! **Why a cache at all.** Reading a volume's scores is `above_threshold(0.0)`: an
//! ordered read of EVERY scored folder, which SQLite runs as an external merge sort
//! (a measured 368,043 scored folders on one root), plus rebuilding a map that size.
//! Ruinous per UI query — the file-status badge asks per visible range, per pane, on
//! every listing swap and enrichment tick. Uncached, those queries piled up on the
//! blocking pool until it hit its 512-thread cap, and every OTHER `spawn_blocking` in
//! the app (directory listings, the volume list) starved behind them. Ruinous on a
//! timer too: it was 45.8 ms of every 60-second media live tick at 90,308 folders
//! (release build, M1 Max, `scheduler/live_bench.rs`, 2026-08-21 —
//! `docs/notes/live-tick-cost-2026-08-21.md`). Every consumer reads through here now,
//! passes included.
//!
//! **Why a subscription and not a generation stamp.** An INCREMENTAL rescore writes
//! rows at the CURRENT generation without bumping it (`importance/writer.rs`), so a
//! generation-keyed cache would serve stale scores until the next full pass. The
//! recompute bus is the store's own answer to this: [`WeightsChanged`] carries either
//! a patch or "rebuild", and a receiver that falls behind is TOLD rather than silently
//! skipped. The reload contract is documented in
//! `crates/cmdr-index/src/importance/read/DETAILS.md` § The reload contract.
//!
//! **Why draining is pull-based.** The one other caching consumer (search's
//! `weights.rs`) owns a background task because its map must be fresh for a query
//! that never asks for it. Here every read goes through [`importance_scores`], so
//! draining the receiver at the top of a read is the same freshness with no task, no
//! lifecycle, and no per-volume spawn from inside a blocking closure. A notice that
//! arrives while nobody is asking simply waits in the channel.
//!
//! **A store read happens while the cache lock is held**, so first reads for different
//! volumes serialize rather than overlapping. That's deliberate: it costs a little
//! multi-volume startup latency (a handful of reads, once each) and in exchange a
//! thundering herd for the SAME volume collapses into one read instead of N identical
//! ones, which is the case that actually hurt. Keep it that way unless a profile says
//! otherwise; per-volume locks buy little once the cache is warm.
//!
//! **Why paths are hashed, and why only while media indexing is on.** A volume's table
//! is resident for as long as anything reads it, and it's big: 179,949 scored folders on
//! one boot volume held 31 MiB as path `String`s plus their table (release build, page
//! census, 2026-09-23), and a NAS scores 368,043. Every consumer LOOKS UP a folder and
//! none enumerates the paths, so the table keeps [`hash_path`] in their place: a 17-byte
//! slot per folder, ~4.5 MiB at that size (pinned by `scores/memory_tests.rs`). The host
//! also drops a data dir's tables when media indexing turns off ([`release_scores`]), and
//! its volume-state poll doesn't read scores while it's off, so a user who never turned
//! the feature on never pays for them at all.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};

use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::TryRecvError;

use crate::importance::read::{WeightsChanged, subscribe};
use cmdr_fs::ignore_poison::IgnorePoison;
use cmdr_fs::path_hash::{PrehashedState, hash_path};

/// `hash_path(folder)` → importance score. The resident shape; see the module header.
type ScoreTable = HashMap<u64, f64, PrehashedState>;

/// A volume's importance folder scores: a cheap handle onto the cached table, optionally
/// narrowed to the folders at or above a threshold.
///
/// Keyed by path HASH, so a folder is looked up by its path but never listed. Each lookup
/// hashes the path once, which costs about what hashing a `String` key did.
///
/// ## Collisions
///
/// Two folders whose paths hash to the same `u64` share one entry, so one reads the
/// other's score. At 368,043 folders (the biggest volume measured) against 64 bits, the
/// chance of ANY collision on that volume is ~3.7e-9 (`n² / 2⁶⁵`). If one happened:
///
/// - An unscored folder reading a real score gets covered: its images get enriched, and
///   a reclaim keeps its rows. Extra work, never lost data.
/// - A scored folder reading a LOWER score may drop below the threshold: its images wait,
///   and a user-confirmed reclaim may delete its rows, which are derived from the images
///   and come back on the next pass that covers them.
/// - A delta's removal keyed on a colliding hash drops the other folder's score until the
///   next full recompute rebuilds the table.
///
/// All of it bounded, self-healing, and about as likely as a cosmic-ray bit flip in the
/// table itself, which is why this doesn't keep the paths to rule it out.
#[derive(Clone)]
pub struct FolderScores {
    table: Arc<ScoreTable>,
    /// `Some(threshold)` hides every folder scoring below it: the view the enrichment gate
    /// reads, where MEMBERSHIP is the coverage decision.
    at_least: Option<f64>,
}

impl FolderScores {
    /// No scored folders: every lookup misses. What a scope that never consults
    /// importance counts against.
    pub fn empty() -> FolderScores {
        FolderScores {
            table: Arc::new(ScoreTable::default()),
            at_least: None,
        }
    }

    /// `folder`'s score, or `None` when it isn't scored or scores below this view's
    /// threshold.
    pub fn get(&self, folder: &str) -> Option<f64> {
        let score = *self.table.get(&hash_path(folder))?;
        match self.at_least {
            Some(threshold) if score < threshold => None,
            _ => Some(score),
        }
    }

    /// Whether `folder` is in this view: scored, and at or above its threshold.
    pub fn contains(&self, folder: &str) -> bool {
        self.get(folder).is_some()
    }

    /// How many folders in this view score at or above `threshold`. A pass over the
    /// table, so it's for settings previews, ❌ never per image.
    pub fn count_at_least(&self, threshold: f64) -> u64 {
        let floor = self.at_least.map_or(threshold, |own| own.max(threshold));
        self.table.values().filter(|score| **score >= floor).count() as u64
    }

    /// Whether `a` and `b` read the same table: the test proof that no re-read happened.
    #[cfg(test)]
    pub(crate) fn shares_table_with(&self, other: &FolderScores) -> bool {
        Arc::ptr_eq(&self.table, &other.table)
    }
}

/// Build a view over every folder in `scores`, for tests and a host with its own score
/// source. Production reads through [`importance_scores`].
impl<S: AsRef<str>> FromIterator<(S, f64)> for FolderScores {
    fn from_iter<I: IntoIterator<Item = (S, f64)>>(scores: I) -> FolderScores {
        FolderScores {
            table: Arc::new(table_from(scores.into_iter())),
            at_least: None,
        }
    }
}

/// One volume's cached scores, plus the subscription that keeps them honest.
struct CachedScores {
    /// Every scored folder, exactly what a fresh `above_threshold(0.0)` would read. Held
    /// as an `Arc` so a reader clones a handle rather than a table that costs megabytes;
    /// a threshold view shares it rather than copying the folders that pass.
    all: Arc<ScoreTable>,
    /// Recompute notices for this volume. Subscribed BEFORE the first read, so a pass
    /// that finishes during that read lands in the channel instead of being missed.
    notices: Receiver<WeightsChanged>,
}

/// What a cached entry is an entry FOR: one importance store, which is one volume
/// inside one data dir. Keyed by both because the volume id alone doesn't name a store —
/// the app has exactly one data dir, so this changes nothing in production, and it means
/// two tests over their own temp dirs can't read each other's scores through a
/// process-global map.
#[derive(Clone, PartialEq, Eq, Hash)]
struct StoreKey {
    data_dir: std::path::PathBuf,
    volume_id: String,
}

impl StoreKey {
    fn new(data_dir: &Path, volume_id: &str) -> StoreKey {
        StoreKey {
            data_dir: data_dir.to_path_buf(),
            volume_id: volume_id.to_string(),
        }
    }
}

/// Per-store score caches. An entry lives until the host releases its data dir
/// ([`release_scores`], when media indexing turns off): an unmounted volume's entry costs
/// one table and stays correct if it returns.
static CACHE: LazyLock<Mutex<HashMap<StoreKey, CachedScores>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// What draining the notices told us to do with a cached entry.
#[derive(Debug, PartialEq)]
enum Refresh {
    /// Nothing changed; the cached map is current.
    Fresh,
    /// Apply these edits in place — `O(changed)`, not `O(all scores)`.
    Patch {
        upserted: Vec<(String, f64)>,
        removed: Vec<String>,
    },
    /// Re-read the whole table: a full pass replaced it, or we fell behind and can't
    /// know what we missed.
    Rebuild,
}

/// Fold every notice waiting on `notices` into ONE decision, without blocking.
///
/// **A lag rebuilds.** The channel is bounded, so a receiver that falls behind is
/// told it missed notices rather than handed a hole — and a skipped delta would leave
/// the map disagreeing with the store with nothing to detect it until the next full
/// pass. ❌ Never treat a lag as "nothing happened".
///
/// A rebuild absorbs every notice after it: once we're re-reading the whole table,
/// the deltas we'd apply on top are already in what we'll read. Deltas before a
/// rebuild are dropped for the same reason. A closed channel means the senders are
/// gone (they're process-global, so this never fires in practice); the cached map is
/// then as good as it will ever get, so it reads `Fresh`.
fn drain(notices: &mut Receiver<WeightsChanged>) -> Refresh {
    let mut refresh = Refresh::Fresh;
    loop {
        match notices.try_recv() {
            Ok(WeightsChanged::Delta { upserted, removed, .. }) => {
                if let Refresh::Rebuild = refresh {
                    continue;
                }
                let (mut ups, mut rems) = match refresh {
                    Refresh::Patch { upserted, removed } => (upserted, removed),
                    _ => (Vec::new(), Vec::new()),
                };
                ups.extend(upserted.iter().cloned());
                rems.extend(removed.iter().cloned());
                refresh = Refresh::Patch {
                    upserted: ups,
                    removed: rems,
                };
            }
            Ok(WeightsChanged::ReloadAll { .. }) => refresh = Refresh::Rebuild,
            Err(TryRecvError::Lagged(missed)) => {
                log::debug!(target: "media_index", "importance score notices missed ({missed}), rebuilding the coverage score cache");
                refresh = Refresh::Rebuild;
            }
            Err(TryRecvError::Empty) | Err(TryRecvError::Closed) => return refresh,
        }
    }
}

/// Apply one drained delta to a cached map in place.
///
/// Removals go FIRST, matching the order the store's transaction applied them and
/// the search cache's rule, so an upsert of a path that also appears in `removed`
/// resolves in favor of the upsert (the fresher fact). [`Arc::make_mut`] mutates in
/// place when no reader holds the handle and clones when one does, so a reader that
/// already took a snapshot keeps reading it untouched.
fn patch(entry: &mut CachedScores, upserted: &[(String, f64)], removed: &[String]) {
    let all = Arc::make_mut(&mut entry.all);
    for path in removed {
        all.remove(&hash_path(path));
    }
    for (path, score) in upserted {
        all.insert(hash_path(path), *score);
    }
}

/// Read every scored folder for `volume_id` straight from the store, bypassing the
/// cache. `None` when importance has NEVER scored the volume (fresh, offline, or
/// importance disabled) — the load-bearing signal that sends the coverage gates to
/// override-only rather than to "cover everything".
fn read_all(data_dir: &Path, volume_id: &str) -> Option<ScoreTable> {
    use crate::importance::{ImportanceIndex, SignalSet};
    let index = ImportanceIndex::open(data_dir, volume_id, SignalSet::all());
    if !index.is_scored() {
        return None;
    }
    match index.above_threshold(0.0) {
        Ok(weights) => Some(table_from(weights.into_iter().map(|w| (w.path, w.score.value())))),
        Err(e) => {
            log::debug!(target: "media_index", "importance scores unreadable for '{volume_id}': {e}");
            None
        }
    }
}

/// Build the resident table from `(folder, score)` pairs, keeping each path's hash and
/// dropping the path.
fn table_from<S: AsRef<str>>(scores: impl Iterator<Item = (S, f64)>) -> ScoreTable {
    scores.map(|(path, score)| (hash_path(path.as_ref()), score)).collect()
}

/// Every scored folder for `volume_id`, refreshing the cached map first. Private
/// because a host reaches this through [`importance_scores`], whose `at_least`
/// argument also covers the gate's threshold-filtered view: two public functions would
/// spend one of the crate's capped public items on a view this one can serve.
fn all_scores(data_dir: &Path, volume_id: &str) -> Option<Arc<ScoreTable>> {
    let key = StoreKey::new(data_dir, volume_id);
    let mut cache = CACHE.lock_ignore_poison();
    // Taking the entry OUT hands us its receiver to carry into the rebuild below. ❌
    // Don't `resubscribe()` there instead: a fresh receiver starts at the channel's
    // tail, so a notice sent between the drain and the resubscribe would be skipped,
    // and this receiver is already positioned exactly after the last notice we read.
    if let Some(mut entry) = cache.remove(&key) {
        match drain(&mut entry.notices) {
            Refresh::Fresh => {}
            Refresh::Patch { upserted, removed } => patch(&mut entry, &upserted, &removed),
            Refresh::Rebuild => {
                // Read with the subscription still LIVE, so a pass that commits
                // during the read waits in the channel rather than falling into the
                // gap. The cost is re-applying a notice the read already reflects,
                // which is idempotent.
                entry.all = Arc::new(read_all(data_dir, volume_id)?);
            }
        }
        let all = Arc::clone(&entry.all);
        cache.insert(key, entry);
        return Some(all);
    }
    // First read for this store: subscribe BEFORE reading, same gap-free reason.
    let notices = subscribe(volume_id);
    let all = Arc::new(read_all(data_dir, volume_id)?);
    cache.insert(
        key,
        CachedScores {
            all: Arc::clone(&all),
            notices,
        },
    );
    Some(all)
}

/// A volume's importance folder scores, or `None` when importance never scored it
/// (fresh / offline / disabled) — the load-bearing signal that sends the coverage gates to
/// override-only rather than to "cover everything".
///
/// `at_least` picks the view:
///
/// - `None`: EVERY scored folder, no threshold applied, so ONE read serves any slider
///   position during a debounced drag.
/// - `Some(threshold)`: only the folders at or above it, because the enrichment gate
///   ([`local_should_enrich`](crate::media_index::scheduler::local_should_enrich)) keys
///   on MEMBERSHIP — the threshold has to be part of the view rather than checked at
///   lookup.
///
/// Both views share the one cached table, so either is a handle, never a copy. Cheap
/// after the first call per volume: a fresh read happens only when the store says its
/// weights moved. See this module's header for why that freshness signal is the
/// recompute subscription and not the generation stamp.
pub fn importance_scores(data_dir: &Path, volume_id: &str, at_least: Option<f64>) -> Option<FolderScores> {
    Some(FolderScores {
        table: all_scores(data_dir, volume_id)?,
        at_least,
    })
}

/// Drop every cached table read from `data_dir`, which the host does when media indexing
/// turns off: nothing that reads scores runs while it's off, and a table can hold tens of
/// MiB on a big volume. A reader still holding a view keeps it until it's done, and the
/// next read after the feature comes back re-reads the store.
///
/// Per data dir, like the cache key: tests running in parallel each own one, so releasing
/// one can't pull a table out from under another.
pub(crate) fn release_scores(data_dir: &Path) {
    CACHE.lock_ignore_poison().retain(|key, _| key.data_dir != data_dir);
}

/// Drop ONE volume's cached entries (in every data dir), so a test starts from a cold
/// cache for its own volume. Test-only, and deliberately NOT re-exported from
/// `coverage`: every caller is inside this crate, so widening it would spend one of the
/// crate's capped public items (`index-crate-isolation`) on nothing.
///
/// ❌ Per volume, never the whole map: these tests run in parallel against one
/// process-wide cache, and clearing all of it dropped a sibling test's entry between
/// its patch and its read.
#[cfg(test)]
fn clear_cache_for_test(volume_id: &str) {
    CACHE.lock_ignore_poison().retain(|key, _| key.volume_id != volume_id);
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod memory_tests;
