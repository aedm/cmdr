//! One broadcast round, and the local-volume snapshot rounds share.
//!
//! Split from `volume_broadcast.rs` so the ordering rules are testable with a fake
//! discovery and a fake sink: no `AppHandle`, no hung mount, virtual time.

use super::{LIST_TIMEOUT, PROVISIONAL_AFTER, VolumesChanged};
use crate::ignore_poison::IgnorePoison;
use crate::volume_listing::{ListingOutcome, LocationInfo};
use std::future::Future;
use std::sync::Mutex;

/// What the broadcast knows about the LOCAL half of the list (mounts, favorites,
/// cloud drives): the part discovery produces and a hung mount can hold up.
///
/// **Why a timeout must not publish an empty list.** `timed_out: true` means "this list
/// may be missing volumes", and the frontend voices exactly that. Pairing it with an
/// empty list said "you have no volumes" instead: the picker went blank, and its
/// refresh button re-ran the same listing into the same timeout, so nothing the user
/// could do brought the volumes back. A transient 2 s stall on one hung mount left the
/// app looking like it had lost every drive, permanently.
///
/// A stale entry is the right trade against a blank picker: it's flagged stale, an
/// unmount arrives on its own `volume-unmounted` event regardless, and picking a volume
/// that has since gone reports a normal missing-path error. ❌ Don't "simplify" this
/// back to emitting `vec![]` on timeout.
pub(super) struct LocalSnapshot {
    /// The most recent SUCCESSFUL listing.
    volumes: Vec<LocationInfo>,
    /// Whether the newest finished discovery came up short (timed out).
    timed_out: bool,
    /// The start number of the discovery `volumes` and `timed_out` come from.
    applied: u64,
    /// How many discoveries have started, which numbers them.
    started: u64,
    /// How many discoveries are still running.
    in_flight: usize,
}

impl LocalSnapshot {
    pub(super) const fn new() -> Self {
        Self {
            volumes: Vec::new(),
            timed_out: false,
            applied: 0,
            started: 0,
            in_flight: 0,
        }
    }

    /// Numbers a new discovery and counts it as running.
    fn begin(&mut self) -> u64 {
        self.started += 1;
        self.in_flight += 1;
        self.started
    }

    /// Folds discovery `round`'s outcome in and counts it as done.
    ///
    /// ❗ A discovery that STARTED before the one already applied says nothing newer,
    /// so it can't roll the snapshot back: two rounds overlap whenever a mount hangs,
    /// and the older one can finish last.
    ///
    /// A panic reports `timed_out: false`: the frontend's flag drives a retry affordance
    /// for a slow listing, and a panicked one isn't slow. The last good set still
    /// carries, for the same reason it does on a timeout.
    pub(super) fn finish(&mut self, round: u64, outcome: ListingOutcome) {
        self.in_flight = self.in_flight.saturating_sub(1);
        if round <= self.applied {
            return;
        }
        self.applied = round;
        match outcome {
            ListingOutcome::Listed(volumes) => {
                self.volumes = volumes;
                self.timed_out = false;
            }
            ListingOutcome::TimedOut => self.timed_out = true,
            ListingOutcome::Panicked => self.timed_out = false,
        }
    }

    /// The local part to publish now: `(volumes, timed_out, discovery_pending)`.
    pub(super) fn local_part(&self) -> (Vec<LocationInfo>, bool, bool) {
        (self.volumes.clone(), self.timed_out, self.in_flight > 0)
    }

    /// Starts a discovery for tests that drive `finish` by hand.
    #[cfg(test)]
    pub(super) fn begin_for_test(&mut self) -> u64 {
        self.begin()
    }
}

/// A running discovery's slot in the snapshot. Dropping it unfinished (a panic in
/// `complete`, a cancelled task) still counts the discovery as done, so a lost round
/// can't leave every later event saying `discovery_pending` forever.
struct RoundGuard<'a> {
    snapshot: &'a Mutex<LocalSnapshot>,
    round: u64,
    finished: bool,
}

impl RoundGuard<'_> {
    fn finish(mut self, outcome: ListingOutcome) {
        self.finished = true;
        self.snapshot.lock_ignore_poison().finish(self.round, outcome);
    }
}

impl Drop for RoundGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.snapshot
                .lock_ignore_poison()
                .finish(self.round, ListingOutcome::Panicked);
        }
    }
}

/// The snapshot, plus the lock that keeps events in the order the snapshot moved.
pub(super) struct Broadcaster {
    local: Mutex<LocalSnapshot>,
    /// Held from reading the snapshot until the event is out, so an event composed
    /// from an older snapshot can never land after one composed from a newer one.
    publish: tokio::sync::Mutex<()>,
}

impl Broadcaster {
    pub(super) const fn new() -> Self {
        Self {
            local: Mutex::new(LocalSnapshot::new()),
            publish: tokio::sync::Mutex::const_new(()),
        }
    }

    /// One round: runs `discovery` and emits the finished list through `emit`.
    ///
    /// ❗ **Server rows never wait on local discovery.** Discovery can hang for its
    /// full 2 s on one dead mount, and the server rows ride the same event, so a
    /// re-added server kept its OLD folder in the switcher for those 2 s (same id,
    /// host + port + user, so the stale row stayed clickable and opened to "Path not
    /// found"). Past [`PROVISIONAL_AFTER`] the round emits the cached local part with
    /// `discovery_pending: true`, then emits again when discovery lands.
    ///
    /// `complete` turns a local part into the published list (`volume_listing::complete`
    /// in production: devices, servers, registry enrichment), and it runs at EMIT time,
    /// so every event carries the server rows as they are when it goes out.
    pub(super) async fn round<D, C, CF, E>(&self, discovery: D, complete: C, mut emit: E)
    where
        D: Future<Output = ListingOutcome>,
        C: Fn(Vec<LocationInfo>) -> CF,
        CF: Future<Output = Vec<LocationInfo>>,
        E: FnMut(VolumesChanged),
    {
        let guard = RoundGuard {
            snapshot: &self.local,
            round: self.local.lock_ignore_poison().begin(),
            finished: false,
        };
        let mut discovery = std::pin::pin!(discovery);
        let outcome = match tokio::time::timeout(PROVISIONAL_AFTER, &mut discovery).await {
            Ok(outcome) => outcome,
            Err(_) => {
                {
                    let _order = self.publish.lock().await;
                    self.compose_and_emit(&complete, &mut emit).await;
                }
                discovery.await
            }
        };

        let _order = self.publish.lock().await;
        guard.finish(outcome);
        self.compose_and_emit(&complete, &mut emit).await;
    }

    /// Emits the snapshot's local part, completed. Call with `publish` held.
    async fn compose_and_emit<C, CF, E>(&self, complete: &C, emit: &mut E)
    where
        C: Fn(Vec<LocationInfo>) -> CF,
        CF: Future<Output = Vec<LocationInfo>>,
        E: FnMut(VolumesChanged),
    {
        let (local, timed_out, discovery_pending) = self.local.lock_ignore_poison().local_part();
        let data = complete(local).await;
        emit(VolumesChanged {
            data,
            timed_out,
            discovery_pending,
        });
    }
}

// Keeps the constants' relationship honest: a provisional event only helps if it
// can go out before discovery gives up.
const _: () = assert!(PROVISIONAL_AFTER.as_millis() < LIST_TIMEOUT.as_millis());
