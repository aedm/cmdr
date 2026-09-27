//! Free space for a live readout, without asking CacheDelete every tick.
//!
//! The figure the readout shows is `NSURLVolumeAvailableCapacityForImportantUsageKey`: it counts
//! purgeable space, so it matches Finder. On the boot volume it costs ~6.5 ms of CPU a call, most
//! of it in the kernel and the `deleted` daemon; `statfs` answers in microseconds but leaves the
//! purgeable part out. The two differ by that purgeable part alone, and it holds still while
//! ordinary writes move both by the same bytes. So a live reading is the last important-usage
//! figure plus how far `statfs` moved since, and the expensive query runs again only when that
//! stops being safe ([`plan`]). `DETAILS.md` § "Live volume space" has the evidence.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::file_system::volume::SpaceInfo;
use crate::ignore_poison::IgnorePoison;

/// The longest a live reading leans on one important-usage figure. Bounds how stale the
/// purgeable part can go when it moves without `statfs` moving (a file deleted from under a Time
/// Machine local snapshot, iCloud evicting a download), at ~0.01% of a core.
const MAX_ANCHOR_AGE: Duration = Duration::from_secs(60);

/// How far `statfs` may drift from the anchor, as a fraction of the drive, before the expensive
/// figure is taken again: about one step of the pane's readout (`space_poller/readout.rs`). A
/// drift this size can be macOS purging (free space rising while the important-usage figure holds
/// still), which a derived reading would show as room that isn't new.
const MAX_DRIFT_DIVISOR: u64 = 1000;

/// `statfs` for the filesystem: the cheap figures, microseconds a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CheapReading {
    total_bytes: u64,
    free_bytes: u64,
}

/// The last important-usage reading of one filesystem, with the `statfs` taken beside it.
#[derive(Debug, Clone, Copy)]
struct Anchor {
    total_bytes: u64,
    available_bytes: u64,
    cheap: CheapReading,
    taken_at: Instant,
    /// [`STALE_EPOCH`] when it was taken; a bump retires every anchor.
    epoch: u64,
}

/// What a live reading does with a fresh [`CheapReading`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    /// Ask for the important-usage figure again and anchor on it.
    Refresh,
    /// The anchor still holds: this is the reading.
    Derived(SpaceInfo),
}

/// Bumped when a write operation settles, so the next live reading of every filesystem asks for
/// the important-usage figure again.
static STALE_EPOCH: AtomicU64 = AtomicU64::new(0);

/// One anchor per filesystem, keyed by its mount point (`statfs`'s `f_mntonname`), so every volume
/// on one filesystem shares it. An unmounted filesystem's entry stays behind: a handful of entries
/// at most, and a remount at the same path still has to pass [`plan`]'s total, drift, and age checks.
static ANCHORS: LazyLock<Mutex<HashMap<String, Anchor>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The space a live readout shows for the volume holding `path`: what
/// [`get_volume_space`](super::get_volume_space) would say, at `statfs` cost on most calls.
///
/// Only for readouts that poll. A caller that acts on the number once (the copy pre-flight) asks
/// [`get_volume_space`](super::get_volume_space) directly.
pub fn live_volume_space(path: &str) -> Option<SpaceInfo> {
    let Some((mount_point, cheap)) = statfs_reading(path) else {
        return super::get_volume_space(path);
    };
    let epoch = STALE_EPOCH.load(Ordering::Acquire);
    let now = Instant::now();
    let anchor = ANCHORS.lock_ignore_poison().get(&mount_point).copied();
    match plan(anchor.as_ref(), cheap, now, epoch) {
        Plan::Derived(space) => Some(space),
        Plan::Refresh => {
            let space = super::get_volume_space(path)?;
            if let SpaceInfo::Bounded {
                total_bytes,
                available_bytes,
                ..
            } = space
            {
                let anchor = Anchor {
                    total_bytes,
                    available_bytes,
                    cheap,
                    taken_at: now,
                    epoch,
                };
                ANCHORS.lock_ignore_poison().insert(mount_point, anchor);
            }
            Some(space)
        }
    }
}

