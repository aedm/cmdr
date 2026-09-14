//! Knowing when a volume's index has let go of the volume.
//!
//! A [`VolumeHold`] is a share of one start's stake in a volume. The start's
//! reservation takes the first share; the registry instance, the manager it builds,
//! and the workers that read the drive carry more. The registry can't answer "is
//! anything still working on this volume?" on its own: a teardown that meets
//! `Initializing` frees the key while the start behind it is still standing its
//! manager up, a drain or a claimed teardown runs behind a transient phase that says
//! nothing about when it ends, and a cancelled worker can keep reading after its
//! manager is gone. The hold answers in every one of those windows.
//!
//! ## Generations
//!
//! Every reservation starts a new GENERATION, and the table counts shares per
//! (volume id, generation, [`HoldKind`]), remembering which mounted filesystem the
//! generation's root was (its [`MountIdentity`]). A re-plugged drive keeps its volume
//! id, so without generations a worker stuck on the dead device would hold the drive's
//! next life hostage. A removable stop asks the host whether each generation's
//! filesystem is still mounted anywhere: a generation whose filesystem is gone is
//! VANISHED, and no wait counts it. Still held once that stop is done waiting, it
//! becomes a ZOMBIE: logged once, never counted again, and gone with its last share.
//!
//! ❌ **Never by path.** Renaming a mounted volume moves its mount point while the
//! filesystem stays mounted under it (verified on macOS 26.6.2, APFS and HFS+ images,
//! `cmdr_fs::testing::disk_images::real_images`, 2026-09-14), so a root missing from
//! the mount table is not a drive that's gone.
//!
//! A leaf beside `volume.rs` and `metadata.rs`, because the scanner, reconcile, and
//! watch workers carry shares and nothing below `lifecycle` may import
//! `lifecycle::state`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Condvar, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use cmdr_fs::ignore_poison::IgnorePoison;
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::indexing::host::volumes::MountIdentity;
use crate::indexing::volume::VolumeId;

/// What a share of a volume's hold is for, so a stop that runs out of time can name
/// the work still reading the drive.
///
/// One variant per place that spawns drive-reading work, plus the start itself.
#[expect(
    dead_code,
    reason = "a worker's variant is constructed once its spawn site carries a share"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum HoldKind {
    /// The start: its reservation, the registry instance, and the manager it builds.
    Reservation,
    /// A walker worker thread (`index-walk`), which its walk never joins.
    WalkerWorker,
    /// A full scan's thread (`index-scanner`).
    Scanner,
    /// A full local rescan's walk (`index-local-reconcile`).
    LocalReconcile,
    /// A local rescan's directory reader (`reconcile-read`), abandoned on a timeout.
    ReconcileRead,
    /// The scan completion task, its replay included.
    ScanCompletion,
    /// The live event loop, which can outlive `shutdown`'s five-second drain.
    LiveLoop,
    /// A subtree rescan (`rescan-subtree`).
    SubtreeRescan,
    /// The phase machine (`index-phases`).
    Phases,
    /// A background cover walk (`index-cover`, `WalkFor::TheIndex`): the phase
    /// machine's.
    PhaseCover,
    /// A cover walk somebody waits on (`index-cover`, `WalkFor::TheUser`): a search's,
    /// which runs on the search's token and is linked to the volume's.
    SearchCover,
    /// A verifier task and its `scan_subtree` walk.
    Verifier,
    /// A cover bootstrap's mount probe (`index-mount-probe`).
    MountProbe,
}

/// Which of a volume's lives a share belongs to: one per reservation.
type Generation = u64;

/// Whether a generation's drive is still there, as far as a removable stop could tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// Mounted, or never asked about. Every wait counts it.
    Live,
    /// A removable stop found its filesystem mounted nowhere. No wait counts it.
    Vanished,
    /// Vanished, and still held when the stop that found it gone was done waiting.
    /// Logged once. ❌ Never counted again, even once a re-plugged drive mounts again:
    /// its stuck work is on the dead device, not the new one.
    Zombie,
}

/// The shares one generation has outstanding.
struct GenerationHolds {
    /// The filesystem its root was when it was reserved, or `None` when the start
    /// couldn't name one (not a local-scanner volume, a root that isn't a mount point,
    /// an unreadable table). A generation without one is never asked about, so never
    /// flagged.
    identity: Option<MountIdentity>,
    standing: Standing,
    /// Outstanding shares per kind. A kind with none has no entry, and a generation
    /// with none has no entry either.
    shares: BTreeMap<HoldKind, usize>,
}

