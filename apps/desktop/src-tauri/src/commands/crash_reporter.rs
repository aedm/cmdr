//! Crash reporter Tauri commands.
//!
//! Thin wrappers for crash file detection, dismissal, and sending.

use crate::config;
use crate::crash_reporter::{self, CrashReport};
use crate::server_request::{self, ServerRequestError};

/// Server URL for crash report ingestion.
#[cfg(debug_assertions)]
const CRASH_REPORT_URL: &str = "http://localhost:8787/crash-report";

#[cfg(not(debug_assertions))]
const CRASH_REPORT_URL: &str = "https://api.getcmdr.com/crash-report";

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
    let Ok(data_dir) = config::resolved_app_data_dir(&app) else {
        return;
    };
    let crash_path = data_dir.join("crash-report.json");
    let _ = std::fs::remove_file(crash_path);
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
pub async fn send_crash_report(app: tauri::AppHandle, report: CrashReport) -> Result<(), ServerRequestError> {
    let should_skip = cfg!(debug_assertions) || cfg!(feature = "playwright-e2e") || std::env::var("CI").is_ok();

    if should_skip {
        log::info!("Crash reporter: skipping send (debug build, E2E build, or CI)");
    } else {
        post_crash_report(CRASH_REPORT_URL, &report).await?;
    }

    // Delete the local crash file after successful send (or skip)
    if let Ok(data_dir) = config::resolved_app_data_dir(&app) {
        let crash_path = data_dir.join("crash-report.json");
        let _ = std::fs::remove_file(crash_path);
    }

    Ok(())
}

/// POSTs one report to `url`. Split from the command so a test can point it at a mock server.
async fn post_crash_report(url: &str, report: &CrashReport) -> Result<(), ServerRequestError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| ServerRequestError::unexpected(format!("HTTP client: {e}")))?;
    server_request::send(client.post(url).json(report)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn a_report() -> CrashReport {
        serde_json::from_value(serde_json::json!({
            "version": 1,
            "timestamp": "2026-09-13T10:00:00Z",
            "signal": null,
            "panicMessage": "main thread panicked",
            "backtraceFrames": [],
            "threadName": "main",
            "threadCount": 1,
            "appVersion": "1.2.3",
            "osVersion": "macOS 15.3",
            "arch": "aarch64",
            "uptimeSecs": 120,
            "activeSettings": {
                "indexingEnabled": true,
                "aiProvider": "off",
                "mcpEnabled": false,
                "verboseLogging": false
            },
            "possibleCrashLoop": false
        }))
        .expect("a minimal crash report deserializes")
    }

    async fn server_answering(status: u16) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/crash-report"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn a_report_the_server_accepts_lands() {
        let server = server_answering(204).await;
        post_crash_report(&format!("{}/crash-report", server.uri()), &a_report())
            .await
            .expect("a 204 lands");
    }

    /// A 4xx is Cmdr and its server disagreeing, which the frontend logs at error; it must arrive
    /// with its status rather than as a sentence.
    #[tokio::test]
    async fn a_report_the_server_turns_down_comes_back_refused_with_its_status() {
        let server = server_answering(400).await;
        let err = post_crash_report(&format!("{}/crash-report", server.uri()), &a_report())
            .await
            .expect_err("a 400 is a refusal");
        assert!(
            matches!(err, ServerRequestError::Refused { status: 400, .. }),
            "expected a 400 Refused, got {err:?}"
        );
    }
}
