//! Custom macOS updater that preserves TCC/Full Disk Access permissions across updates.
//!
//! Instead of replacing the entire `.app` bundle (which changes its inode and causes macOS
//! to lose track of FDA grants), this updater syncs files *into* the existing bundle,
//! preserving the directory inode and `com.apple.macl` xattr.
//!
//! Three Tauri commands:
//! - `check_for_update`: fetches `latest.json`, compares versions
//! - `download_update`: downloads tarball, verifies minisign signature
//! - `install_update`: extracts and syncs into the running `.app` bundle

mod bundle_location;
// Crate-visible for `installer::running_bundle`, which `dock/` needs to decide whether a Dock tile
// could point at this copy. Nothing else outside `updater` reaches in.
pub(crate) mod installer;
mod manifest;
mod signature;

pub use bundle_location::BundleWriteBlocker;
use manifest::UpdateInfo;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tauri::State;

// Per-call timeouts for the manifest fetch. The default `reqwest::get` client has no
// overall timeout. A stuck TCP handshake against the redirect target can hang for
// minutes before the OS gives up. These bounds keep a flaky network from looking like
// a hung app and stop the auto-error-reporter from firing on long hangs.
const MANIFEST_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MANIFEST_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

// Per-call timeouts for the tarball download. No overall `timeout` here: a 60+ MB
// download on a slow connection can legitimately take minutes. `read_timeout` bounds
// "no bytes received in N seconds" instead, which catches mid-download stalls without
// punishing slow-but-working networks.
const DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_READ_TIMEOUT: Duration = Duration::from_secs(30);

use crate::server_request::describe_error_chain;

/// Shared state between `download_update` and `install_update`.
/// Holds the path to the downloaded (and verified) tarball.
pub struct UpdateState {
    downloaded_tarball: Mutex<Option<PathBuf>>,
}

impl UpdateState {
    pub fn new() -> Self {
        Self {
            downloaded_tarball: Mutex::new(None),
        }
    }
}

/// Why this process must not run an update check. Carried (rather than collapsed to a bool) so
/// the log names the exact condition that fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkipReason {
    /// The executable isn't inside a `.app` bundle, so the install can't possibly succeed.
    NotAnAppBundle,
    /// One of [`crate::prod_instance::NON_PROD_ENV_VARS`] is set in this process's environment.
    NonProdEnv(&'static str),
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAnAppBundle => f.write_str("not running from a .app bundle"),
            Self::NonProdEnv(name) => write!(f, "{name} is set"),
        }
    }
}

/// Pure core of the gate, with `in_app_bundle` and `env_is_set` injected so the matrix is
/// unit-testable without mutating the process environment or faking a bundle on disk.
///
/// The bundle condition is checked first: it's the one that makes an update impossible rather
/// than merely unwanted, so it's the more useful thing to see in a log.
fn skip_reason_for(in_app_bundle: bool, env_is_set: &dyn Fn(&str) -> bool) -> Option<SkipReason> {
    if !in_app_bundle {
        return Some(SkipReason::NotAnAppBundle);
    }
    crate::prod_instance::non_prod_env_var_in(env_is_set).map(SkipReason::NonProdEnv)
}

/// The gate against this process's real environment and executable location.
fn skip_reason() -> Option<SkipReason> {
    skip_reason_for(installer::is_running_from_app_bundle(), &|name| {
        std::env::var_os(name).is_some()
    })
}

/// Fetches `latest.json` (via the update check proxy for analytics) and returns update info
/// if a newer version is available.
///
/// Returns `None` when:
/// - This isn't a real user's production install ([`skip_reason`]): the executable isn't inside a
///   `.app` bundle (dev builds: install can't possibly succeed, so there's no point checking and
///   no point letting the user click "Update"), or one of
///   [`crate::prod_instance::NON_PROD_ENV_VARS`] is set. Every check reaches
///   `api.getcmdr.com/update-check`, which writes an `update_checks` row that the dashboard counts
///   as an active install, so Cmdr's own runs must never call it.
/// - The remote version is not newer than the current version
/// - The manifest doesn't contain an entry for this platform
#[tauri::command]
#[specta::specta]
pub async fn check_for_update() -> Result<Option<UpdateInfo>, crate::server_request::ServerRequestError> {
    if let Some(reason) = skip_reason() {
        log::info!("Skipping update check: {reason}");
        return Ok(None);
    }

    let current_version = env!("CARGO_PKG_VERSION");
    log::info!("Checking for updates (current version: {current_version})");

    let arch = manifest::platform_key().strip_prefix("darwin-").unwrap_or("unknown");
    let url = format!("https://api.getcmdr.com/update-check/{current_version}?arch={arch}");

    let manifest = fetch_manifest(&url).await?;
    Ok(manifest::check_manifest(&manifest, current_version))
}

