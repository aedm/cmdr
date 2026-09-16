//! One eject at a time per volume, and one per physical disk. A request for a
//! volume whose eject is still running JOINS it and gets the same answer, with no
//! second teardown: a slow `diskutil` (10.5 s in one user's log) invites repeat
//! clicks, and every caller (the header chip, a dropdown row, the native menu,
//! MCP) lands in [`super::eject`].
//!
//! A disk flight takes its SIBLINGS into the set under its own flight
//! ([`join_or_own_disk`]), so an eject of a partition whose disk is already coming
//! down joins in [`join_or_start`], before anything else runs. A sibling whose own
//! flight had already started instead resolves to the same [`DiskKey`] and awaits
//! the disk flight from inside its own.
//!
//! The volumes with an eject in flight are the EJECTING set, pushed to the
//! frontend as `volumes-ejecting-changed` on every change and bootstrapped through
//! `get_ejecting_volume_ids`, the same shape as the busy set's
//! `volumes-busy-changed` (`write_operations/status_cache.rs`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};

use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};

use super::EjectError;
use crate::ignore_poison::IgnorePoison;

/// An eject in flight, awaitable by every caller that joined it.
pub(super) type Flight = Shared<BoxFuture<'static, Result<(), EjectError>>>;

/// One physical disk, by the registry entry ID of its whole media
/// (`IORegistryEntryGetRegistryEntryID`). Unique while the disk is attached, so two
/// volumes of one disk resolve to the same key and one teardown serves both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct DiskKey(pub(super) u64);

#[derive(Clone)]
struct InFlight {
    /// Tells this flight's own landing apart from a newer flight's for the same volume.
    flight_id: u64,
    flight: Flight,
}

/// What has an eject in flight: volumes by ID, and the physical disks a flight owns.
// DEFAULT-OK: nothing is being ejected before the first request.
#[derive(Default)]
struct InFlightSet {
    volumes: HashMap<String, InFlight>,
    #[cfg_attr(
        all(not(test), not(target_os = "macos")),
        expect(dead_code, reason = "only macOS ejects per physical disk")
    )]
    disks: HashMap<DiskKey, InFlight>,
}

static IN_FLIGHT: LazyLock<Mutex<InFlightSet>> = LazyLock::new(Mutex::default);
static NEXT_FLIGHT_ID: AtomicU64 = AtomicU64::new(0);

/// Typed `volumes-ejecting-changed` Tauri event. Wraps the ID list in a struct
/// because `tauri_specta::Event` payloads must be named types; the struct name
/// kebab-cases to the wire name.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct VolumesEjectingChanged {
    /// IDs of volumes whose eject is still running (sorted).
    pub volume_ids: Vec<String>,
}

/// App handle for emitting `volumes-ejecting-changed`. Absent in unit tests, where
/// the set is still queryable through [`ejecting_volume_ids`].
static EJECTING_APP: OnceLock<tauri::AppHandle> = OnceLock::new();

/// Stores the app handle used to broadcast `volumes-ejecting-changed`. Call once at
/// app setup, before the frontend can ask for an eject.
pub fn init_ejecting_volume_emitter(app: &tauri::AppHandle) {
    let _ = EJECTING_APP.set(app.clone());
}

/// The volumes whose eject is still running (sorted). Backs the
/// `get_ejecting_volume_ids` bootstrap and the native menus' disabled Eject item.
pub fn ejecting_volume_ids() -> Vec<String> {
    sorted_ids(&IN_FLIGHT.lock_ignore_poison())
}

/// Whether an eject of `volume_id` is in flight. The drive-release gate reads it
/// under its own lock, so this lock is never held while taking that one.
pub(crate) fn is_ejecting(volume_id: &str) -> bool {
    IN_FLIGHT.lock_ignore_poison().volumes.contains_key(volume_id)
}

