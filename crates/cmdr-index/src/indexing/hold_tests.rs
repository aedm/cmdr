//! What a [`VolumeHold`] answers: share counting per generation, the wait a
//! removable stop makes, and the vanished/zombie path a re-plugged drive takes.
//!
//! A `#[path]` child of `hold.rs`, so `super::` here is `hold` and its
//! module-private items are in reach.

use std::cell::RefCell;
use std::sync::mpsc;
use std::time::Instant;

use super::*;
use crate::indexing::host::volumes::{FakeVolumeProvider, VolumeProvider, install_for_test};

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

/// Install `provider` as the host for one test, serialized on the handle lock
/// because the provider slot is process-wide.
fn install(provider: &Arc<FakeVolumeProvider>) -> crate::indexing::host::volumes::TestProviderGuard {
    install_for_test(Arc::clone(provider) as Arc<dyn VolumeProvider>)
}

/// The one answer that authorizes a delete: the generation's filesystem is still
/// in the host's table.
#[test]
fn a_generation_whose_drive_is_listed_reads_as_present() {
    let _serialized = crate::indexing::handle::test_lock();
    let provider = FakeVolumeProvider::shared();
    provider.mount("/Volumes/Stick", DRIVE);
    let _installed = install(&provider);

    let hold = take("hold-test-gate-listed");
    assert!(hold.drive_is_listed());
}

/// The case every delete gate exists for: the drive left, so a listing that came
/// back short proves nothing and no row may be deleted against it.
#[test]
fn a_generation_whose_drive_left_reads_as_gone() {
    let _serialized = crate::indexing::handle::test_lock();
    let provider = FakeVolumeProvider::shared();
    provider.mount("/Volumes/Stick", DRIVE);
    let _installed = install(&provider);
    let hold = take("hold-test-gate-left");
    assert!(hold.drive_is_listed(), "precondition: it starts mounted");

    provider.mark_unmounted("/Volumes/Stick");
    assert!(!hold.drive_is_listed(), "a drive that left authorizes nothing");
}

/// "The table wouldn't read" is not permission to delete.
///
/// ⚠️ The opposite disposition to [`flag_vanished`], which never FLAGS on `None`.
/// Both refuse to act on a don't-know; for a stop the safe act is to keep waiting,
/// for a delete it's to keep the row.
#[test]
fn an_unreadable_mount_table_never_authorizes_a_delete() {
    let _serialized = crate::indexing::handle::test_lock();
    let provider = FakeVolumeProvider::shared();
    provider.mount("/Volumes/Stick", DRIVE).mark_table_unreadable();
    let _installed = install(&provider);

    let hold = take("hold-test-gate-unreadable");
    assert!(!hold.drive_is_listed());
}

/// A generation whose start couldn't name a filesystem is never asked, so hostless
/// tools and every test that builds work with `for_test` delete exactly as today.
#[test]
fn a_generation_with_no_identity_reads_as_present() {
    let _serialized = crate::indexing::handle::test_lock();
    let provider = FakeVolumeProvider::shared();
    let _installed = install(&provider);

    let unnamed = VolumeHold::take("hold-test-gate-unnamed", None);
    assert!(
        unnamed.drive_is_listed(),
        "never asked, so never a reason to withhold a delete"
    );
}
