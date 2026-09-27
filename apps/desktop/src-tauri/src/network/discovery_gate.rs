//! Runs the mDNS browse only while something needs it.
//!
//! Every consumer of live discovery holds a [`DiscoveryLease`] for as long as it
//! needs one: the Servers view while it's on screen, an upgrade while it resolves
//! a mount's server, the short warm-up at launch. The first lease starts the
//! browse; after the last one drops, the browse lingers for [`LINGER`] and then
//! stops, taking its thread and its multicast wakeups with it. The host cache
//! outlives the browse (`discovery_cache.rs`), marked stale.
//!
//! [`BrowseGate`] is the pure state machine; the statics below drive it and do what
//! it says through `mdns_discovery`.

use crate::ignore_poison::IgnorePoison;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long the browse outlives its last lease. Covers the pane that leaves the
/// Servers view and comes straight back, and back-to-back upgrades, without a
/// restart that would have to re-resolve every host before it's evidence again.
pub(crate) const LINGER: Duration = Duration::from_secs(10);

/// How long the launch warm-up browses: enough to fill the host cache, so the
/// Servers view opens on known servers.
pub const LAUNCH_WARM_UP: Duration = Duration::from_secs(10);

/// What the driver has to do after a gate transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GateStep {
    Start,
    /// The last holder left: stop at this deadline unless someone comes back first.
    StopAt(Instant),
    Stop,
    Nothing,
}

/// Who needs the browse, and whether it runs.
#[derive(Debug)]
pub(crate) struct BrowseGate {
    holders: usize,
    browsing: bool,
    /// `network.enabled`: holders still count while it's off, so turning it back
    /// on resumes for whoever is still waiting.
    enabled: bool,
    /// Set once the app is on its way out; nothing starts after it.
    shut: bool,
    stop_at: Option<Instant>,
}

impl Default for BrowseGate {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowseGate {
    const fn new() -> Self {
        Self {
            holders: 0,
            browsing: false,
            enabled: true,
            shut: false,
            stop_at: None,
        }
    }

    pub(crate) fn acquire(&mut self) -> GateStep {
        self.holders += 1;
        self.stop_at = None;
        self.start_if_wanted()
    }

    pub(crate) fn release(&mut self, now: Instant) -> GateStep {
        self.holders = self.holders.saturating_sub(1);
        if self.holders > 0 || !self.browsing {
            return GateStep::Nothing;
        }
        let at = now + LINGER;
        self.stop_at = Some(at);
        GateStep::StopAt(at)
    }

    /// A linger timer fired. Only the latest deadline counts: a holder that came and
    /// went since has pushed it later, and one still here has cleared it.
    pub(crate) fn deadline_passed(&mut self, now: Instant) -> GateStep {
        match self.stop_at {
            Some(at) if now >= at && self.holders == 0 && self.browsing => {
                self.stop_at = None;
                self.browsing = false;
                GateStep::Stop
            }
            _ => GateStep::Nothing,
        }
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool) -> GateStep {
        self.enabled = enabled;
        if enabled {
            return self.start_if_wanted();
        }
        self.stop_now()
    }

    /// The daemon couldn't start: the next holder tries again.
    pub(crate) fn start_failed(&mut self) {
        self.browsing = false;
    }

    pub(crate) fn shut_down(&mut self) -> GateStep {
        self.shut = true;
        self.stop_now()
    }

    #[cfg(test)]
    pub(crate) fn is_browsing(&self) -> bool {
        self.browsing
    }

    fn start_if_wanted(&mut self) -> GateStep {
        if self.browsing || self.holders == 0 || !self.enabled || self.shut {
            return GateStep::Nothing;
        }
        self.browsing = true;
        GateStep::Start
    }

    fn stop_now(&mut self) -> GateStep {
        self.stop_at = None;
        if !self.browsing {
            return GateStep::Nothing;
        }
        self.browsing = false;
        GateStep::Stop
    }
}

static GATE: Mutex<BrowseGate> = Mutex::new(BrowseGate::new());

/// Keeps the mDNS browse running while it lives. Drop it when the need ends.
#[must_use = "the browse stops (after the linger) as soon as the lease drops"]
pub struct DiscoveryLease {
    _private: (),
}

impl Drop for DiscoveryLease {
    fn drop(&mut self) {
        transition(|gate| gate.release(Instant::now()));
    }
}

/// Holds the browse until the returned lease drops.
pub fn hold() -> DiscoveryLease {
    transition(BrowseGate::acquire);
    DiscoveryLease { _private: () }
}

/// Holds the browse for `duration`, for a need with no scope to tie a lease to.
pub fn hold_for(duration: Duration) {
    let lease = hold();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(duration).await;
        drop(lease);
    });
}

/// Mirrors `network.enabled`: off stops the browse whoever holds it, on resumes it
/// for the holders still there.
pub(crate) fn set_enabled(enabled: bool) {
    transition(|gate| gate.set_enabled(enabled));
}

/// Stops the browse for good, on the way out of the app.
pub(crate) fn shut_down() {
    transition(BrowseGate::shut_down);
}

/// Runs one gate transition and carries out its step under the same lock, so a
/// start and a stop can never land in the opposite order they were decided in.
fn transition(step_of: impl FnOnce(&mut BrowseGate) -> GateStep) {
    let mut gate = GATE.lock_ignore_poison();
    let step = step_of(&mut gate);
    carry_out(step, &mut gate);
}

fn carry_out(step: GateStep, gate: &mut BrowseGate) {
    match step {
        GateStep::Start => {
            if !super::mdns_discovery::start_browse() {
                gate.start_failed();
            }
        }
        GateStep::Stop => super::mdns_discovery::stop_browse(),
        GateStep::StopAt(at) => {
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep_until(at.into()).await;
                linger_ran_out();
            });
        }
        GateStep::Nothing => {}
    }
}

fn linger_ran_out() {
    transition(|gate| gate.deadline_passed(Instant::now()));
}

#[cfg(test)]
#[path = "discovery_gate_test.rs"]
mod tests;