/// Every volume's outstanding shares, by generation.
#[derive(Default)]
struct Table {
    /// The generation the next reservation takes.
    next_generation: Generation,
    /// A volume nothing holds has no entry.
    volumes: HashMap<VolumeId, BTreeMap<Generation, GenerationHolds>>,
}

impl Table {
    /// The shares of `volume_id`'s live generations, summed per kind, in kind order.
    fn live_holders(&self, volume_id: &str) -> Vec<(HoldKind, usize)> {
        let mut holders = BTreeMap::new();
        for (kind, count) in self
            .live_generations(volume_id)
            .flat_map(|generation| &generation.shares)
        {
            *holders.entry(*kind).or_insert(0) += count;
        }
        holders.into_iter().collect()
    }

    /// Whether any live generation holds `volume_id`.
    fn is_live_held(&self, volume_id: &str) -> bool {
        self.live_generations(volume_id).next().is_some()
    }

    fn live_generations(&self, volume_id: &str) -> impl Iterator<Item = &GenerationHolds> {
        self.volumes
            .get(volume_id)
            .into_iter()
            .flat_map(BTreeMap::values)
            .filter(|generation| generation.standing == Standing::Live)
    }
}

/// The table, and the signal a generation leaving the count sends: its last share
/// dropping, or a stop finding its drive gone.
///
/// A LEAF lock, like the read-handle tables: the reservation takes it while it holds
/// `INDEX_REGISTRY`, and nothing takes the registry while holding it. ❌ Nothing
/// blocks under it either: the mount-table reads happen off it.
struct Holds {
    table: Mutex<Table>,
    changed: Condvar,
}

static HOLDS: LazyLock<Holds> = LazyLock::new(|| Holds {
    table: Mutex::new(Table::default()),
    changed: Condvar::new(),
});

/// A share of one start's stake in a volume, for one piece of work.
///
/// ⚠️ **The first share is taken inside the critical section that reserves the
/// slot** (`lifecycle/state/reservation.rs`). Taken after it, a stop could free the
/// slot, find nothing holding the volume, and answer "released" while the start goes
/// on to stand a manager up.
///
/// Shares ride the things doing the work and drop with them: the registry instance,
/// the `IndexManager` (which drops after `shutdown` on every path: a drain, a claimed
/// teardown, a start whose slot was taken away, a start that failed), and the
/// workers. ❌ Don't release one by hand anywhere else; riding the owner's own drop
/// is what keeps a new teardown path from forgetting it.
pub(crate) struct VolumeHold {
    volume_id: VolumeId,
    generation: Generation,
    kind: HoldKind,
}

impl VolumeHold {
    /// Start a new generation's stake in `volume_id`, whose root is the filesystem
    /// `identity` names.
    fn take(volume_id: &str, identity: Option<MountIdentity>) -> Self {
        let mut table = HOLDS.table.lock_ignore_poison();
        let generation = table.next_generation;
        table.next_generation += 1;
        table.volumes.entry(volume_id.to_string()).or_default().insert(
            generation,
            GenerationHolds {
                identity,
                standing: Standing::Live,
                shares: BTreeMap::from([(HoldKind::Reservation, 1)]),
            },
        );
        Self {
            volume_id: volume_id.to_string(),
            generation,
            kind: HoldKind::Reservation,
        }
    }

    /// Another share of this generation, for work of `kind`.
    fn share(&self, kind: HoldKind) -> Self {
        let mut table = HOLDS.table.lock_ignore_poison();
        // Always there: `self` is one of its shares, so the generation can't have gone.
        if let Some(generation) = table
            .volumes
            .get_mut(&self.volume_id)
            .and_then(|generations| generations.get_mut(&self.generation))
        {
            *generation.shares.entry(kind).or_insert(0) += 1;
        }
        Self {
            volume_id: self.volume_id.clone(),
            generation: self.generation,
            kind,
        }
    }
}

impl Clone for VolumeHold {
    fn clone(&self) -> Self {
        self.share(self.kind)
    }
}

impl Drop for VolumeHold {
    fn drop(&mut self) {
        let mut table = HOLDS.table.lock_ignore_poison();
        let Some(generations) = table.volumes.get_mut(&self.volume_id) else {
            return;
        };
        let Some(generation) = generations.get_mut(&self.generation) else {
            return;
        };
        if let Some(count) = generation.shares.get_mut(&self.kind) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                generation.shares.remove(&self.kind);
            }
        }
        if !generation.shares.is_empty() {
            return;
        }
        generations.remove(&self.generation);
        if generations.is_empty() {
            table.volumes.remove(&self.volume_id);
        }
        // One condvar serves every volume, so this wakes waiters on other volumes
        // too; each re-checks its own before it answers.
        HOLDS.changed.notify_all();
    }
}

