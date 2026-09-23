//! Tracks directories with unprocessed index writes in flight.
//!
//! When the user deletes or adds many files at once, the writer needs seconds
//! to minutes to propagate the recursive `dir_stats` deltas up every ancestor
//! chain. During that window the sizes shown in the file list are stale. This
//! tracker lets the UI mark those directories with a "size updating" hourglass
//! so the numbers aren't presented as settled truth while they're still moving.
//!
//! ## How it stays correct
//!
//! Two tiers, because two kinds of pending work have different lifetimes.
//!
//! **Transient set** (the `paths` map) — fast per-event marks:
//!
//! - **Mark** (live event loop): every dir whose recursive size is about to
//!   change is inserted, along with all its ancestors. We're handed exactly
//!   that set already — it's the `pending_paths` the loop drains into
//!   `DirsUpdated` — so marking rides the same data that drives the UI
//!   refresh ("flag exactly what we refresh").
//! - **Clear** (writer thread): the transient set is cleared wholesale when the
//!   writer's queue drains to empty. An empty queue means there is no
//!   unprocessed writer work, so the set is correct to empty. This is
//!   self-healing: even if marking ever missed or over-marked, every time the
//!   writer catches up the state resets to truth. There's no per-entry
//!   increment/decrement to leak, so the "stuck hourglass forever" failure class
//!   doesn't exist.
//!
//! **Held-roots set** (the `held_roots` map) — coalesced rescan scopes:
//!
//! - A detached `MustScanSubDirs` reconcile runs for seconds to minutes while
//!   the writer queue oscillates empty, so the transient set's drain-clear would
//!   wipe its hourglass long before it finishes. `hold(root)` / `release(root)`
//!   track a small set of rescan ROOT paths (no ancestor expansion) that survive
//!   writer drains; the reconciler holds at queue time and releases at every
//!   exit. `is_pending(path)` treats a held root's whole chain as pending in
//!   BOTH directions (below).
//! - Why roots + a query-time prefix test instead of expanding ancestors into
//!   the set: overlapping rescans (`/a/b` and `/a/c`) share ancestor rows, so
//!   expanding would either strip `/a` while one is in flight or leak it forever.
//!   Holding only roots keeps release exact and needs no refcounting.
//!
//! ## When the hourglass shows
//!
//! Pending and SHOWN are different questions. Under ordinary background churn
//! (cache writes in `~/Library`) a folder is pending for a few hundred
//! milliseconds every few seconds, and showing each of those blinked the
//! hourglass on and off all day. So each mark and hold remembers when its
//! episode started (a re-mark keeps the start; a drain or release ends it), and
//! [`PendingSizes::view`] shows the hourglass only once the episode has run for
//! [`SHOW_AFTER`]. An episode that did show stays up until [`MIN_SHOWN`] after it
//! first showed, so a folder that settles just past the threshold can't flash.
//! A folder that really stays busy (a mass delete the writer is minutes behind
//! on) shows two seconds in and stays marked until it settles.
//!
//! Nothing writes at the moment a flip happens, so the reader has to learn when
//! to look again: `view` answers `changes_in` for the timed flips, and
//! [`PendingSizes::clear`] returns the folders whose shown episode it just ended,
//! which the writer announces as a `DirsUpdated` batch.
//!
//! **Read** (`DirStats` build in `queries.rs`): a single [`PendingSizes::view`]
//! per directory, carried on `DirStats.recursive_size_pending` (shown) and
//! `DirStats.recursive_size_pending_changes_in`.
//!
//! **Per-volume routing (both tiers).** Marks, holds, releases, and the
//! writer-drain clear all target the OWNING volume's tracker via
//! `get_pending_sizes_for(volume_id)`. A root-only handle used from a non-root
//! writer would wipe root's hourglass on a non-root drain AND never clear its
//! own — so the volume id is threaded through `queue_must_scan_sub_dirs` and the
//! writer loop rather than defaulting to root.
//!
//! The tradeoff is coarse granularity: during a storm every touched ancestor
//! stays flagged until the writer fully drains (transient) or the rescan
//! completes (held), then they clear. For the target scenario (mass delete, "is
//! it settled yet?") that's the right granularity — it answers exactly that
//! question. The hourglass's role in the wider size-integrity story, and the
//! release-before-emit completion sequence, are in `indexing/DETAILS.md`
//! § "The dir_stats ledger".

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use super::handles::VolumeHandles;
use crate::indexing::paths::path_prefix;
#[cfg(test)]
use crate::indexing::volume::ROOT_VOLUME_ID;
use cmdr_fs::firmlinks;
use cmdr_fs::ignore_poison::IgnorePoison;

