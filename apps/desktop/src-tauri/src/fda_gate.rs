//! Full Disk Access gate.
//!
//! At first launch on macOS, Cmdr shows an in-app modal that walks the user
//! through granting Full Disk Access (FDA) before the indexer scans `/`.
//! The same gate applies more broadly: any launch-time code that touches
//! TCC-protected paths (Downloads, Documents, Desktop, ...) or NSWorkspace
//! icon/LaunchServices APIs on those paths must skip work while the FDA
//! decision is pending. Otherwise macOS stacks several native permission
//! popups (MediaLibrary, AppData, Desktop, Documents, Downloads, ...) on
//! top of our in-app modal. That's exactly the onboarding-flood UX we want to
//! avoid.
//!
//! **A closed gate means exactly one thing: the FDA question is on screen.** The frontend holds
//! up its end (`routes/(main)/startup-gates.ts`): every launch that reaches the explorer without
//! the wizard opens the gate itself via `start_indexing_after_fda_decision`, so a gate that stays
//! shut can never leave launch-time work deferred with nothing on screen to explain it.
//!
//! The gate has two pieces:
//!
//! 1. `is_fda_pending(fda_choice, os_fda_granted)`: pure decision used at startup and by tests.
//!    Pending whenever the OS reports FDA isn't granted and the user hasn't answered `Deny`.
//! 2. A process-global `AtomicBool` set once at startup (and cleared when the user denies FDA
//!    in-session). Read by code that runs after startup via `is_fda_pending_runtime()`.
//!
//! On non-macOS platforms FDA doesn't exist; the runtime gate is always
//! `false` (open) so cross-platform callers get the right behaviour without
//! cfg-guards at every site.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::settings::FullDiskAccessChoice;

static FDA_PENDING: OnceLock<AtomicBool> = OnceLock::new();

/// Pure decision: is the FDA decision still pending at this moment?
///
/// Pending whenever the OS says FDA isn't granted and the user hasn't actively
/// declined. That's deliberately conservative, and it has to be: the frontend
/// parks people on step 1 for a revoked grant and for "clicked Allow, never
/// finished in System Settings" just as much as for a fresh install, and a
/// recorded `Allow` says nothing about whether the grant exists right now.
///
/// The two open cases:
///
/// - `os_fda_granted` is true: the per-folder TCC services are subsumed by FDA,
///   so protected paths are safe even with no in-app choice recorded.
/// - `Deny`: the per-folder prompts are exactly what the user signed up for, and
///   the wizard is behind them either way.
pub fn is_fda_pending(fda_choice: FullDiskAccessChoice, os_fda_granted: bool) -> bool {
    !os_fda_granted && fda_choice != FullDiskAccessChoice::Deny
}

/// Set the runtime gate. Call once at startup with the result of
/// `is_fda_pending(...)`, and again with `false` after the user makes a
/// choice in-session (deny path; the allow path requires a restart and
/// re-enters startup).
pub fn set_fda_pending(pending: bool) {
    FDA_PENDING
        .get_or_init(|| AtomicBool::new(pending))
        .store(pending, Ordering::Release);
}

/// Read the runtime gate. Returns `false` until `set_fda_pending` has been
/// called. Safe default for tests and any non-macOS build that never sets
/// it.
pub fn is_fda_pending_runtime() -> bool {
    FDA_PENDING.get().is_some_and(|f| f.load(Ordering::Acquire))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All six (choice × grant) rows, because each one decides whether a launch
    /// stacks native TCC popups over step 1 or defers work nobody will resume.
    #[test]
    fn pending_whenever_the_os_denies_and_the_user_has_not() {
        // Fresh install: the question is on screen, nothing may touch a protected path.
        assert!(is_fda_pending(FullDiskAccessChoice::Unanswered, false));
        // Granted before we ever asked (a reinstall, or a grant made by hand). FDA subsumes
        // the per-folder TCC services, so there's nothing left to pop up.
        assert!(!is_fda_pending(FullDiskAccessChoice::Unanswered, true));
        // Revoked-after-allow and first-time-stuck: the wizard parks both on step 1, so the
        // gate has to hold even though the recorded choice is `Allow`.
        assert!(is_fda_pending(FullDiskAccessChoice::Allow, false));
        // The settled happy path.
        assert!(!is_fda_pending(FullDiskAccessChoice::Allow, true));
        // Declined: the per-folder prompts are what the user signed up for, so work runs.
        assert!(!is_fda_pending(FullDiskAccessChoice::Deny, false));
        assert!(!is_fda_pending(FullDiskAccessChoice::Deny, true));
    }
}
