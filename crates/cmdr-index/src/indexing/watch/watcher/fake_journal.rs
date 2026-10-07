//! Tests that drive a whole volume through the handle, and never wait on a
//! delivery, put its root on a fake journal: no stream, and synthetic event IDs.
//!
//! ⚠️ **Why it exists**: every real stream start and every
//! [`current_event_id`](super::current_event_id) is a round trip to `fseventsd`,
//! ONE daemon for the whole machine. With other processes' disk churn pegging it,
//! each call took 0.7–3.7 s, so the phase-machine tests (36 processes, a few calls
//! each) blew the 8 s cap at full parallelism while each took 0.13 s alone
//! (verified on macOS 27, `sample` + timing logs, 2026-10-07, issue #374). No lock
//! or group in the test runner helps: the contention is with every process on the
//! machine.
//!
//! ❌ Not for a test that asserts on what a watcher DELIVERS: that one needs the
//! real stream (`.config/nextest.toml`'s `real-notify` group).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cmdr_fs::ignore_poison::IgnorePoison;
use tokio::sync::mpsc;

use super::{DriveWatcher, FsChangeEvent};

/// Scoped by root, so a real-stream test sharing the process under plain
/// `cargo test` keeps its real journal.
static ROOTS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Far from zero, which reads as "no stored ID" to every caller.
static NEXT_EVENT_ID: AtomicU64 = AtomicU64::new(1_000_000);

/// Fake the journal for `root` and everything under it until the guard drops.
pub(crate) fn fake_for(root: &Path) -> Guard {
    ROOTS.lock_ignore_poison().push(root.to_path_buf());
    Guard(root.to_path_buf())
}

pub(super) fn covers(path: &Path) -> bool {
    ROOTS.lock_ignore_poison().iter().any(|root| path.starts_with(root))
}

pub(super) fn next_event_id() -> u64 {
    NEXT_EVENT_ID.fetch_add(1, Ordering::Relaxed)
}

/// A running watcher with no stream behind it. Its task holds the sender until
/// `stop`, so the loop reading the other end waits the way it waits on a quiet
/// drive, ❌ never reads a closed channel as a watcher that died.
pub(super) fn watcher(event_sender: mpsc::UnboundedSender<FsChangeEvent>) -> DriveWatcher {
    let forward_task = crate::indexing::host::runtime::spawn(async move {
        let _held = event_sender;
        std::future::pending::<()>().await;
    });
    DriveWatcher {
        running: Arc::new(AtomicBool::new(true)),
        last_event_id: Arc::new(AtomicU64::new(0)),
        overflow: Arc::new(AtomicBool::new(false)),
        handler: None,
        forward_task: Some(forward_task),
    }
}

pub(crate) struct Guard(PathBuf);

impl Drop for Guard {
    fn drop(&mut self) {
        let mut roots = ROOTS.lock_ignore_poison();
        if let Some(index) = roots.iter().position(|root| *root == self.0) {
            roots.remove(index);
        }
    }
}
