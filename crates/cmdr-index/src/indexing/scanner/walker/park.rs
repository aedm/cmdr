//! A test-only park point for the guarded walker (`#[cfg(test)]`, so it exists in no
//! release build and in no build of the app).
//!
//! A test arms it for a walk ROOT. Once that walk has read `after_dirs` directories,
//! every worker that reaches the park point stops there until the test releases it.
//! The point sits in `Engine::run_worker` after a task is popped and before its
//! directory is opened, and each worker counts itself busy from that point until the
//! task is fully handled (its read, then the visitor's per-child work). So "parked"
//! means no worker holds a directory handle on the walked volume or touches it at
//! all: a volume detached while the walk is parked has vanished BETWEEN reads, never
//! under an open file descriptor.
//!
//! Parked workers aren't in flight, so the watchdog never abandons them; a walk that's
//! cancelled while parked stays parked until the handle releases or drops.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use cmdr_fs::ignore_poison::IgnorePoison;

/// Parks armed right now, one per walk root a test is holding.
static ARMED: Mutex<Vec<Arc<Park>>> = Mutex::new(Vec::new());

/// A test's hold on a walk: armed by [`ParkHandle::arm`], released by
/// [`ParkHandle::release`] or on drop.
pub(crate) struct ParkHandle {
    park: Arc<Park>,
}

impl ParkHandle {
    /// Parks the next walk of `root` once it has read `after_dirs` directories. Only a
    /// walk whose root is exactly `root` sees it.
    pub(crate) fn arm(root: &Path, after_dirs: u64) -> Self {
        let park = Arc::new(Park {
            root: root.to_path_buf(),
            after_dirs,
            state: Mutex::new(State::default()),
            cv: Condvar::new(),
        });
        ARMED.lock_ignore_poison().push(Arc::clone(&park));
        Self { park }
    }

    /// Waits until the walk has tripped the park and every worker has finished the
    /// task it held, so nothing reads the volume. `false` when that didn't happen
    /// within `timeout` (the walk ended first, or never started).
    pub(crate) fn wait_until_parked(&self, timeout: Duration) -> bool {
        let state = self.park.state.lock_ignore_poison();
        let (state, _) = self
            .park
            .cv
            .wait_timeout_while(state, timeout, |state| !state.is_parked())
            .unwrap_or_else(|e| e.into_inner());
        state.is_parked()
    }

    /// Lets the parked workers go on. The park never trips again for this handle.
    pub(crate) fn release(&self) {
        self.park.state.lock_ignore_poison().released = true;
        self.park.cv.notify_all();
    }
}

impl Drop for ParkHandle {
    fn drop(&mut self) {
        self.release();
        ARMED.lock_ignore_poison().retain(|park| !Arc::ptr_eq(park, &self.park));
    }
}

/// The park armed for a walk of `root`, if any.
pub(super) fn armed_for(root: &Path) -> Option<Arc<Park>> {
    ARMED
        .lock_ignore_poison()
        .iter()
        .find(|park| park.root == root)
        .cloned()
}

/// One armed park, shared by the handle and the walk's engine.
pub(super) struct Park {
    root: PathBuf,
    after_dirs: u64,
    state: Mutex<State>,
    /// Wakes parked workers on release, and the test's wait on every trip and every
    /// task a worker finishes.
    cv: Condvar,
}

#[derive(Default)]
struct State {
    /// The walk read `after_dirs` directories; workers stop at the park point.
    tripped: bool,
    /// The test let them go.
    released: bool,
    /// Workers past the park point and not yet done with their task.
    busy: usize,
}

impl State {
    fn is_parked(&self) -> bool {
        self.tripped && !self.released && self.busy == 0
    }
}

impl Park {
    /// The park point a worker passes before opening its next directory. Trips the
    /// park once the walk has read `after_dirs` directories, waits while it's tripped
    /// and unreleased, then counts the worker busy until the returned guard drops.
    pub(super) fn enter(self: &Arc<Self>, dirs_read: u64) -> Busy {
        let mut state = self.state.lock_ignore_poison();
        if dirs_read >= self.after_dirs && !state.released && !state.tripped {
            state.tripped = true;
            self.cv.notify_all();
        }
        while state.tripped && !state.released {
            state = self.cv.wait(state).unwrap_or_else(|e| e.into_inner());
        }
        state.busy += 1;
        Busy { park: Arc::clone(self) }
    }
}

/// A worker's busy count on a [`Park`], held from the park point until its task is
/// handled.
pub(super) struct Busy {
    park: Arc<Park>,
}

impl Drop for Busy {
    fn drop(&mut self) {
        let mut state = self.park.state.lock_ignore_poison();
        state.busy = state.busy.saturating_sub(1);
        drop(state);
        self.park.cv.notify_all();
    }
}