/// How long a folder has to stay pending before the hourglass shows. Below it, an update is a blip
/// the size column absorbs silently: the writer caught up before anyone could read the hourglass.
pub(crate) const SHOW_AFTER: Duration = Duration::from_secs(2);

/// How long a shown hourglass stays up at least, counted from when it first showed.
pub(crate) const MIN_SHOWN: Duration = Duration::from_secs(1);

/// What the hourglass shows for one folder at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PendingView {
    /// The "size updating" hourglass is on.
    pub(crate) shown: bool,
    /// When `shown` flips on its own, with no write to announce it. `None` when only a write, a
    /// drain, or a release can flip it.
    pub(crate) changes_in: Option<Duration>,
}

/// A shown episode that ended before [`MIN_SHOWN`] ran out, still on screen until `until`.
struct Linger {
    path: String,
    /// A held rescan root's linger covers its whole chain in both directions, like the hold did.
    tree: bool,
    until: Instant,
}

/// In-memory set of directory paths with unprocessed index writes in flight.
///
/// Paths are stored normalized (via [`firmlinks::normalize_path`]) so a query
/// matches regardless of how the caller navigated to the path (firmlink alias,
/// `/tmp` vs `/private/tmp`, etc.). Each maps to when its episode started.
pub(crate) struct PendingSizes {
    /// Per-event marks, cleared wholesale when the writer queue drains.
    paths: Mutex<HashMap<String, Instant>>,
    /// Rescan ROOT paths held for the lifetime of a detached `MustScanSubDirs`
    /// reconcile. Never ancestor-expanded; the drain-clear leaves them alone.
    held_roots: Mutex<HashMap<String, Instant>>,
    /// Shown episodes that ended inside their minimum time on screen. A handful at most.
    lingering: Mutex<Vec<Linger>>,
}

impl PendingSizes {
    pub(crate) fn new() -> Self {
        Self {
            paths: Mutex::new(HashMap::new()),
            held_roots: Mutex::new(HashMap::new()),
            lingering: Mutex::new(Vec::new()),
        }
    }

    /// Normalize `path` and insert it AND every ancestor directory.
    ///
    /// Centralizing the ancestor expansion here means callers can pass whatever
    /// affected dirs they have — a full ancestor chain (normal events) or a
    /// single parent (rename pre-pass) — and the membership test is correct for
    /// any ancestor row shown in the UI.
    pub(crate) fn mark(&self, path: &str) {
        self.mark_at(path, Instant::now());
    }

    fn mark_at(&self, path: &str, now: Instant) {
        let normalized = firmlinks::normalize_path(path);
        let mut guard = self.paths.lock_ignore_poison();
        let mut cur = normalized.as_str();
        loop {
            // A re-mark keeps the episode's start: the writer hasn't caught up since.
            guard.entry(cur.to_string()).or_insert(now);
            match cur.rfind('/') {
                // Parent is the root "/": insert it and stop.
                Some(0) => {
                    guard.entry("/".to_string()).or_insert(now);
                    break;
                }
                Some(pos) => cur = &cur[..pos],
                // No slash at all (not an absolute path). Nothing more to walk.
                None => break,
            }
        }
    }

    /// Hold `root` (a rescan root path) for the lifetime of its detached
    /// reconcile. Normalized so `is_pending` matches regardless of firmlink
    /// aliasing. Re-holding an already-held root is a no-op, and keeps its start.
    pub(crate) fn hold(&self, root: &str) {
        self.hold_at(root, Instant::now());
    }