/// Whether an eject OTHER than `flight_id`'s is taking `volume_id` down.
///
/// A disk flight asks this before it hands a sibling's index back: its own adopted
/// ids read as ejecting until it lands, so a plain [`is_ejecting`] would have the
/// flight refuse its own resume.
#[cfg_attr(
    all(not(test), not(target_os = "macos")),
    expect(dead_code, reason = "only macOS ejects per physical disk")
)]
pub(super) fn is_ejecting_by_another(volume_id: &str, flight_id: u64) -> bool {
    IN_FLIGHT
        .lock_ignore_poison()
        .volumes
        .get(volume_id)
        .is_some_and(|running| running.flight_id != flight_id)
}

/// Joins the eject in flight for `volume_id`, or starts one from `start`.
///
/// Synchronous on purpose: the join-or-start decision is made under the lock
/// before this returns, so no second request can slip in between and start a
/// second teardown. `start` is called only when nothing is in flight, and its
/// future runs on its own task, so it finishes even if every caller stops
/// waiting (an unmount can't be taken back halfway), and a panic inside it
/// answers [`EjectError::Unexpected`] instead of stranding the volume in the set.
pub(super) fn join_or_start<F, Fut>(volume_id: &str, start: F) -> Flight
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), EjectError>> + Send + 'static,
{
    let mut in_flight = IN_FLIGHT.lock_ignore_poison();
    if let Some(running) = in_flight.volumes.get(volume_id) {
        log::debug!(target: "eject", "An eject of {volume_id} is already running; joining it");
        return running.flight.clone();
    }

    let flight_id = NEXT_FLIGHT_ID.fetch_add(1, Ordering::Relaxed);
    let landing = Landing {
        volume_id: volume_id.to_string(),
        flight_id,
    };
    let work = start();
    let task = tokio::spawn(async move {
        // Dropped the moment the teardown ends, a panic included, and before any
        // caller sees the answer, so a caller never reads a stale "ejecting".
        let _landing = landing;
        work.await
    });
    let flight = async move {
        task.await.unwrap_or_else(|join_err| {
            Err(EjectError::Unexpected {
                detail: format!("the eject task failed: {join_err}"),
            })
        })
    }
    .boxed()
    .shared();

    in_flight.volumes.insert(
        volume_id.to_string(),
        InFlight {
            flight_id,
            flight: flight.clone(),
        },
    );
    emit_changed(&in_flight);
    flight
}

/// What a flight found when it asked to own its physical disk.
#[cfg_attr(
    all(not(test), not(target_os = "macos")),
    expect(dead_code, reason = "only macOS ejects per physical disk")
)]
pub(super) enum DiskFlight {
    /// This flight owns the disk: it took every sibling into the ejecting set and
    /// runs the one teardown. Dropping the ownership hands the adopted ids back.
    Owner(DiskOwnership),
    /// Another flight owns the disk. Await it and answer what it answers, so one
    /// `diskutil eject` serves every volume of the disk.
    Joined(Flight),
}

/// A flight's claim on one physical disk, plus the siblings it adopted.
#[cfg_attr(
    all(not(test), not(target_os = "macos")),
    expect(dead_code, reason = "only macOS ejects per physical disk")
)]
pub(super) struct DiskOwnership {
    key: DiskKey,
    flight_id: u64,
    /// One landing per adopted sibling. The flight's OWN landing belongs to its task.
    landings: Vec<Landing>,
}

#[cfg_attr(
    all(not(test), not(target_os = "macos")),
    expect(dead_code, reason = "only macOS ejects per physical disk")
)]
impl DiskOwnership {
    /// The owning flight's ID, which tells this flight's hold on a volume apart from
    /// a later flight's ([`is_ejecting_by_another`]).
    pub(super) fn flight_id(&self) -> u64 {
        self.flight_id
    }

    /// The siblings this ownership took into the ejecting set, for the log.
    pub(super) fn adopted(&self) -> Vec<String> {
        self.landings.iter().map(|landing| landing.volume_id.clone()).collect()
    }
}

impl Drop for DiskOwnership {
    fn drop(&mut self) {
        let mut in_flight = IN_FLIGHT.lock_ignore_poison();
        if in_flight
            .disks
            .get(&self.key)
            .is_some_and(|owner| owner.flight_id == self.flight_id)
        {
            in_flight.disks.remove(&self.key);
        }
        // The landings drop after this body, each taking the lock again to leave the
        // set and wake the gate.
    }
}