/// Drive-reading work and its share of the volume's hold, as ONE value.
///
/// The pairing is the point: work that takes a `VolumeWork` can't stop with the
/// volume without holding it, or hold it without hearing the stop. ❌ Don't hand a
/// thread the bare `cancel`; move the whole value in, so the share drops when the
/// work does.
pub(crate) struct VolumeWork {
    /// Fires when this work should stop.
    pub(crate) cancel: CancellationToken,
    /// This work's share of its volume's hold.
    hold: VolumeHold,
    /// For [`linked`](Self::linked) work: ends the task that forwards the volume's
    /// stop, once this work and every clone and child of it are gone.
    link: Option<Arc<DropGuard>>,
}

impl VolumeWork {
    /// The root work of a new reservation of `volume_id`: a fresh stop signal and the
    /// new generation's first share, remembering the filesystem `identity` names.
    ///
    /// ⚠️ Only the reservation calls this, INSIDE its critical section (see
    /// [`VolumeHold`]).
    pub(crate) fn take(volume_id: &str, identity: Option<MountIdentity>) -> Self {
        Self {
            cancel: CancellationToken::new(),
            hold: VolumeHold::take(volume_id, identity),
            link: None,
        }
    }

    /// Work under this one: a child of its stop signal, sharing its generation as work
    /// of `kind`.
    pub(crate) fn child(&self, kind: HoldKind) -> Self {
        Self {
            cancel: self.cancel.child_token(),
            hold: self.hold.share(kind),
            link: self.link.clone(),
        }
    }

    /// Work a CALLER started on `volume`, like a search's cover walk: a child of
    /// `caller`, so the caller still stops it on its own (a preemption, a cancelled
    /// search), that also stops when the volume's stop signal fires, which the
    /// caller's token never hears.
    ///
    /// The share is taken now, so the volume is held from the moment this returns.
    /// The volume's stop reaches the work through a small forwarding task, one
    /// scheduler hop later; the task ends as soon as either token fires or the work
    /// is gone.
    pub(crate) fn linked(caller: &CancellationToken, volume: &VolumeWork, kind: HoldKind) -> Self {
        let cancel = caller.child_token();
        let finished = CancellationToken::new();
        let link = Arc::new(finished.clone().drop_guard());
        let volume_stop = volume.cancel.clone();
        let stopped = cancel.clone();
        let to_stop = cancel.clone();
        crate::indexing::host::runtime::spawn(async move {
            tokio::select! {
                () = volume_stop.cancelled() => to_stop.cancel(),
                () = stopped.cancelled() => {}
                () = finished.cancelled() => {}
            }
        });
        Self {
            cancel,
            hold: volume.hold.share(kind),
            link: Some(link),
        }
    }

    /// Root work for a manager a test builds without a registry slot: a generation
    /// with no mount identity, which no mount-table read ever flags.
    #[cfg(test)]
    pub(crate) fn for_test(volume_id: &str) -> Self {
        Self::take(volume_id, None)
    }
}

impl Clone for VolumeWork {
    fn clone(&self) -> Self {
        Self {
            cancel: self.cancel.clone(),
            hold: self.hold.clone(),
            link: self.link.clone(),
        }
    }
}

/// What waiting for a volume to be let go came to.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Release {
    /// No live generation holds the volume.
    Released,
    /// The wait ran out with these shares of live generations outstanding, summed
    /// per kind, in kind order.
    StillHeld(Vec<(HoldKind, usize)>),
}

/// Whether any live generation holds `volume_id` right now.
pub(crate) fn is_held(volume_id: &str) -> bool {
    HOLDS.table.lock_ignore_poison().is_live_held(volume_id)
}

/// Wait up to `wait` for no live generation to hold `volume_id`, answering what the
/// wait came to.
///
/// Woken when a generation leaves the count, ❌ never by polling. A zero `wait`
/// answers at once from what the table holds right now.
pub(crate) fn wait_until_released(volume_id: &str, wait: Duration) -> Release {
    let table = HOLDS.table.lock_ignore_poison();
    // Recovering is right for the same reason `lock_ignore_poison` is: this is a
    // count table, and no critical section here can panic part way through.
    let (table, _) = HOLDS
        .changed
        .wait_timeout_while(table, wait, |table| table.is_live_held(volume_id))
        .unwrap_or_else(PoisonError::into_inner);
    let holders = table.live_holders(volume_id);
    if holders.is_empty() {
        Release::Released
    } else {
        Release::StillHeld(holders)
    }
}