    fn hold_at(&self, root: &str, now: Instant) {
        let normalized = firmlinks::normalize_path(root);
        self.held_roots.lock_ignore_poison().entry(normalized).or_insert(now);
    }

    /// Release a previously-held rescan root. Releasing an unheld (or
    /// already-released) root is a harmless no-op.
    pub(crate) fn release(&self, root: &str) {
        self.release_at(root, Instant::now());
    }

    fn release_at(&self, root: &str, now: Instant) {
        let normalized = firmlinks::normalize_path(root);
        let Some(since) = self.held_roots.lock_ignore_poison().remove(&normalized) else {
            return;
        };
        self.linger_if_short(normalized, true, since, now);
    }

    /// Whether `path` (normalized) has unprocessed index writes in flight —
    /// either it's in the transient set, or it's related to a held rescan root
    /// in EITHER direction: an ancestor-or-equal of the root (its aggregate
    /// includes the subtree being rewritten) or a descendant of it (its own rows
    /// are being rewritten). The held set is bounded by `pending_rescans` (a
    /// handful), so the linear scan is trivial.
    ///
    /// Raw membership, whatever the episode's age: what the tests pin the
    /// bookkeeping with. What the UI shows is [`Self::view`].
    #[cfg(test)]
    pub(crate) fn is_pending(&self, path: &str) -> bool {
        let normalized = firmlinks::normalize_path(path);
        self.pending_since(&normalized).is_some()
    }

    /// Whether the hourglass shows for `path` now, and when that flips on its own.
    pub(crate) fn view(&self, path: &str) -> PendingView {
        self.view_at(path, Instant::now())
    }

    fn view_at(&self, path: &str, now: Instant) -> PendingView {
        let normalized = firmlinks::normalize_path(path);
        let shows_at = self.pending_since(&normalized).map(|since| since + SHOW_AFTER);
        if shows_at.is_some_and(|at| at <= now) {
            // Up until the drain or release that ends it, which announces itself.
            return PendingView {
                shown: true,
                changes_in: None,
            };
        }
        let lingers_until = self
            .lingering
            .lock_ignore_poison()
            .iter()
            .filter(|linger| linger.until > now && covers(&linger.path, linger.tree, &normalized))
            .map(|linger| linger.until)
            .max();
        let next_flip = [shows_at, lingers_until].into_iter().flatten().min();
        PendingView {
            shown: lingers_until.is_some(),
            changes_in: next_flip.map(|at| at.saturating_duration_since(now)),
        }
    }

    /// Drop the transient marks. Called when the writer queue drains to empty.
    /// Leaves `held_roots` alone: a rescan's hourglass must outlive the writer
    /// oscillating empty mid-walk (that's the whole point of the held tier).
    ///
    /// Returns the folders whose hourglass was showing, so the caller announces
    /// that it went away (or started its last [`MIN_SHOWN`] stretch). Empty for
    /// the blips that never showed, which is nearly every drain.
    pub(crate) fn clear(&self) -> Vec<String> {
        self.clear_at(Instant::now())
    }

    fn clear_at(&self, now: Instant) -> Vec<String> {
        let drained = std::mem::take(&mut *self.paths.lock_ignore_poison());
        self.lingering.lock_ignore_poison().retain(|linger| linger.until > now);
        let mut ended = Vec::new();
        for (path, since) in drained {
            if since + SHOW_AFTER <= now {
                self.linger_if_short(path.clone(), false, since, now);
                ended.push(path);
            }
        }
        ended
    }

    /// The start of the episode `normalized` is part of, if it's pending: its own
    /// mark, or the earliest held root related to it.
    fn pending_since(&self, normalized: &str) -> Option<Instant> {
        let marked = self.paths.lock_ignore_poison().get(normalized).copied();
        let held = self
            .held_roots
            .lock_ignore_poison()
            .iter()
            .filter(|(root, _)| covers(root, true, normalized))
            .map(|(_, since)| *since)
            .min();
        marked.into_iter().chain(held).min()
    }

