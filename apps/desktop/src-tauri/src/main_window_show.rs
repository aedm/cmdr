//! The backend's guarantee that the main window appears.
//!
//! The main window is created `"visible": false` (`tauri.conf.json`), and the frontend shows it
//! through the `show_main_window` command: from `onMount` on a normal launch
//! (`routes/(main)/show-main-on-mount.ts`), or from the boot guard in `src/app.html` on a WebKit
//! below the floor. Both depend on the webview running something. When it can't (a bundle that
//! doesn't parse, a startup `await` that throws or never settles), nothing else would show the
//! window, and the app runs invisibly: a menu bar, a Dock icon, and a full index walk in the
//! background. A Catalina user's log recorded exactly that across 11 launches of v0.43.0–v0.45.0.
//!
//! So setup arms a fallback: if nothing has shown the window [`FALLBACK_DELAY`] later, the backend
//! shows it. A blank or half-loaded window is a state the user can see, report, and quit; an
//! invisible app isn't.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::{Runtime, WebviewWindow};

/// How long the frontend gets before the backend shows the window itself.
///
/// The frontend starts logging 0.5–2.1 s after the backend's logger comes up, and shows the window
/// shortly after (first `FE:` line per launch in a production log, 12 launches on an Apple Silicon
/// Mac, 2026-09-09 to 2026-09-12). Ten seconds leaves a slow cold start on an old Intel Mac plenty
/// of room before the fallback could fire early and show the loading spinner.
const FALLBACK_DELAY: Duration = Duration::from_secs(10);

/// Whether `show_main_window` has run in this process.
static SHOWN: AtomicBool = AtomicBool::new(false);

/// Records that the frontend showed the main window, which disarms the fallback.
pub fn mark_shown() {
    SHOWN.store(true, Ordering::Release);
}

/// Arms the fallback for the main window. Call once, from setup, after placing the window.
pub fn arm_fallback<R: Runtime>(window: WebviewWindow<R>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FALLBACK_DELAY).await;
        if !fallback_needed(SHOWN.load(Ordering::Acquire), crate::test_mode::is_e2e_mode()) {
            return;
        }
        log::warn!(
            target: "ui",
            "Nothing showed the main window within {} s of launch, so the frontend likely never mounted; showing it from the backend",
            FALLBACK_DELAY.as_secs()
        );
        // A bare `show()`, without the `set_focus` a launch show adds: ten seconds in, the user may
        // have moved on to another app, and pulling the front back to a broken window is worse.
        if let Err(e) = window.show() {
            log::warn!(target: "ui", "Couldn't show the main window from the backend: {e}");
        }
    });
}

/// The fallback's decision, apart from its timer.
///
/// An E2E run never gets it: `show_main_window` orders that run's windows to the BACK so a test
/// run doesn't take the developer's focus, and a bare `show()` would undo that. A frontend that
/// never mounts fails the run loudly anyway.
fn fallback_needed(shown: bool, e2e_run: bool) -> bool {
    !shown && !e2e_run
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_only_when_nothing_showed_the_window() {
        assert!(fallback_needed(false, false));
        assert!(!fallback_needed(true, false));
    }

    #[test]
    fn never_fires_on_an_automated_run() {
        assert!(!fallback_needed(false, true));
        assert!(!fallback_needed(true, true));
    }
}