/// Fetches and parses the manifest at `url`. Split from the command so a test can point it at a mock
/// server instead of the update endpoint.
///
/// The status is checked before the body is parsed (`server_request::send`): a 5xx or an HTML
/// maintenance page would otherwise read as "the manifest is malformed" and send whoever reads the log
/// to the wrong layer. A 2xx that doesn't parse is `BadResponse`, which the frontend logs at error:
/// Cmdr's server and this build disagree on the contract. The frontend owns the log line, gated once
/// per condition, so a Rust warn here would only repeat it every poll tick.
async fn fetch_manifest(url: &str) -> Result<manifest::UpdateManifest, crate::server_request::ServerRequestError> {
    let client = reqwest::Client::builder()
        .connect_timeout(MANIFEST_CONNECT_TIMEOUT)
        .timeout(MANIFEST_REQUEST_TIMEOUT)
        .build()
        .map_err(|e| {
            crate::server_request::ServerRequestError::unexpected(format!(
                "update HTTP client: {}",
                describe_error_chain(&e)
            ))
        })?;
    let response = crate::server_request::send(client.get(url)).await?;
    crate::server_request::read_json(response).await
}

/// Reports whether the running bundle sits somewhere an update can be written into, or `None`
/// when nothing is in the way.
///
/// The frontend asks after a check finds an update and before the download starts. Skipping the
/// download is the point: an install that can't write its own bundle would otherwise pull ~63 MB
/// and rewrite nothing, once per poll interval, for as long as the app runs. It also gives the
/// user a reason for a failure they'd otherwise never see, since neither arrangement can be fixed
/// from inside the app.
///
/// Returns `None` outside a `.app` bundle too: there's no bundle to classify, and the check gate
/// (`skip_reason`) has already stopped that process from getting here.
#[tauri::command]
#[specta::specta]
pub async fn update_write_blocker() -> Result<Option<BundleWriteBlocker>, String> {
    let Ok(bundle) = installer::running_bundle() else {
        return Ok(None);
    };
    let blocker = bundle_location::classify(&bundle);
    if let Some(reason) = blocker {
        log::warn!(
            "Can't install updates into {}: {reason}. The user needs to move Cmdr to Applications.",
            bundle.display()
        );
    }
    Ok(blocker)
}

/// Downloads the update tarball and verifies its minisign signature.
///
/// On success, stores the tarball path in `UpdateState` for `install_update` to consume.
#[tauri::command]
#[specta::specta]
pub async fn download_update(url: String, signature: String, state: State<'_, UpdateState>) -> Result<(), String> {
    log::info!("Downloading update from {url}");

    let client = reqwest::Client::builder()
        .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
        .read_timeout(DOWNLOAD_READ_TIMEOUT)
        .build()
        .map_err(|e| format!("Couldn't build update HTTP client: {}", describe_error_chain(&e)))?;

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Couldn't download update: {}", describe_error_chain(&e)))?;

    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Couldn't read update response: {}", describe_error_chain(&e)))?;

    log::info!("Downloaded {} bytes, verifying signature", bytes.len());
    signature::verify(&bytes, &signature)?;
    log::info!("Signature verified");

    let temp_dir = std::env::temp_dir().join("cmdr-update");
    std::fs::create_dir_all(&temp_dir).map_err(|e| format!("Couldn't create temp dir: {e}"))?;

    let tarball_path = temp_dir.join("Cmdr.app.tar.gz");
    std::fs::write(&tarball_path, &bytes).map_err(|e| format!("Couldn't write tarball: {e}"))?;

    let mut guard = state
        .downloaded_tarball
        .lock()
        .map_err(|e| format!("Couldn't lock update state: {e}"))?;
    *guard = Some(tarball_path);

    Ok(())
}