/// Retires every anchor: the next live reading of each filesystem takes the important-usage
/// figure again. Called when a write operation settles, since a delete under a local snapshot
/// frees purgeable space that `statfs` never sees.
pub fn expect_space_change() {
    STALE_EPOCH.fetch_add(1, Ordering::Release);
}

/// The live-reading policy: derive from the anchor while it's fresh, same-shaped, and close.
fn plan(anchor: Option<&Anchor>, cheap: CheapReading, now: Instant, epoch: u64) -> Plan {
    let Some(anchor) = anchor else { return Plan::Refresh };
    let holds = anchor.epoch == epoch
        && now.saturating_duration_since(anchor.taken_at) < MAX_ANCHOR_AGE
        && cheap.total_bytes == anchor.cheap.total_bytes
        && cheap.free_bytes.abs_diff(anchor.cheap.free_bytes) < anchor.total_bytes / MAX_DRIFT_DIVISOR;
    if !holds {
        return Plan::Refresh;
    }
    let available = if cheap.free_bytes >= anchor.cheap.free_bytes {
        anchor
            .available_bytes
            .saturating_add(cheap.free_bytes - anchor.cheap.free_bytes)
    } else {
        anchor
            .available_bytes
            .saturating_sub(anchor.cheap.free_bytes - cheap.free_bytes)
    };
    Plan::Derived(SpaceInfo::bounded(
        anchor.total_bytes,
        available.min(anchor.total_bytes),
    ))
}