/// Flag every live generation of `volume_id` whose filesystem `is_mounted` answers
/// `Some(false)` for as vanished, so no wait counts it from here on.
///
/// Only a generation with a mount identity is asked. ❌ `None`, a mount table that
/// couldn't be read, never flags anything: "don't know" must not let a stop answer
/// "released" over a worker still reading a mounted drive. One read per distinct
/// identity, taken off the lock.
pub(crate) fn flag_vanished(volume_id: &str, is_mounted: impl Fn(MountIdentity) -> Option<bool>) {
    let asked: Vec<(Generation, MountIdentity)> = HOLDS
        .table
        .lock_ignore_poison()
        .volumes
        .get(volume_id)
        .into_iter()
        .flatten()
        .filter(|(_, holds)| holds.standing == Standing::Live)
        .filter_map(|(generation, holds)| Some((*generation, holds.identity?)))
        .collect();
    if asked.is_empty() {
        return;
    }

    let mut answers: HashMap<MountIdentity, Option<bool>> = HashMap::new();
    let gone: Vec<Generation> = asked
        .into_iter()
        .filter(|(_, identity)| *answers.entry(*identity).or_insert_with(|| is_mounted(*identity)) == Some(false))
        .map(|(generation, _)| generation)
        .collect();
    if gone.is_empty() {
        return;
    }

    let mut flagged = Vec::new();
    {
        let mut table = HOLDS.table.lock_ignore_poison();
        let Some(generations) = table.volumes.get_mut(volume_id) else {
            return;
        };
        for generation in gone {
            // A generation whose last share dropped while we asked is already gone.
            if let Some(holds) = generations.get_mut(&generation)
                && holds.standing == Standing::Live
            {
                holds.standing = Standing::Vanished;
                flagged.push(generation);
            }
        }
    }
    if !flagged.is_empty() {
        log::info!("'{volume_id}': generation(s) {flagged:?} lost their drive, so no stop waits on them");
        HOLDS.changed.notify_all();
    }
}