/// Installs a previously downloaded update by syncing files into the running `.app` bundle.
///
/// Reads (and clears) the tarball path stored by `download_update`.
#[tauri::command]
#[specta::specta]
pub async fn install_update(state: State<'_, UpdateState>) -> Result<(), String> {
    let tarball_path = {
        let mut guard = state
            .downloaded_tarball
            .lock()
            .map_err(|e| format!("Couldn't lock update state: {e}"))?;
        guard.take().ok_or_else(|| "No update downloaded".to_string())?
    };

    log::info!("Installing update from {}", tarball_path.display());

    // Run the install on a blocking thread since it does filesystem I/O

    tokio::task::spawn_blocking(move || installer::install(&tarball_path))
        .await
        .map_err(|e| format!("Install task panicked: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Asks the gate about a process running from `in_app_bundle` whose environment holds exactly
    /// `vars` and nothing else.
    fn skip(in_app_bundle: bool, vars: &[&str]) -> Option<SkipReason> {
        let set: HashSet<&str> = vars.iter().copied().collect();
        skip_reason_for(in_app_bundle, &|name| set.contains(name))
    }

    #[test]
    fn a_bundled_release_with_clean_env_may_check() {
        assert_eq!(skip(true, &[]), None);
    }

    #[test]
    fn an_unbundled_build_never_checks() {
        assert_eq!(skip(false, &[]), Some(SkipReason::NotAnAppBundle));
    }

    /// Every non-prod signal suppresses on its own, even from a properly bundled app. A bundled
    /// harness run is exactly the case the old `CI`-only gate let through.
    #[test]
    fn each_non_prod_env_var_suppresses_a_bundled_app() {
        for name in crate::prod_instance::NON_PROD_ENV_VARS {
            assert_eq!(
                skip(true, &[name]),
                Some(SkipReason::NonProdEnv(name)),
                "{name} alone must keep a bundled app off the update-check endpoint"
            );
        }
    }

    /// The gate and the analytics gate must agree about what a real install is, or the dashboard's
    /// `update_checks` ceiling and its heartbeat floor start counting different populations.
    #[test]
    fn every_tooling_launcher_is_suppressed_even_when_bundled() {
        // `scripts/check/checks/e2e-playwright-app.go`.
        let e2e_checker = ["CMDR_INSTANCE_ID", "CMDR_DATA_DIR", "CMDR_E2E_MODE", "CMDR_MOCK_FDA"];
        // `apps/desktop/scripts/i18n-capture.ts`.
        let i18n_capture = ["CMDR_E2E_MODE", "CMDR_DATA_DIR", "CMDR_MOCK_FDA"];
        // `apps/desktop/scripts/marketing-shots.ts` deliberately leaves `CMDR_E2E_MODE` unset.
        let marketing_shots = ["CMDR_DATA_DIR"];
        // `apps/desktop/scripts/tauri-wrapper.ts` (dev and per-worktree dev).
        let dev_wrapper = ["CMDR_INSTANCE_ID", "CMDR_DATA_DIR"];

        for (label, vars) in [
            ("e2e checker", &e2e_checker[..]),
            ("i18n capture", &i18n_capture[..]),
            ("marketing shots", &marketing_shots[..]),
            ("dev wrapper", &dev_wrapper[..]),
        ] {
            assert!(
                skip(true, vars).is_some(),
                "{label} must not reach the update-check endpoint"
            );
        }
    }

    /// The bundle condition wins, so the log names the thing that makes an update impossible
    /// rather than one that merely makes it unwanted.
    #[test]
    fn the_bundle_condition_is_reported_first() {
        assert_eq!(skip(false, &["CMDR_E2E_MODE"]), Some(SkipReason::NotAnAppBundle));
    }

    /// The log has to name the condition, or the next pollution incident is undiagnosable.
    #[test]
    fn reasons_name_the_condition() {
        assert_eq!(SkipReason::NotAnAppBundle.to_string(), "not running from a .app bundle");
        assert_eq!(
            SkipReason::NonProdEnv("CMDR_DATA_DIR").to_string(),
            "CMDR_DATA_DIR is set"
        );
    }

    use crate::server_request::ServerRequestError;
    use serde_json::json;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A mock server answering every GET with `response`, and the manifest URL on it. Keep the server
    /// bound for the test's length: dropping it stops the mock.
    async fn manifest_at(response: ResponseTemplate) -> (MockServer, String) {
        let server = MockServer::start().await;
        Mock::given(method("GET")).respond_with(response).mount(&server).await;
        let url = format!("{}/latest.json", server.uri());
        (server, url)
    }

    #[tokio::test]
    async fn a_manifest_the_server_serves_parses() {
        let (_server, url) = manifest_at(ResponseTemplate::new(200).set_body_json(json!({
            "version": "0.45.1",
            "platforms": { "darwin-aarch64": { "url": "https://example.invalid/Cmdr.tar.gz", "signature": "sig" } }
        })))
        .await;
        let manifest = fetch_manifest(&url).await.expect("a well-formed manifest parses");
        assert_eq!(manifest.version, "0.45.1");
    }

    /// The shape behind the old "Couldn't parse update manifest" lines: a 2xx that isn't a manifest
    /// means Cmdr's server and this build disagree, which the frontend logs at error. It must never
    /// read as a network blip.
    #[tokio::test]
    async fn a_2xx_that_isnt_a_manifest_is_a_bad_response() {
        let (_server, url) =
            manifest_at(ResponseTemplate::new(200).set_body_json(json!({ "version": "0.45.1" }))).await;
        let err = fetch_manifest(&url)
            .await
            .expect_err("a manifest without platforms doesn't parse");
        assert!(
            matches!(err, ServerRequestError::BadResponse { .. }),
            "expected BadResponse, got {err:?}"
        );
    }

    /// A maintenance page on a 5xx stays a refusal with its status, never "the manifest is malformed".
    #[tokio::test]
    async fn a_5xx_maintenance_page_is_refused_not_malformed() {
        let (_server, url) = manifest_at(ResponseTemplate::new(503).set_body_string("<html>maintenance</html>")).await;
        let err = fetch_manifest(&url).await.expect_err("a 503 is a refusal");
        assert!(
            matches!(err, ServerRequestError::Refused { status: 503, .. }),
            "expected a 503 Refused, got {err:?}"
        );
    }
}
