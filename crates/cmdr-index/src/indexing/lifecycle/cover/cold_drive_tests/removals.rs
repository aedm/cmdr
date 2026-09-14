//! Stopping a removable drive's index for an eject. Whatever window the stop
//! lands in, it answers "released" only once nothing is working on the drive any
//! more: an unmount that meets a live watcher can wedge macOS FSKit.

use std::path::Path;
use std::time::Duration;

use super::toggles::{an_indexed_drive, turn_on};
use super::*;
use crate::indexing::hold::{self, HoldKind, Release};
use crate::indexing::lifecycle::state::{self, RemovableStop};
use crate::indexing::scanner::park::ParkHandle;

/// The shares a drive's live generations still hold, per kind.
fn holders(volume_id: &str) -> Vec<(HoldKind, usize)> {
    match hold::wait_until_released(volume_id, Duration::ZERO) {
        Release::Released => Vec::new(),
        Release::StillHeld(holders) => holders,
    }
}

/// A stop landing while ANOTHER teardown is draining the drive waits for that
/// drain. The drain's manager still holds the watcher until it shuts down, so
/// answering from the published `ShuttingDown` alone let an eject unmount under it.
#[test]
fn a_stop_that_meets_a_drain_in_flight_waits_for_it() {
    let drive = an_indexed_drive("cover-removal-mid-drain-test");
    assert_eq!(
        state::volume_kind(drive.volume_id),
        Some(IndexVolumeKind::LocalExternal),
        "precondition: a drive the removable stop is for"
    );

    state::while_stopping_for_test(drive.volume_id, || {
        assert_eq!(
            state::stop_removable_volume(drive.volume_id, Duration::ZERO),
            RemovableStop::StillReleasing,
            "the drain's manager hasn't shut down yet, so the drive isn't let go"
        );
    });

    // The window closed with the drain run to its end, so now nothing holds it.
    assert!(!state::is_active(drive.volume_id), "the drain retired the instance");
    assert_eq!(
        state::stop_removable_volume(drive.volume_id, Duration::ZERO),
        RemovableStop::NothingToStop,
        "and the manager that drained is gone with it"
    );
}

/// A stop landing while a scan start holds the drive's manager out of the
/// registry is only RECORDED, and the drain runs when the manager comes back. The
/// stop waits for that, rather than reporting a manager it never saw as let go.
#[test]
fn a_stop_that_lands_during_a_scan_start_waits_for_the_handback() {
    let drive = an_indexed_drive("cover-removal-detached-test");

    state::while_detached_for_test(drive.volume_id, || {
        assert_eq!(
            state::stop_removable_volume(drive.volume_id, Duration::ZERO),
            RemovableStop::StillReleasing,
            "the scan start still holds the manager; the stop is only claimed on it"
        );
    });

    // Handing the manager back carried the claimed stop out, drain and all.
    assert!(
        !state::is_active(drive.volume_id),
        "the claimed stop ran at the handback"
    );
    assert_eq!(
        state::stop_removable_volume(drive.volume_id, Duration::ZERO),
        RemovableStop::NothingToStop,
        "and the manager it drained is gone"
    );
}

/// A search's walk reads the drive on a thread of its own, so it holds the drive
/// like any other worker: a stop that has already drained the index still waits
/// for the walk. And the stop reaches the walk, which the search's own token never
/// hears, without stopping the search.
#[test]
fn a_stop_waits_for_a_search_walk_still_reading_the_drive_and_stops_it() {
    let drive = ColdDrive::new("cover-removal-search-walk-test");
    std::fs::create_dir_all(drive.tree.path().join("scope/inner")).expect("dirs");
    let scope = drive.path("scope");
    let park = ParkHandle::arm(Path::new(&scope), 0);
    let search = CancellationToken::new();
    let walk = drive
        .index
        .cover(
            drive.volume_id,
            vec![scope.clone()],
            CoverageDimension::Listing,
            search.clone(),
        )
        .expect("the drive is walkable");
    assert!(
        park.wait_until_parked(Duration::from_secs(10)),
        "the walk parks before it reads {scope}"
    );
    let held = holders(drive.volume_id);
    assert!(
        held.contains(&(HoldKind::SearchCover, 1)) && held.contains(&(HoldKind::WalkerWorker, 1)),
        "the walk's thread and its walker's workers each hold the drive, so a stop can name them: {held:?}"
    );

    // Once the stop reaches the walk, its walker gives up waiting and the walk's own
    // thread can end, but the worker parked before its next read still holds on.
    assert_eq!(
        state::stop_removable_volume(drive.volume_id, Duration::ZERO),
        RemovableStop::StillReleasing,
        "the index is drained, but the search walk still reads the drive"
    );
    cmdr_fs::testing::wait_until(
        Duration::from_secs(5),
        "the drive's stop to reach the search walk",
        || walk.is_stopped_for_test(),
    );
    assert!(!search.is_cancelled(), "without stopping the search");

    park.release();
    let (_, outcome) = drain(walk);
    assert!(outcome.cancelled, "the walk ended because the drive stopped");
    assert_eq!(
        hold::wait_until_released(drive.volume_id, Duration::from_secs(5)),
        Release::Released,
        "and let go of the drive once its last read returned"
    );
}

/// A drive's first index runs on the phase machine's thread and the walk threads
/// it starts, and the drain joins neither. Both hold the drive until they end, and
/// the drain's stop is what ends them.
#[test]
fn a_stop_waits_for_the_phase_machine_and_the_walk_it_runs() {
    let drive = ColdDrive::new("cover-removal-phases-test");
    std::fs::create_dir_all(drive.tree.path().join("scope/inner")).expect("dirs");
    std::fs::write(drive.tree.path().join("scope/found.txt"), "x").expect("file");
    // The machine's first walk takes the volume root or the folder under it,
    // whichever its frontier names.
    let parks = [
        ParkHandle::arm(drive.tree.path(), 0),
        ParkHandle::arm(&drive.tree.path().join("scope"), 0),
    ];
    turn_on(&drive);
    cmdr_fs::testing::wait_until(Duration::from_secs(10), "a phase walk to park", || {
        parks.iter().any(|park| park.wait_until_parked(Duration::ZERO))
    });

    let held = holders(drive.volume_id);
    assert!(
        [HoldKind::Phases, HoldKind::PhaseCover, HoldKind::WalkerWorker]
            .iter()
            .all(|kind| held.contains(&(*kind, 1))),
        "the machine, its walk, and the walk's workers each hold the drive: {held:?}"
    );

    // The stop ends the machine and the walk's thread, and the worker parked before
    // its next read still holds on.
    assert_eq!(
        state::stop_removable_volume(drive.volume_id, Duration::ZERO),
        RemovableStop::StillReleasing,
        "the index is drained, but the machine's walk still reads the drive"
    );

    drop(parks);
    assert_eq!(
        hold::wait_until_released(drive.volume_id, Duration::from_secs(10)),
        Release::Released,
        "the stop ended the walk and the machine, and each let go as it ended"
    );
}
