//! A confirmation with one checkbox under it: the native alert, with its
//! suppression button relabelled as the option ("Also forget the saved password").
//!
//! ❗ Why not `tauri-plugin-dialog`'s `ask`, which every other confirmation uses:
//! it has no checkbox, and the choice belongs IN the question, where the person
//! answers both at once. The alert looks and behaves like `ask`'s (a sheet on the
//! window, Esc on Cancel, Return on the confirming button).
//!
//! Off macOS the answer is `Unsupported`, and the frontend asks the plain question
//! and leaves the option at its safe side (`confirm-dialog.ts`): a choice nobody
//! saw is never made for them.

use serde::{Deserialize, Serialize};

/// What to ask. Every string is already translated by the frontend.
#[derive(Debug, Clone, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CheckboxConfirmRequest {
    pub title: String,
    pub message: String,
    pub confirm_label: String,
    pub cancel_label: String,
    pub checkbox_label: String,
    /// Whether the checkbox starts checked.
    pub checked: bool,
}

/// What the person answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CheckboxConfirm {
    /// The confirming button, with the checkbox as it was left.
    Confirmed {
        checked: bool,
    },
    Cancelled,
    /// No native alert here: ask without the checkbox.
    #[cfg_attr(
        target_os = "macos",
        allow(dead_code, reason = "Linux's answer; it's on the wire for every platform")
    )]
    Unsupported,
}

/// Asks `request` in a native alert on the calling window, and answers once the
/// person did.
#[tauri::command]
#[specta::specta]
pub async fn confirm_with_checkbox(window: tauri::Window, request: CheckboxConfirmRequest) -> CheckboxConfirm {
    ask(window, request).await
}

#[cfg(target_os = "macos")]
async fn ask(window: tauri::Window, request: CheckboxConfirmRequest) -> CheckboxConfirm {
    let (tx, rx) = tokio::sync::oneshot::channel();
    // AppKit only on the main thread; the NSWindow pointer isn't `Send`, so it's read in there.
    let on_main = window.clone();
    let scheduled = window.run_on_main_thread(move || {
        let _ = tx.send(macos::run_alert(&on_main, &request));
    });
    if let Err(e) = scheduled {
        log::warn!(target: "ui", "confirm_with_checkbox: couldn't reach the main thread: {e}");
        return CheckboxConfirm::Cancelled;
    }
    rx.await.unwrap_or(CheckboxConfirm::Cancelled)
}

#[cfg(not(target_os = "macos"))]
async fn ask(_window: tauri::Window, _request: CheckboxConfirmRequest) -> CheckboxConfirm {
    CheckboxConfirm::Unsupported
}

/// The alert's answer: the first button confirms, anything else (Cancel, Esc, a
/// sheet ended from outside) cancels.
#[cfg(any(target_os = "macos", test))]
fn answer_of(response: isize, first_button: isize, checked: bool) -> CheckboxConfirm {
    if response == first_button {
        CheckboxConfirm::Confirmed { checked }
    } else {
        CheckboxConfirm::Cancelled
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{CheckboxConfirm, CheckboxConfirmRequest, answer_of};
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication, NSControlStateValueOff, NSControlStateValueOn,
        NSWindow,
    };
    use objc2_foundation::NSString;

    /// Runs the alert as a sheet on `window` and waits for it, the way `ask`'s alert
    /// does: begin the sheet, then run a modal session its completion handler stops.
    pub(super) fn run_alert(window: &tauri::Window, request: &CheckboxConfirmRequest) -> CheckboxConfirm {
        let Some(mtm) = MainThreadMarker::new() else {
            log::warn!(target: "ui", "confirm_with_checkbox: not on the main thread");
            return CheckboxConfirm::Cancelled;
        };
        let alert = NSAlert::new(mtm);
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str(&request.title));
        alert.setInformativeText(&NSString::from_str(&request.message));
        alert.addButtonWithTitle(&NSString::from_str(&request.confirm_label));
        let cancel = alert.addButtonWithTitle(&NSString::from_str(&request.cancel_label));
        // Esc cancels whatever the button is called; AppKit only assigns it to one titled "Cancel".
        cancel.setKeyEquivalent(&NSString::from_str("\u{1b}"));
        alert.setShowsSuppressionButton(true);
        let Some(checkbox) = alert.suppressionButton() else {
            return CheckboxConfirm::Cancelled;
        };
        checkbox.setTitle(&NSString::from_str(&request.checkbox_label));
        checkbox.setState(if request.checked {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });

        let completion = block2::StackBlock::new(move |code| {
            NSApplication::sharedApplication(mtm).stopModalWithCode(code);
        });
        match window.ns_window() {
            Ok(ptr) => {
                // SAFETY: `ns_window` hands back the live NSWindow behind this Tauri window,
                // and we're on the main thread for the whole borrow.
                let parent: &NSWindow = unsafe { &*(ptr as *const NSWindow) };
                alert.beginSheetModalForWindow_completionHandler(parent, Some(&completion));
            }
            Err(e) => log::warn!(target: "ui", "confirm_with_checkbox: no NSWindow, so an app-modal alert: {e}"),
        }
        let response = alert.runModal();
        answer_of(
            response,
            NSAlertFirstButtonReturn,
            checkbox.state() == NSControlStateValueOn,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_button_confirms_with_the_checkbox_as_left_and_anything_else_cancels() {
        assert_eq!(
            answer_of(1000, 1000, true),
            CheckboxConfirm::Confirmed { checked: true }
        );
        assert_eq!(
            answer_of(1000, 1000, false),
            CheckboxConfirm::Confirmed { checked: false }
        );
        assert_eq!(answer_of(1001, 1000, true), CheckboxConfirm::Cancelled);
        assert_eq!(
            answer_of(-1000, 1000, true),
            CheckboxConfirm::Cancelled,
            "a sheet ended from outside"
        );
    }
}