/// `statfs` for `path`: the filesystem's mount point and its cheap figures. `None` when the call fails.
fn statfs_reading(path: &str) -> Option<(String, CheapReading)> {
    let c_path = std::ffi::CString::new(path).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: `c_path` is a valid NUL-terminated C string, and `stat` is an uninitialized but
    // correctly-typed `libc::statfs` out-buffer the kernel fills on success.
    if unsafe { libc::statfs(c_path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: `statfs` returned 0, so the kernel fully initialized `stat`.
    let stat = unsafe { stat.assume_init() };
    let mount_point = super::fs_type::statfs_string(&stat.f_mntonname);
    let block = u64::from(stat.f_bsize);
    Some((
        mount_point,
        CheapReading {
            total_bytes: stat.f_blocks.saturating_mul(block),
            free_bytes: stat.f_bavail.saturating_mul(block),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1_000_000_000;
    const MB: u64 = 1_000_000;
    /// The boot volume this was measured on: 995 GB, 345 GB free to `statfs`, 10.7 GB purgeable.
    const TOTAL: u64 = 995 * GB;
    const STATFS_FREE: u64 = 345 * GB;
    const IMPORTANT_FREE: u64 = STATFS_FREE + 10_700 * MB;

    fn anchor(at: Instant) -> Anchor {
        Anchor {
            total_bytes: TOTAL,
            available_bytes: IMPORTANT_FREE,
            cheap: CheapReading {
                total_bytes: TOTAL,
                free_bytes: STATFS_FREE,
            },
            taken_at: at,
            epoch: 0,
        }
    }

    fn cheap(free_bytes: u64) -> CheapReading {
        CheapReading {
            total_bytes: TOTAL,
            free_bytes,
        }
    }

    #[test]
    fn the_boot_volume_reads_what_the_expensive_query_says() {
        let direct = super::super::get_volume_space("/").expect("macOS reports space for the boot volume");
        expect_space_change();
        let refreshed = live_volume_space("/").expect("a live reading of the boot volume");
        let derived = live_volume_space("/").expect("a second live reading of the boot volume");
        for live in [refreshed, derived] {
            let (Some(want), Some(got)) = (direct.available_bytes(), live.available_bytes()) else {
                panic!("the boot volume is bounded: {direct:?} vs {live:?}");
            };
            // Other processes write between the calls; the drift allowance bounds how far apart
            // they may read.
            assert!(want.abs_diff(got) < 1_000_000_000, "direct {want}, live {got}");
            assert_eq!(direct.total_bytes(), live.total_bytes());
        }
    }

    #[test]
    fn the_first_reading_asks_for_the_real_figure() {
        assert_eq!(plan(None, cheap(STATFS_FREE), Instant::now(), 0), Plan::Refresh);
    }

    #[test]
    fn a_write_moves_the_reading_by_exactly_what_it_wrote() {
        let t0 = Instant::now();
        let got = plan(Some(&anchor(t0)), cheap(STATFS_FREE - 300 * MB), t0 + Duration::from_secs(2), 0);
        assert_eq!(got, Plan::Derived(SpaceInfo::bounded(TOTAL, IMPORTANT_FREE - 300 * MB)));
    }

    #[test]
    fn a_delete_moves_the_reading_up_by_what_it_freed() {
        let t0 = Instant::now();
        let got = plan(Some(&anchor(t0)), cheap(STATFS_FREE + 200 * MB), t0 + Duration::from_secs(2), 0);
        assert_eq!(got, Plan::Derived(SpaceInfo::bounded(TOTAL, IMPORTANT_FREE + 200 * MB)));
    }

    #[test]
    fn drifting_a_readout_step_asks_again() {
        // A gigabyte on a 995 GB drive: past one step, so it may be macOS purging.
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(2);
        assert_eq!(plan(Some(&anchor(t0)), cheap(STATFS_FREE + GB), later, 0), Plan::Refresh);
        assert_eq!(plan(Some(&anchor(t0)), cheap(STATFS_FREE - GB), later, 0), Plan::Refresh);
    }

    #[test]
    fn an_old_anchor_asks_again_even_when_nothing_moved() {
        let t0 = Instant::now();
        assert!(matches!(
            plan(Some(&anchor(t0)), cheap(STATFS_FREE), t0 + MAX_ANCHOR_AGE - Duration::from_secs(1), 0),
            Plan::Derived(_)
        ));
        assert_eq!(
            plan(Some(&anchor(t0)), cheap(STATFS_FREE), t0 + MAX_ANCHOR_AGE, 0),
            Plan::Refresh
        );
    }

    #[test]
    fn a_finished_write_operation_retires_the_anchor() {
        // Deleting a file a local snapshot still holds frees nothing to `statfs`, but the
        // important-usage figure grows: only a fresh query shows it.
        let t0 = Instant::now();
        assert_eq!(
            plan(Some(&anchor(t0)), cheap(STATFS_FREE), t0 + Duration::from_secs(2), 1),
            Plan::Refresh
        );
    }

    #[test]
    fn a_resized_volume_asks_again() {
        let t0 = Instant::now();
        let resized = CheapReading {
            total_bytes: TOTAL + GB,
            free_bytes: STATFS_FREE,
        };
        assert_eq!(
            plan(Some(&anchor(t0)), resized, t0 + Duration::from_secs(2), 0),
            Plan::Refresh
        );
    }

    #[test]
    fn a_derived_reading_stays_inside_the_drive() {
        // A tiny drive where one step is 1 MB, and an anchor that says it's all but full.
        let t0 = Instant::now();
        let small = Anchor {
            total_bytes: GB,
            available_bytes: 100_000,
            cheap: CheapReading {
                total_bytes: GB,
                free_bytes: 400_000,
            },
            taken_at: t0,
            epoch: 0,
        };
        let got = plan(
            Some(&small),
            CheapReading {
                total_bytes: GB,
                free_bytes: 0,
            },
            t0 + Duration::from_secs(2),
            0,
        );
        assert_eq!(got, Plan::Derived(SpaceInfo::bounded(GB, 0)));
    }
}
