//! Whether anyone can see the main window, as its webview reports it.
//!
//! The frontend pushes `document.visibilityState` through [`set_main_window_visible`] on load and on
//! every `visibilitychange`. That's WebKit's own answer, so it covers every way a window stops being
//! seen: minimized, the app hidden, another Space, or fully covered by other windows (macOS window
//! occlusion). The backend's idle work that exists only to redraw the main window (the disk-space
//! stream, listing size refreshes) holds while it's hidden and catches up on the edge back.
//!
//! Starts as visible, so a frontend that never reports leaves everything running as if it were.

use std::sync::LazyLock;

use tokio::sync::watch;

static VISIBLE: LazyLock<watch::Sender<bool>> = LazyLock::new(|| watch::Sender::new(true));

/// Whether the main window is visible right now.
pub(crate) fn is_visible() -> bool {
    *VISIBLE.borrow()
}

/// A receiver that wakes on every change, for work that catches up the moment the window shows.
pub(crate) fn subscribe() -> watch::Receiver<bool> {
    VISIBLE.subscribe()
}

/// Records the main window's visibility. A repeat of the current value wakes nobody.
pub(crate) fn set_visible(visible: bool) {
    VISIBLE.send_if_modified(|current| {
        if *current == visible {
            return false;
        }
        *current = visible;
        true
    });
}

/// The main window's webview reports whether it's visible (`document.visibilityState`).
///
/// ❗ An E2E run always counts as visible: its windows open ordered to the back, where WebKit calls
/// them hidden, and the suite's contract is that ordering changes nothing a test observes
/// (`test/e2e-playwright/DETAILS.md`).
#[tauri::command]
#[specta::specta]
pub fn set_main_window_visible(visible: bool) {
    set_visible(visible || crate::test_mode::is_e2e_mode());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_visible_and_wakes_subscribers_only_on_a_real_change() {
        assert!(is_visible(), "a frontend that never reports leaves everything running");
        let mut rx = subscribe();

        set_visible(true);
        assert!(!rx.has_changed().expect("sender is static"), "a repeat wakes nobody");

        set_visible(false);
        assert!(rx.has_changed().expect("sender is static"));
        assert!(!*rx.borrow_and_update());
        assert!(!is_visible());

        set_visible(true);
        assert!(*rx.borrow_and_update());
    }
}
