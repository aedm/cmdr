//! Crash reporter Tauri commands.
//!
//! Thin wrappers for crash file detection, dismissal, and sending.

use crate::crash_reporter::{self, CrashReport};
use crate::server_request::ServerRequestError;

/// Checks for a pending crash report from a previous session.
/// Returns the report, or `null` if none exists.
#[tauri::command]
#[specta::specta]
pub fn check_pending_crash_report(app: tauri::AppHandle) -> Option<CrashReport> {
    crash_reporter::take_pending_crash_report(&app)
}

/// Deletes the crash report file without sending it.
#[tauri::command]
#[specta::specta]
pub fn dismiss_crash_report(app: tauri::AppHandle) {
    crash_reporter::dismiss_pending_crash_report(&app);
}

/// Sends the crash report to the ingestion server, then deletes the local file.
/// Skipped in debug builds, E2E builds (`playwright-e2e`), and CI to avoid polluting production
/// data. The E2E skip mirrors `error_reporter::upload`: an E2E build is a release build, so without
/// it a crash during a test run would reach the live channel looking like a real user's.
///
/// A failed send keeps the file, so the report is offered again next launch; the frontend words the
/// typed [`ServerRequestError`] and decides its log level.
#[tauri::command]
#[specta::specta]
pub async fn send_crash_report(
    app: tauri::AppHandle,
    report_id: String,
    email: Option<String>,
) -> Result<(), ServerRequestError> {
    let email = crate::error_reporter::AttachedEmail::from_flow_a_dialog(email);
    crash_reporter::send_pending_crash_report(&app, &report_id, email).await
}