    /// Keeps an episode that just ended on screen until [`MIN_SHOWN`] after it
    /// first showed, when it showed at all and that time isn't up yet.
    fn linger_if_short(&self, path: String, tree: bool, since: Instant, now: Instant) {
        let shown_at = since + SHOW_AFTER;
        let until = shown_at + MIN_SHOWN;
        if shown_at <= now && until > now {
            self.lingering.lock_ignore_poison().push(Linger { path, tree, until });
        }
    }

    /// Number of transient marks. Test-only observability.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.paths.lock_ignore_poison().len()
    }

    /// Number of held rescan roots. Test-only observability.
    #[cfg(test)]
    pub(crate) fn held_len(&self) -> usize {
        self.held_roots.lock_ignore_poison().len()
    }

    /// Marks `path` as if its episode started `ago`, so a test can read a
    /// sustained episode without sleeping through [`SHOW_AFTER`].
    #[cfg(test)]
    pub(crate) fn mark_started(&self, path: &str, ago: Duration) {
        self.mark_at(path, Instant::now() - ago);
    }
}

/// Whether `normalized` is `path`, or (for a `tree`) related to it in either direction.
fn covers(path: &str, tree: bool, normalized: &str) -> bool {
    normalized == path
        || (tree
            && (path_prefix::is_strict_descendant(normalized, path)
                || path_prefix::is_strict_descendant(path, normalized)))
}

/// Every indexed volume's pending-size tracker, keyed by volume id. Installed and
/// withdrawn by lifecycle in lockstep with the volume's read pool (`enrichment.rs`
/// and the lifecycle sites in `lifecycle/state.rs`); an absent entry means that
/// volume isn't indexed, so reads answer "not pending".
static PENDING_SIZES: LazyLock<VolumeHandles<PendingSizes>> = LazyLock::new(VolumeHandles::new);

/// Tests that touch the root volume's tracker must hold this lock to avoid races
/// with parallel test threads (mirrors `READ_POOL_TEST_MUTEX`).
#[cfg(test)]
pub(crate) static PENDING_SIZES_TEST_MUTEX: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Clone the root tracker `Arc`, if installed.
#[cfg(test)]
pub(crate) fn get_pending_sizes() -> Option<Arc<PendingSizes>> {
    get_pending_sizes_for(ROOT_VOLUME_ID)
}

/// Clone a specific volume's tracker. `None` means the volume isn't indexed.
pub(crate) fn get_pending_sizes_for(volume_id: &str) -> Option<Arc<PendingSizes>> {
    PENDING_SIZES.get(volume_id)
}

/// Publish a volume's tracker, alongside its read pool, as lifecycle reserves the
/// volume's registry slot.
pub(crate) fn install_pending_sizes(volume_id: &str, tracker: Arc<PendingSizes>) {
    PENDING_SIZES.install(volume_id, tracker);
}

