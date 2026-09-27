use super::*;
use std::time::{Duration, Instant};

fn t0() -> Instant {
    Instant::now()
}

#[test]
fn the_first_holder_starts_the_browse_and_later_ones_share_it() {
    let mut gate = BrowseGate::default();
    assert_eq!(gate.acquire(), GateStep::Start);
    assert_eq!(
        gate.acquire(),
        GateStep::Nothing,
        "a second holder rides the running browse"
    );
    assert!(gate.is_browsing());
}

#[test]
fn the_browse_stops_only_after_the_last_holder_leaves_and_the_linger_runs_out() {
    let mut gate = BrowseGate::default();
    let now = t0();
    gate.acquire();
    gate.acquire();

    assert_eq!(gate.release(now), GateStep::Nothing, "one holder is still there");
    assert_eq!(gate.release(now), GateStep::StopAt(now + LINGER));
    assert_eq!(
        gate.deadline_passed(now + LINGER - Duration::from_millis(1)),
        GateStep::Nothing,
        "the browse lingers until its deadline"
    );
    assert!(gate.is_browsing());

    assert_eq!(gate.deadline_passed(now + LINGER), GateStep::Stop);
    assert!(!gate.is_browsing(), "nothing needs discovery, so nothing browses");
}

#[test]
fn a_holder_arriving_during_the_linger_keeps_the_browse_running() {
    let mut gate = BrowseGate::default();
    let now = t0();
    gate.acquire();
    assert_eq!(gate.release(now), GateStep::StopAt(now + LINGER));

    assert_eq!(
        gate.acquire(),
        GateStep::Nothing,
        "the browse is still up, so no restart"
    );
    assert_eq!(
        gate.deadline_passed(now + LINGER),
        GateStep::Nothing,
        "the stale deadline is void"
    );
    assert!(gate.is_browsing());
}

#[test]
fn only_the_latest_deadline_stops_the_browse() {
    let mut gate = BrowseGate::default();
    let now = t0();
    gate.acquire();
    gate.release(now);
    gate.acquire();
    let later = now + Duration::from_secs(3);
    assert_eq!(gate.release(later), GateStep::StopAt(later + LINGER));

    assert_eq!(
        gate.deadline_passed(now + LINGER),
        GateStep::Nothing,
        "the first timer is stale"
    );
    assert_eq!(gate.deadline_passed(later + LINGER), GateStep::Stop);
}

#[test]
fn a_disabled_network_stops_the_browse_and_holds_off_new_holders() {
    let mut gate = BrowseGate::default();
    gate.acquire();

    assert_eq!(gate.set_enabled(false), GateStep::Stop);
    assert_eq!(gate.acquire(), GateStep::Nothing, "no browse while networking is off");
    assert!(!gate.is_browsing());

    assert_eq!(
        gate.set_enabled(true),
        GateStep::Start,
        "turning networking back on resumes for the holders still there"
    );
}

#[test]
fn enabling_with_nobody_holding_starts_nothing() {
    let mut gate = BrowseGate::default();
    gate.set_enabled(false);
    assert_eq!(gate.set_enabled(true), GateStep::Nothing);
    assert!(!gate.is_browsing());
}

#[test]
fn a_failed_start_is_retried_by_the_next_holder() {
    let mut gate = BrowseGate::default();
    assert_eq!(gate.acquire(), GateStep::Start);
    gate.start_failed();
    assert!(!gate.is_browsing());
    assert_eq!(gate.acquire(), GateStep::Start);
}

#[test]
fn a_release_after_a_failed_start_schedules_no_stop() {
    let mut gate = BrowseGate::default();
    gate.acquire();
    gate.start_failed();
    assert_eq!(gate.release(t0()), GateStep::Nothing, "there's no browse to stop");
}

#[test]
fn shutting_down_stops_a_running_browse_even_with_holders() {
    let mut gate = BrowseGate::default();
    gate.acquire();
    assert_eq!(gate.shut_down(), GateStep::Stop);
    assert_eq!(gate.shut_down(), GateStep::Nothing, "idempotent");
    assert_eq!(gate.acquire(), GateStep::Nothing, "nothing restarts on the way out");
}