/// Claims `key` for the flight running for `volume_id`, taking `sibling_ids` into the
/// ejecting set under it, or joins the flight that already owns the disk.
///
/// Synchronous on purpose: the decision is made under the lock before this returns,
/// so two volumes of one disk can't each start a teardown. A sibling that already
/// has a flight of its own keeps it: that flight resolves to this same key and joins
/// here in turn.
#[cfg_attr(
    all(not(test), not(target_os = "macos")),
    expect(dead_code, reason = "only macOS ejects per physical disk")
)]
pub(super) fn join_or_own_disk(key: DiskKey, volume_id: &str, sibling_ids: &[String]) -> DiskFlight {
    let mut in_flight = IN_FLIGHT.lock_ignore_poison();
    let Some(mine) = in_flight.volumes.get(volume_id).cloned() else {
        // Only a caller that reached the pipeline without a flight of its own, which
        // nothing does today. Owning nothing is still correct: it runs its own teardown.
        log::warn!(
            target: "eject",
            "The eject of {volume_id} isn't in the ejecting set, so its disk flight adopts no siblings"
        );
        return DiskFlight::Owner(DiskOwnership {
            key,
            flight_id: NEXT_FLIGHT_ID.fetch_add(1, Ordering::Relaxed),
            landings: Vec::new(),
        });
    };
    if let Some(owner) = in_flight.disks.get(&key)
        && owner.flight_id != mine.flight_id
    {
        log::info!(target: "eject", "The disk under {volume_id} is already coming down; joining that flight");
        return DiskFlight::Joined(owner.flight.clone());
    }

    in_flight.disks.insert(key, mine.clone());
    let mut landings = Vec::new();
    for sibling_id in sibling_ids {
        if sibling_id == volume_id || in_flight.volumes.contains_key(sibling_id) {
            continue;
        }
        in_flight.volumes.insert(sibling_id.clone(), mine.clone());
        landings.push(Landing {
            volume_id: sibling_id.clone(),
            flight_id: mine.flight_id,
        });
    }
    emit_changed(&in_flight);
    DiskFlight::Owner(DiskOwnership {
        key,
        flight_id: mine.flight_id,
        landings,
    })
}

/// Takes its flight out of the ejecting set when the teardown ends.
struct Landing {
    volume_id: String,
    flight_id: u64,
}

impl Drop for Landing {
    fn drop(&mut self) {
        let landed = {
            let mut in_flight = IN_FLIGHT.lock_ignore_poison();
            let ours = in_flight
                .volumes
                .get(&self.volume_id)
                .is_some_and(|running| running.flight_id == self.flight_id);
            if ours {
                in_flight.volumes.remove(&self.volume_id);
                emit_changed(&in_flight);
            }
            ours
        };
        // A person's start waiting out this eject reads the set again. Outside the
        // lock: the gate takes its own lock first and reads this set under it.
        if landed {
            crate::file_system::volume::drive_release::notify_ejecting_changed();
        }
    }
}

fn sorted_ids(in_flight: &InFlightSet) -> Vec<String> {
    let mut ids: Vec<String> = in_flight.volumes.keys().cloned().collect();
    ids.sort();
    ids
}

