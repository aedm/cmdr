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
/// ❌ No variant for a volume-classification probe (a cover bootstrap's or a start's
/// `statfs`): it runs before any reservation, so there's no generation to share, and
/// the host's `drive_release` ticket spans it instead.
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

    /// Whether this generation's filesystem is still in the host's mount table.
    ///
    /// The question every delete gate asks AFTER its observation, ❌ never before: an
    /// unmounted `/Volumes/X` whose mount-point folder survives lists as empty and
    /// complete, so a read taken first would let every top-level child be deleted.
    ///
    /// **Two different "don't knows", answered differently.** A generation whose start
    /// couldn't name a filesystem (no host, a root that isn't a mount point, an
    /// unreadable table then) is never asked and reads PRESENT, so hostless tools and
    /// tests delete exactly as they do today. A generation that HAS an identity but
    /// whose table can't be read now reads GONE: a delete needs `Some(true)`, and
    /// "don't know" must never authorize one.
    pub(crate) fn drive_is_listed(&self) -> bool {
        // Read the identity out and drop the table lock before asking the host:
        // ❌ nothing blocks under this lock (see [`Holds`]).
        let identity = HOLDS
            .table
            .lock_ignore_poison()
            .volumes
            .get(&self.volume_id)
            .and_then(|generations| generations.get(&self.generation))
            .and_then(|generation| generation.identity);
        let Some(identity) = identity else {
            return true;
        };
        crate::indexing::host::volumes::current().is_mounted(identity) == Some(true)
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

    /// Root work for a test that needs the delete gate to actually ask the host: a
    /// generation that captured `identity`, the way a real `LocalExternal` start
    /// does. ⚠️ Pair it with a provider that has `identity` mounted — [`for_test`]
    /// captures none, so every gate reads the drive as present and a gate test
    /// built on it would pass vacuously.
    #[cfg(test)]
    pub(crate) fn for_test_on(volume_id: &str, identity: MountIdentity) -> Self {
        Self::take(volume_id, Some(identity))
    }

    /// The volume this work reads, for the gates that record a delete batch against
    /// it.
    pub(crate) fn volume_id(&self) -> &str {
        &self.hold.volume_id
    }

    /// Whether this work's generation still has its drive; see
    /// [`VolumeHold::drive_is_listed`] for the two kinds of "don't know".
    pub(crate) fn drive_is_listed(&self) -> bool {
        self.hold.drive_is_listed()
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
/// `Some(false)` for as vanished, so no wait counts it from here on. Reports whether
/// it flagged any, which is a stop's evidence that this drive really went away
/// rather than being let go of.
///
/// Only a generation with a mount identity is asked. ❌ `None`, a mount table that
/// couldn't be read, never flags anything: "don't know" must not let a stop answer
/// "released" over a worker still reading a mounted drive. One read per distinct
/// identity, taken off the lock.
pub(crate) fn flag_vanished(volume_id: &str, is_mounted: impl Fn(MountIdentity) -> Option<bool>) -> bool {
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
        return false;
    }

    let mut answers: HashMap<MountIdentity, Option<bool>> = HashMap::new();
    let gone: Vec<Generation> = asked
        .into_iter()
        .filter(|(_, identity)| *answers.entry(*identity).or_insert_with(|| is_mounted(*identity)) == Some(false))
        .map(|(generation, _)| generation)
        .collect();
    if gone.is_empty() {
        return false;
    }

    let mut flagged = Vec::new();
    {
        let mut table = HOLDS.table.lock_ignore_poison();
        let Some(generations) = table.volumes.get_mut(volume_id) else {
            // Every generation we asked about has let go since. Their drive still
            // went away, which is what the caller acts on.
            return true;
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
    true
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
#[path = "hold_tests.rs"]
mod tests;