/// Turn every vanished generation of `volume_id` that's still held into a zombie,
/// with one `warn` naming what holds them. A stop calls this once it's done waiting.
pub(crate) fn zombify_vanished(volume_id: &str) {
    let mut zombies: Vec<(Generation, Vec<(HoldKind, usize)>)> = Vec::new();
    if let Some(generations) = HOLDS.table.lock_ignore_poison().volumes.get_mut(volume_id) {
        for (generation, holds) in generations.iter_mut() {
            if holds.standing == Standing::Vanished {
                holds.standing = Standing::Zombie;
                zombies.push((
                    *generation,
                    holds.shares.iter().map(|(kind, count)| (*kind, *count)).collect(),
                ));
            }
        }
    }
    if !zombies.is_empty() {
        log::warn!(
            "'{volume_id}': a drive that's gone is still held by (generation, shares) {zombies:?}; none of it holds up the drive's next life"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::sync::mpsc;
    use std::time::Instant;

    use super::*;

    /// The filesystem the drive in these tests is.
    const DRIVE: MountIdentity = MountIdentity::from_raw(0x0100_0012);

    /// A start's first share of `volume_id`, on the drive [`DRIVE`] names.
    fn take(volume_id: &str) -> VolumeHold {
        VolumeHold::take(volume_id, Some(DRIVE))
    }

    fn standing_of(volume_id: &str, generation: Generation) -> Option<Standing> {
        HOLDS
            .table
            .lock_ignore_poison()
            .volumes
            .get(volume_id)
            .and_then(|generations| generations.get(&generation))
            .map(|generation| generation.standing)
    }

    #[test]
    fn a_volume_nothing_holds_is_let_go_at_once() {
        assert!(!is_held("release-test-never-held"));
        assert_eq!(
            wait_until_released("release-test-never-held", Duration::ZERO),
            Release::Released
        );
    }

    #[test]
    fn every_hold_has_to_let_go_before_the_volume_does() {
        let volume_id = "release-test-two-holds";
        let first = take(volume_id);
        let second = take(volume_id);

        drop(first);
        assert!(is_held(volume_id), "a second start still holds the volume");
        assert!(
            matches!(wait_until_released(volume_id, Duration::ZERO), Release::StillHeld(_)),
            "and a wait that ran out says so"
        );
        drop(second);
        assert_eq!(
            wait_until_released(volume_id, Duration::ZERO),
            Release::Released,
            "the last hold let go"
        );
    }

    /// The wait is a subscription: the drop wakes it. A waiter that only noticed at
    /// its deadline would still answer "released", so the time it took is the
    /// assertion.
    #[test]
    fn a_waiter_wakes_as_the_last_hold_lets_go() {
        let volume_id = "release-test-wake";
        let wait = Duration::from_secs(5);
        let hold = take(volume_id);
        let (waiting, waiter_started) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            waiting.send(()).expect("the test is listening");
            let started = Instant::now();
            let released = wait_until_released(volume_id, wait);
            (released, started.elapsed())
        });
        waiter_started.recv().expect("the waiter starts");

        drop(hold);
        let (released, took) = waiter.join().expect("the waiter doesn't panic");

        assert_eq!(released, Release::Released, "the volume was let go inside the wait");
        assert!(
            took < wait,
            "the drop has to wake the waiter, not leave it to its deadline (took {took:?})"
        );
    }

    /// Each kind of work counts on its own, so an answer can say WHICH work still
    /// holds the drive, and a generation outlives its manager in its workers.
    #[test]
    fn every_kind_of_share_is_counted_on_its_own() {
        let volume_id = "hold-test-kinds";
        let reservation = take(volume_id);
        let walker = reservation.share(HoldKind::WalkerWorker);
        let other_walker = walker.clone();
        let live_loop = reservation.share(HoldKind::LiveLoop);

        assert_eq!(
            wait_until_released(volume_id, Duration::ZERO),
            Release::StillHeld(vec![
                (HoldKind::Reservation, 1),
                (HoldKind::WalkerWorker, 2),
                (HoldKind::LiveLoop, 1),
            ])
        );

        drop(reservation);
        drop(walker);
        assert_eq!(
            wait_until_released(volume_id, Duration::ZERO),
            Release::StillHeld(vec![(HoldKind::WalkerWorker, 1), (HoldKind::LiveLoop, 1)]),
            "the manager is gone, and its workers still hold the drive"
        );

        drop(other_walker);
        drop(live_loop);
        assert_eq!(wait_until_released(volume_id, Duration::ZERO), Release::Released);
    }

    /// A wait that runs out names what's still holding the drive: the one clue a log
    /// reader gets for an eject that answered "still releasing".
    #[test]
    fn a_wait_that_runs_out_names_what_still_holds_the_volume() {
        let volume_id = "hold-test-names";
        let reservation = take(volume_id);
        let stuck_read = reservation.share(HoldKind::ReconcileRead);
        drop(reservation);

        assert_eq!(
            wait_until_released(volume_id, Duration::from_millis(10)),
            Release::StillHeld(vec![(HoldKind::ReconcileRead, 1)])
        );
        drop(stuck_read);
    }

    /// The re-plugged drive: same UUID, same volume id, and a worker from its last
    /// life still stuck on the dead device. Once a stop finds that life's filesystem
    /// gone, the stuck share stops counting, the next life's stop answers from its own
    /// shares alone, and the stuck share turns zombie when that stop is done waiting.
    #[test]
    fn a_stuck_share_of_a_drive_that_left_never_blocks_its_next_life() {
        let volume_id = "hold-test-replugged";
        let first_life = take(volume_id);
        let stuck = first_life.share(HoldKind::WalkerWorker);
        let first_generation = stuck.generation;
        drop(first_life);

        flag_vanished(volume_id, |identity| {
            assert_eq!(identity, DRIVE);
            Some(false)
        });
        assert!(!is_held(volume_id), "a generation whose drive is gone holds nothing up");
        assert_eq!(wait_until_released(volume_id, Duration::ZERO), Release::Released);

        // The drive comes back, and a new start reserves it.
        let next_life = take(volume_id);
        flag_vanished(volume_id, |_| Some(true));
        assert_eq!(
            wait_until_released(volume_id, Duration::ZERO),
            Release::StillHeld(vec![(HoldKind::Reservation, 1)]),
            "only the next life counts"
        );
        zombify_vanished(volume_id);
        assert_eq!(
            standing_of(volume_id, first_generation),
            Some(Standing::Zombie),
            "still held once the stop was done waiting"
        );

        drop(next_life);
        assert_eq!(
            wait_until_released(volume_id, Duration::ZERO),
            Release::Released,
            "the dead drive's stuck share never counts again, though the drive is mounted again"
        );
        drop(stuck);
        assert_eq!(
            standing_of(volume_id, first_generation),
            None,
            "its last share took the zombie with it"
        );
    }

    /// A stop already waiting on a generation answers as soon as another stop finds
    /// that generation's drive gone, not at its deadline.
    #[test]
    fn a_waiter_wakes_when_the_generation_it_waits_on_vanishes() {
        let volume_id = "hold-test-vanish-wake";
        let wait = Duration::from_secs(5);
        let stuck = take(volume_id);
        let (waiting, waiter_started) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            waiting.send(()).expect("the test is listening");
            let started = Instant::now();
            (wait_until_released(volume_id, wait), started.elapsed())
        });
        waiter_started.recv().expect("the waiter starts");

        flag_vanished(volume_id, |_| Some(false));
        let (released, took) = waiter.join().expect("the waiter doesn't panic");

        assert_eq!(released, Release::Released);
        assert!(
            took < wait,
            "the flag has to wake the waiter, not leave it to its deadline (took {took:?})"
        );
        drop(stuck);
    }

    /// "Couldn't read the mount table" isn't "the drive is gone": reading it as gone
    /// would let a stop answer "released" over a worker still reading a mounted drive.
    #[test]
    fn an_unreadable_mount_table_never_flags_a_generation() {
        let volume_id = "hold-test-unreadable";
        let hold = take(volume_id);
        flag_vanished(volume_id, |_| None);
        assert!(is_held(volume_id));
        drop(hold);
    }

    /// One mount-table read per filesystem however many generations share it, and none
    /// for a generation whose start couldn't name one.
    #[test]
    fn a_stop_asks_about_each_filesystem_once_and_never_about_a_generation_without_one() {
        let volume_id = "hold-test-asks";
        let first = take(volume_id);
        let second = take(volume_id);
        let unnamed = VolumeHold::take(volume_id, None);
        let asked = RefCell::new(Vec::new());

        flag_vanished(volume_id, |identity| {
            asked.borrow_mut().push(identity);
            Some(false)
        });

        assert_eq!(asked.into_inner(), vec![DRIVE]);
        assert_eq!(
            wait_until_released(volume_id, Duration::ZERO),
            Release::StillHeld(vec![(HoldKind::Reservation, 1)]),
            "the generation without an identity was never asked, so it still counts"
        );
        drop((first, second, unnamed));
    }

    /// Work under a volume's root work stops with it and holds the same generation.
    #[test]
    fn a_child_holds_its_parents_generation_and_stops_with_it() {
        let volume_id = "hold-test-child";
        let volume = VolumeWork::for_test(volume_id);
        let child = volume.child(HoldKind::Verifier);
        assert_eq!(
            wait_until_released(volume_id, Duration::ZERO),
            Release::StillHeld(vec![(HoldKind::Reservation, 1), (HoldKind::Verifier, 1)])
        );

        volume.cancel.cancel();
        assert!(child.cancel.is_cancelled());
        drop(volume);
        assert_eq!(
            wait_until_released(volume_id, Duration::ZERO),
            Release::StillHeld(vec![(HoldKind::Verifier, 1)])
        );
        drop(child);
        assert_eq!(wait_until_released(volume_id, Duration::ZERO), Release::Released);
    }

    /// A search's cover walk stops when the search does AND when its volume does: the
    /// search's token alone never hears an eject.
    #[tokio::test]
    async fn a_linked_walk_stops_when_either_its_caller_or_its_volume_stops() {
        let volume_id = "hold-test-linked";
        let volume = VolumeWork::for_test(volume_id);

        let search = CancellationToken::new();
        let walk = VolumeWork::linked(&search, &volume, HoldKind::SearchCover);
        search.cancel();
        assert!(walk.cancel.is_cancelled(), "the search stops its walk at once");
        assert!(!volume.cancel.is_cancelled(), "and never the volume");

        let other_search = CancellationToken::new();
        let other_walk = VolumeWork::linked(&other_search, &volume, HoldKind::SearchCover);
        assert_eq!(
            wait_until_released(volume_id, Duration::ZERO),
            Release::StillHeld(vec![(HoldKind::Reservation, 1), (HoldKind::SearchCover, 2)]),
            "a walk holds the volume from the moment it's linked"
        );
        volume.cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), other_walk.cancel.cancelled())
            .await
            .expect("the volume's stop reaches the walk");
        assert!(!other_search.is_cancelled(), "without stopping the search");
    }
}