/// Withdraw a volume's tracker on stop / clear / failure. Afterwards the hourglass
/// reads "not pending" for that volume, which is the truth once it stops indexing.
pub(crate) fn uninstall_pending_sizes(volume_id: &str) {
    PENDING_SIZES.uninstall(volume_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    // Use synthetic paths under `/aaa` that `firmlinks::normalize_path` leaves
    // unchanged on both macOS and Linux (not `/tmp|/var|/etc`, not under
    // `/System/Volumes/Data`), so assertions are deterministic cross-platform.

    #[test]
    fn mark_then_query_is_pending() {
        let t = PendingSizes::new();
        assert!(!t.is_pending("/aaa/bbb"));
        t.mark("/aaa/bbb");
        assert!(t.is_pending("/aaa/bbb"));
        assert!(!t.is_pending("/aaa/ccc"));
    }

    #[test]
    fn mark_flags_every_ancestor() {
        let t = PendingSizes::new();
        t.mark("/aaa/bbb/ccc/ddd");
        // The path itself and each ancestor dir are pending.
        assert!(t.is_pending("/aaa/bbb/ccc/ddd"));
        assert!(t.is_pending("/aaa/bbb/ccc"));
        assert!(t.is_pending("/aaa/bbb"));
        assert!(t.is_pending("/aaa"));
        assert!(t.is_pending("/"));
        // An unrelated sibling is not.
        assert!(!t.is_pending("/aaa/bbb/ccc/eee"));
    }

    #[test]
    fn clear_empties_everything() {
        let t = PendingSizes::new();
        t.mark("/aaa/bbb/ccc");
        t.mark("/zzz");
        assert!(t.len() > 0);
        t.clear();
        assert_eq!(t.len(), 0);
        assert!(!t.is_pending("/aaa/bbb/ccc"));
        assert!(!t.is_pending("/aaa"));
    }

    #[test]
    fn marking_is_idempotent_across_overlapping_chains() {
        let t = PendingSizes::new();
        t.mark("/aaa/bbb/ccc");
        let after_first = t.len();
        // Marking a sibling under the same ancestors adds only the new leaf nodes;
        // shared ancestors dedup in the set.
        t.mark("/aaa/bbb/ddd");
        assert!(t.is_pending("/aaa/bbb/ccc"));
        assert!(t.is_pending("/aaa/bbb/ddd"));
        // /aaa/bbb/ddd + (shared /aaa/bbb, /aaa, / already present) => +1 only.
        assert_eq!(t.len(), after_first + 1);
    }

    #[test]
    fn normalization_is_symmetric_for_marked_paths() {
        // `mark` and `is_pending` both normalize, so any path normalizes to the
        // same key on read as it did on write. On macOS `/tmp` → `/private/tmp`;
        // on Linux it's unchanged. Either way the round-trip matches.
        let t = PendingSizes::new();
        t.mark("/tmp/aaa/bbb");
        assert!(t.is_pending("/tmp/aaa/bbb"));
        assert!(t.is_pending("/tmp/aaa"));
    }

    #[test]
    fn held_root_is_pending_in_both_directions() {
        let t = PendingSizes::new();
        t.hold("/aaa/bbb/ccc");
        // The held root itself.
        assert!(t.is_pending("/aaa/bbb/ccc"));
        // Ancestors of the held root: their aggregate includes the rewritten subtree.
        assert!(t.is_pending("/aaa/bbb"));
        assert!(t.is_pending("/aaa"));
        assert!(t.is_pending("/"));
        // Descendants of the held root: their own rows are being rewritten.
        assert!(t.is_pending("/aaa/bbb/ccc/ddd"));
        assert!(t.is_pending("/aaa/bbb/ccc/ddd/eee"));
        // A component-sibling that only shares a byte prefix is NOT pending.
        assert!(!t.is_pending("/aaa/bbb/cccX"));
        // An unrelated sibling subtree is not.
        assert!(!t.is_pending("/aaa/bbb/zzz"));
    }

    #[test]
    fn writer_drain_clear_keeps_holds() {
        let t = PendingSizes::new();
        t.mark("/aaa/bbb/ccc");
        t.hold("/aaa/rescan");
        // A writer-drain clear wipes only the transient marks.
        t.clear();
        assert_eq!(t.len(), 0, "transient marks cleared");
        assert!(!t.is_pending("/aaa/bbb/ccc"), "transient mark gone");
        // The held root and its chain survive the drain.
        assert!(t.is_pending("/aaa/rescan"), "held root survives the drain");
        assert!(t.is_pending("/aaa"), "held root ancestor survives the drain");
        assert_eq!(t.held_len(), 1);
    }

    #[test]
    fn release_drops_the_hold() {
        let t = PendingSizes::new();
        t.hold("/aaa/rescan");
        assert!(t.is_pending("/aaa/rescan"));
        t.release("/aaa/rescan");
        assert!(!t.is_pending("/aaa/rescan"));
        assert!(!t.is_pending("/aaa"));
        assert_eq!(t.held_len(), 0);
        // Releasing an unheld root is a harmless no-op.
        t.release("/aaa/never-held");
        assert_eq!(t.held_len(), 0);
    }

    #[test]
    fn overlapping_rescans_release_independently() {
        // Two sibling rescans under a shared ancestor `/aaa`. Releasing one must
        // NOT strip `/aaa`'s pendingness while the other is in flight — the exact
        // failure that expanding ancestors into the held set would cause.
        let t = PendingSizes::new();
        t.hold("/aaa/bbb");
        t.hold("/aaa/ccc");
        assert!(t.is_pending("/aaa"), "shared ancestor pending while both held");
        // Finish /aaa/bbb.
        t.release("/aaa/bbb");
        assert!(!t.is_pending("/aaa/bbb"), "the finished rescan's own chain clears");
        assert!(t.is_pending("/aaa/ccc"), "the in-flight rescan stays pending");
        assert!(
            t.is_pending("/aaa"),
            "shared ancestor still pending via the in-flight rescan"
        );
        // Finish /aaa/ccc: now `/aaa` clears too.
        t.release("/aaa/ccc");
        assert!(!t.is_pending("/aaa"));
        assert_eq!(t.held_len(), 0);
    }

    #[test]
    fn hold_is_idempotent() {
        // Re-holding an already-held root (e.g. a storm re-queue of the active
        // path) is a no-op; one release clears it.
        let t = PendingSizes::new();
        t.hold("/aaa/rescan");
        t.hold("/aaa/rescan");
        assert_eq!(t.held_len(), 1);
        t.release("/aaa/rescan");
        assert!(!t.is_pending("/aaa/rescan"));
        assert_eq!(t.held_len(), 0);
    }

    fn secs(s: f64) -> Duration {
        Duration::from_secs_f64(s)
    }

    fn shown(shown: bool, changes_in: Option<f64>) -> PendingView {
        PendingView {
            shown,
            changes_in: changes_in.map(secs),
        }
    }

    #[test]
    fn a_blip_never_shows_the_hourglass() {
        // Background churn: a cache write marks `~/Library`, the writer drains it within a second.
        let t = PendingSizes::new();
        let t0 = Instant::now();
        t.mark_at("/aaa/bbb", t0);
        assert_eq!(t.view_at("/aaa/bbb", t0 + secs(1.5)), shown(false, Some(0.5)));
        assert!(
            t.clear_at(t0 + secs(1.9)).is_empty(),
            "nothing was on screen to take down"
        );
        assert_eq!(t.view_at("/aaa/bbb", t0 + secs(1.9)), shown(false, None));
    }

    #[test]
    fn two_seconds_of_updating_shows_it() {
        let t = PendingSizes::new();
        let t0 = Instant::now();
        t.mark_at("/aaa/bbb", t0);
        // Re-marking mid-episode keeps its start: the writer never caught up in between.
        t.mark_at("/aaa/bbb", t0 + secs(1.5));
        assert_eq!(t.view_at("/aaa/bbb", t0 + secs(2.0)), shown(true, None));
        assert_eq!(t.view_at("/aaa", t0 + secs(2.0)), shown(true, None), "ancestors too");
    }

    #[test]
    fn a_drain_starts_the_clock_over() {
        // Blinking under churn is many short episodes, never one long one.
        let t = PendingSizes::new();
        let t0 = Instant::now();
        t.mark_at("/aaa/bbb", t0);
        t.clear_at(t0 + secs(1.0));
        t.mark_at("/aaa/bbb", t0 + secs(1.5));
        assert_eq!(t.view_at("/aaa/bbb", t0 + secs(3.0)), shown(false, Some(0.5)));
    }

    #[test]
    fn a_short_episode_stays_on_screen_for_the_minimum() {
        let t = PendingSizes::new();
        let t0 = Instant::now();
        t.mark_at("/aaa/bbb", t0);
        // Shown at 2.0 s, drained at 2.2 s: it stays up until 3.0 s, not a 0.2 s flash.
        let ended = t.clear_at(t0 + secs(2.2));
        assert!(
            ended.contains(&"/aaa/bbb".to_string()),
            "a shown folder's end is announced"
        );
        assert_eq!(t.view_at("/aaa/bbb", t0 + secs(2.5)), shown(true, Some(0.5)));
        assert_eq!(t.view_at("/aaa/bbb", t0 + secs(3.0)), shown(false, None));
    }

    #[test]
    fn a_long_episode_goes_away_the_moment_it_drains() {
        let t = PendingSizes::new();
        let t0 = Instant::now();
        t.mark_at("/aaa/bbb", t0);
        let ended = t.clear_at(t0 + secs(30.0));
        assert!(ended.contains(&"/aaa/bbb".to_string()));
        assert_eq!(t.view_at("/aaa/bbb", t0 + secs(30.0)), shown(false, None));
    }

    #[test]
    fn a_held_rescan_follows_the_same_timing_across_its_tree() {
        let t = PendingSizes::new();
        let t0 = Instant::now();
        t.hold_at("/aaa/bbb", t0);
        assert_eq!(t.view_at("/aaa/bbb/ccc", t0 + secs(1.0)), shown(false, Some(1.0)));
        assert_eq!(t.view_at("/aaa", t0 + secs(2.0)), shown(true, None));
        // Released at 2.4 s: the whole tree stays up until 3.0 s.
        t.release_at("/aaa/bbb", t0 + secs(2.4));
        assert_eq!(t.view_at("/aaa/bbb/ccc", t0 + secs(2.5)), shown(true, Some(0.5)));
        assert_eq!(t.view_at("/aaa", t0 + secs(3.0)), shown(false, None));
    }

    #[test]
    fn get_pending_sizes_for_routes_per_volume() {
        // Pins the cross-volume routing the writer-drain clear and hold/release
        // rely on: a non-root volume id must NOT resolve to the root tracker.
        let _guard = PENDING_SIZES_TEST_MUTEX.lock().unwrap();
        let root_tracker = Arc::new(PendingSizes::new());
        install_pending_sizes(ROOT_VOLUME_ID, Arc::clone(&root_tracker));
        // Root id routes to the installed root tracker.
        let via_root = get_pending_sizes_for(ROOT_VOLUME_ID).expect("root tracker installed");
        assert!(Arc::ptr_eq(&via_root, &root_tracker));
        // A non-root id with nothing installed resolves to None, never to root.
        assert!(
            get_pending_sizes_for("smb://no-such-volume").is_none(),
            "a non-root id must not fall through to the root tracker"
        );
        // An installed non-root id routes to its OWN tracker.
        let smb_tracker = Arc::new(PendingSizes::new());
        install_pending_sizes("smb://writes-its-own", Arc::clone(&smb_tracker));
        let via_smb = get_pending_sizes_for("smb://writes-its-own").expect("smb tracker installed");
        assert!(Arc::ptr_eq(&via_smb, &smb_tracker));
        assert!(
            !Arc::ptr_eq(&via_smb, &root_tracker),
            "a non-root volume must not share root's tracker"
        );
        uninstall_pending_sizes("smb://writes-its-own");
        assert!(get_pending_sizes_for("smb://writes-its-own").is_none());
        uninstall_pending_sizes(ROOT_VOLUME_ID);
    }

    #[test]
    fn global_get_returns_none_when_uninstalled() {
        let _guard = PENDING_SIZES_TEST_MUTEX.lock().unwrap();
        uninstall_pending_sizes(ROOT_VOLUME_ID);
        assert!(get_pending_sizes().is_none());
    }

    #[test]
    fn global_install_and_clear_roundtrip() {
        let _guard = PENDING_SIZES_TEST_MUTEX.lock().unwrap();
        install_pending_sizes(ROOT_VOLUME_ID, Arc::new(PendingSizes::new()));
        let tracker = get_pending_sizes().expect("installed");
        tracker.mark("/aaa/bbb");
        assert!(tracker.is_pending("/aaa/bbb"));
        uninstall_pending_sizes(ROOT_VOLUME_ID);
        assert!(get_pending_sizes().is_none());
    }
}