/// Broadcasts the set. Called with the lock held, so two changes can't reach the
/// frontend out of order.
fn emit_changed(in_flight: &InFlightSet) {
    let Some(app) = EJECTING_APP.get() else {
        return;
    };
    use tauri_specta::Event as _;
    let payload = VolumesEjectingChanged {
        volume_ids: sorted_ids(in_flight),
    };
    if let Err(e) = payload.emit(app) {
        crate::log_error!(target: "eject", "Failed to emit volumes-ejecting-changed: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::wait_until_async;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn explode() -> Result<(), EjectError> {
        panic!("teardown blew up")
    }

    #[tokio::test]
    async fn a_second_eject_joins_the_one_in_flight_and_gets_its_answer() {
        let vid = "volumes-cmdr-test-joins-in-flight";
        let runs = Arc::new(AtomicUsize::new(0));
        let (release, held) = tokio::sync::oneshot::channel::<()>();

        let first = join_or_start(vid, {
            let runs = Arc::clone(&runs);
            move || async move {
                runs.fetch_add(1, Ordering::SeqCst);
                let _ = held.await;
                Err(EjectError::UnmountRefused {
                    holders: super::super::HolderScan::not_scanned(),
                    detail: "held by sleep".to_string(),
                })
            }
        });
        assert!(
            ejecting_volume_ids().contains(&vid.to_string()),
            "the volume reads as ejecting while its flight runs"
        );
        wait_until_async(Duration::from_secs(5), "the first teardown to start", || {
            runs.load(Ordering::SeqCst) == 1
        })
        .await;

        // Arrives while the first is still held: it must join, not run its own.
        let second = join_or_start(vid, {
            let runs = Arc::clone(&runs);
            move || async move {
                runs.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });

        let _ = release.send(());
        let (first, second) = tokio::join!(first, second);

        assert_eq!(
            runs.load(Ordering::SeqCst),
            1,
            "a joined request must not run a second teardown"
        );
        for result in [first, second] {
            assert!(
                matches!(result, Err(EjectError::UnmountRefused { ref detail, .. }) if detail == "held by sleep"),
                "both callers get the one flight's answer, got {result:?}"
            );
        }
        assert!(
            !ejecting_volume_ids().contains(&vid.to_string()),
            "the set clears once the flight lands"
        );
    }

    #[tokio::test]
    async fn a_request_after_the_flight_landed_starts_a_new_one() {
        // A retry after a refusal really retries: nothing caches the old answer.
        let vid = "volumes-cmdr-test-fresh-flight";
        let first = join_or_start(vid, || async { Err(EjectError::TimedOut) }).await;
        assert!(matches!(first, Err(EjectError::TimedOut)), "got {first:?}");

        let second = join_or_start(vid, || async { Ok(()) }).await;
        assert!(second.is_ok(), "got {second:?}");
    }

    // ── One flight per physical disk ──────────────────────────────────

    /// Runs `body` as a flight of `volume_id`, so it can own its disk the way
    /// `eject_now` does: `join_or_own_disk` reads the flight from the ejecting set.
    fn as_a_flight<F, Fut>(volume_id: &str, body: F) -> Flight
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(), EjectError>> + Send + 'static,
    {
        join_or_start(volume_id, body)
    }

    fn owned(flight: DiskFlight) -> DiskOwnership {
        match flight {
            DiskFlight::Owner(ownership) => ownership,
            DiskFlight::Joined(_) => panic!("expected to own the disk"),
        }
    }

    #[tokio::test]
    async fn a_disk_flight_takes_its_siblings_into_the_ejecting_set_so_their_own_eject_joins_it() {
        // A whose disk also carries B: B's eject must join A's flight in
        // `join_or_start`, BEFORE `is_already_unmounted` or any teardown of its own.
        let (a, b) = ("volumes-cmdr-test-disk-a", "volumes-cmdr-test-disk-b");
        let (release, held) = tokio::sync::oneshot::channel::<()>();
        let (owned_disk, disk_is_owned) = tokio::sync::oneshot::channel::<()>();
        let ran_alone = Arc::new(AtomicUsize::new(0));

        let first = as_a_flight(a, {
            let ran_alone = Arc::clone(&ran_alone);
            move || async move {
                ran_alone.fetch_add(1, Ordering::SeqCst);
                let ownership = owned(join_or_own_disk(DiskKey(4242), a, &[a.to_string(), b.to_string()]));
                assert_eq!(ownership.adopted(), [b.to_string()], "the sibling joined this flight");
                let _ = owned_disk.send(());
                let _ = held.await;
                drop(ownership);
                Err(EjectError::UnmountRefused {
                    holders: super::super::HolderScan::not_scanned(),
                    detail: "held by sleep".to_string(),
                })
            }
        });
        let _ = disk_is_owned.await;
        assert!(
            ejecting_volume_ids().contains(&b.to_string()),
            "an adopted sibling reads as ejecting, so no start lands on it"
        );

        let second = join_or_start(b, {
            let ran_alone = Arc::clone(&ran_alone);
            move || async move {
                ran_alone.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });

        let _ = release.send(());
        let (first, second) = tokio::join!(first, second);
        assert_eq!(ran_alone.load(Ordering::SeqCst), 1, "one teardown for the whole disk");
        for result in [first, second] {
            assert!(
                matches!(result, Err(EjectError::UnmountRefused { .. })),
                "both volumes get the disk flight's answer, got {result:?}"
            );
        }
        assert!(
            !ejecting_volume_ids().contains(&b.to_string()),
            "the adopted sibling leaves the set when the flight lands"
        );
    }

    #[tokio::test]
    async fn a_sibling_whose_own_flight_started_first_awaits_the_disk_flight_instead_of_tearing_down_again() {
        // The other order: B's own eject is already running when A resolves the same
        // disk. B keeps its flight, A owns the disk, and B awaits A's teardown.
        let (a, b) = ("volumes-cmdr-test-order-a", "volumes-cmdr-test-order-b");
        let key = DiskKey(9977);
        let (b_owns, b_owned_it) = tokio::sync::oneshot::channel::<()>();
        let (release, held) = tokio::sync::oneshot::channel::<()>();
        let teardowns = Arc::new(AtomicUsize::new(0));

        let b_flight = as_a_flight(b, {
            let teardowns = Arc::clone(&teardowns);
            move || async move {
                let ownership = owned(join_or_own_disk(key, b, &[b.to_string()]));
                let _ = b_owns.send(());
                let _ = held.await;
                teardowns.fetch_add(1, Ordering::SeqCst);
                drop(ownership);
                Err(EjectError::TimedOut)
            }
        });
        let _ = b_owned_it.await;

        let a_flight = as_a_flight(a, {
            let teardowns = Arc::clone(&teardowns);
            move || async move {
                match join_or_own_disk(key, a, &[a.to_string(), b.to_string()]) {
                    DiskFlight::Joined(running) => running.await,
                    DiskFlight::Owner(_) => {
                        teardowns.fetch_add(1, Ordering::SeqCst);
                        panic!("B's flight already owns this disk")
                    }
                }
            }
        });

        let _ = release.send(());
        let (a_result, b_result) = tokio::join!(a_flight, b_flight);
        assert_eq!(teardowns.load(Ordering::SeqCst), 1, "one teardown for the disk");
        assert!(matches!(a_result, Err(EjectError::TimedOut)), "got {a_result:?}");
        assert!(matches!(b_result, Err(EjectError::TimedOut)), "got {b_result:?}");
    }

    #[tokio::test]
    async fn a_flight_reads_its_own_hold_on_a_sibling_apart_from_a_later_flights() {
        // What a flight asks before it hands a sibling's index back: its own adopted
        // ids are ejecting until it lands, and refusing over that would strand them.
        let (a, b) = ("volumes-cmdr-test-owner-a", "volumes-cmdr-test-owner-b");
        let (done, is_done) = tokio::sync::oneshot::channel::<()>();
        let (release, held) = tokio::sync::oneshot::channel::<()>();

        let flight = as_a_flight(a, move || async move {
            let ownership = owned(join_or_own_disk(DiskKey(311), a, &[a.to_string(), b.to_string()]));
            assert!(
                !is_ejecting_by_another(b, ownership.flight_id()),
                "the flight's own adopted sibling is not someone else's eject"
            );
            assert!(
                is_ejecting_by_another(b, ownership.flight_id() + 1_000),
                "another flight's hold on the sibling is"
            );
            let _ = done.send(());
            let _ = held.await;
            drop(ownership);
            Ok(())
        });
        let _ = is_done.await;
        let _ = release.send(());
        assert!(flight.await.is_ok());
    }

    #[tokio::test]
    async fn a_teardown_that_panics_answers_unexpected_and_clears_the_set() {
        let vid = "volumes-cmdr-test-panicking-flight";
        let result = join_or_start(vid, || async { explode() }).await;
        assert!(matches!(result, Err(EjectError::Unexpected { .. })), "got {result:?}");
        assert!(
            !ejecting_volume_ids().contains(&vid.to_string()),
            "a panic can't strand the volume in the ejecting set"
        );
    }
}
