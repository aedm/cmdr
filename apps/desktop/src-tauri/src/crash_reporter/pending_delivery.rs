//! Delivery and dismissal of the backend-authoritative pending crash report.

use super::{CRASH_FILE_NAME, CRASH_SHORT_ID_PREFIX, CrashReport, read_crash_report};
use crate::config;
use crate::server_request::{self, ServerRequestError};
use std::future::Future;
use std::path::Path;

/// Crash-report ingestion endpoint. Debug builds keep their existing localhost target.
#[cfg(debug_assertions)]
const CRASH_REPORT_URL: &str = "http://localhost:8787/crash-report";
#[cfg(not(debug_assertions))]
const CRASH_REPORT_URL: &str = "https://api.getcmdr.com/crash-report";

/// Deletes the pending report when the user dismisses its dialog.
pub fn dismiss_pending_crash_report<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let Ok(data_dir) = config::resolved_app_data_dir(app) else {
        return;
    };
    let _ = std::fs::remove_file(data_dir.join(CRASH_FILE_NAME));
}

/// Reloads and sends the backend-owned pending report identified by the preview's short id.
///
/// The frontend supplies consent, not report contents. Reloading here makes the file authoritative;
/// comparing its id before upload prevents a stale preview from sending a replacement report, and
/// comparing again before deletion preserves a replacement already present at that check. The check
/// and remove are not atomic, so they narrow rather than eliminate the replacement race. A request
/// that doesn't land keeps the original file for the next launch.
pub async fn send_pending_crash_report<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    report_id: &str,
    email: Option<crate::error_reporter::AttachedEmail>,
) -> Result<(), ServerRequestError> {
    let data_dir = config::resolved_app_data_dir(app)
        .map_err(|e| ServerRequestError::unexpected(format!("resolve crash-report directory: {e}")))?;
    let crash_path = data_dir.join(CRASH_FILE_NAME);
    let should_skip = cfg!(debug_assertions) || cfg!(feature = "playwright-e2e") || std::env::var("CI").is_ok();

    send_pending_crash_report_from_path(&crash_path, report_id, email, should_skip, |report| async move {
        post_crash_report(CRASH_REPORT_URL, &report).await
    })
    .await
}

pub(super) async fn send_pending_crash_report_from_path<Upload, UploadFuture>(
    crash_path: &Path,
    report_id: &str,
    email: Option<crate::error_reporter::AttachedEmail>,
    should_skip: bool,
    upload: Upload,
) -> Result<(), ServerRequestError>
where
    Upload: FnOnce(CrashReport) -> UploadFuture,
    UploadFuture: Future<Output = Result<(), ServerRequestError>>,
{
    let mut report = read_crash_report(crash_path)
        .ok_or_else(|| ServerRequestError::unexpected("pending crash report changed before send"))?;
    report.prepare_for_delivery();
    if report.short_id.as_deref() != Some(report_id) {
        return Err(ServerRequestError::unexpected(
            "pending crash report changed before send",
        ));
    }
    report.prepare_for_send(email);

    if should_skip {
        log::info!("Crash reporter: skipping send (debug build, E2E build, or CI)");
    } else {
        upload(report).await?;
    }

    // The upload may have yielded long enough for another pending artifact to replace this one.
    // Preserve a replacement already visible now. This read and the remove are not atomic.
    if pending_report_id(crash_path).as_deref() == Some(report_id) {
        let _ = std::fs::remove_file(crash_path);
    }
    Ok(())
}

pub(super) fn pending_report_id(path: &Path) -> Option<String> {
    read_crash_report(path)?
        .short_id
        .filter(|id| crate::short_id::matches(CRASH_SHORT_ID_PREFIX, id))
}

/// POSTs one backend-owned report to `url`. Split from the send flow so tests can point it at a
/// mock server without weakening the file/id boundary above.
pub(super) async fn post_crash_report(url: &str, report: &CrashReport) -> Result<(), ServerRequestError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| ServerRequestError::unexpected(format!("HTTP client: {e}")))?;
    server_request::send(client.post(url).json(report)).await?;
    Ok(())
}
