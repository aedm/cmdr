//! Archive-level privacy contract shared by the streaming and legacy ZIP pipelines.

use super::{read_zip_entries, sample_manifest};
use crate::error_reporter::bundle_builder::{build_bundle_legacy_window, build_bundle_streaming};
use crate::error_reporter::{BuiltBundle, BundleManifest, BundleScope, breadcrumbs, state_history};
use crate::redact;
use crate::test_support::TestDir;
use chrono::{DateTime, Utc};
use std::time::SystemTime;

const PRIVACY_REPORT_ID: &str = "ERR-AB23X";
const EXPLICIT_NOTE: &str = "EXPLICIT-NOTE-SENTINEL: I consent to share /Users/explicit-consent/Exact note.txt";
const EXPLICIT_EMAIL: &str = "explicit-email-sentinel@example.test";
const LEGACY_BREADCRUMB_SENTINEL: &str = "PRIVATE-LEGACY-BREADCRUMB-SENTINEL";

fn privacy_manifest(redaction: &redact::RedactionContext) -> BundleManifest {
    let mut manifest = sample_manifest();
    manifest.id = PRIVACY_REPORT_ID.to_string();
    manifest.breadcrumbs = vec![breadcrumbs::Breadcrumb {
        at: "2027-01-15T08:00:00+00:00".to_string(),
        event: breadcrumbs::BreadcrumbEvent::Command {
            command_id: "pane.switch".to_string(),
        },
    }];
    manifest.state_history = state_history::privacy_fixture_for_test(redaction);
    manifest.user_note = Some(EXPLICIT_NOTE.to_string());
    manifest.email = Some(EXPLICIT_EMAIL.to_string());
    manifest
}

fn privacy_log(now: DateTime<Utc>) -> String {
    let stamp = now.with_timezone(&chrono::Local).format("%Y-%m-%dT%H:%M:%S%.3f%:z");
    format!(
        "{stamp} INFO path={path:?}\n\
         {stamp} INFO smb://PRIVATE-REMOTE-USER:PRIVATE-REMOTE-PASSWORD@PRIVATE-REMOTE-HOST.local/PRIVATE-REMOTE-SHARE/{name}?PRIVATE-QUERY-KEY=PRIVATE-QUERY-SECRET#PRIVATE-FRAGMENT-SECRET\n\
         {stamp} INFO server=Some(\"PRIVATE-SERVER-IDENTITY\") user=PRIVATE-ACCOUNT-IDENTITY\n",
        path = state_history::PRIVACY_TEST_RAW_PATH,
        name = state_history::PRIVACY_TEST_RAW_NAME,
    )
}

fn assert_privacy_archive(bundle: &BuiltBundle) -> BundleManifest {
    let entries = read_zip_entries(&bundle.zip_bytes);
    let archive_text = entries
        .iter()
        .map(|(name, body)| format!("{name}\n{body}"))
        .collect::<Vec<_>>()
        .join("\n");
    for private in [
        "private-account",
        "client secret",
        " secret",
        state_history::PRIVACY_TEST_RAW_NAME,
        state_history::PRIVACY_TEST_RAW_PATH,
        state_history::PRIVACY_TEST_EXTERNAL_PROSE,
        "PRIVATE-VOLUME-NAME-SENTINEL",
        "PRIVATE-REMOTE-USER",
        "PRIVATE-REMOTE-PASSWORD",
        "PRIVATE-REMOTE-HOST",
        "PRIVATE-REMOTE-SHARE",
        "PRIVATE-QUERY-KEY",
        "PRIVATE-QUERY-SECRET",
        "PRIVATE-FRAGMENT-SECRET",
        "PRIVATE-SERVER-IDENTITY",
        "PRIVATE-ACCOUNT-IDENTITY",
        "PRIVATE-FREE-FORM-PAYLOAD-SENTINEL",
        LEGACY_BREADCRUMB_SENTINEL,
    ] {
        assert!(
            !archive_text.contains(private),
            "private sentinel {private:?} survived:\n{archive_text}"
        );
    }

    // These two fields are deliberately outside the privacy transform: the person previewed
    // them and explicitly attached them to this report.
    assert!(archive_text.contains(EXPLICIT_NOTE));
    assert!(archive_text.contains(EXPLICIT_EMAIL));

    let manifest: BundleManifest = serde_json::from_str(entries.get("manifest.json").expect("manifest entry")).unwrap();
    assert_eq!(manifest.user_note.as_deref(), Some(EXPLICIT_NOTE));
    assert_eq!(manifest.email.as_deref(), Some(EXPLICIT_EMAIL));
    assert_eq!(manifest.breadcrumbs.len(), 1);

    let state = manifest.state_history.first().expect("typed state history");
    assert_eq!(state.generation, 73);
    assert_eq!(state.focused, Some(state_history::PaneSide::Right));
    assert!(state.show_hidden);
    assert_eq!(state.recent_listing_error_count, 5);
    let pane = state.panes.first().expect("pane state");
    assert_eq!(pane.backend, Some(state_history::ReportBackend::Smb));
    assert_eq!(pane.connection, Some(state_history::ReportConnectionState::NeedsSignIn));
    assert_eq!(pane.selected_files, 1);
    assert_eq!(pane.selected_folders, 1);
    assert_eq!(pane.tab_count, 1);
    let cursor = pane.cursor.as_ref().expect("cursor identity");
    let operation = state.operations.first().expect("operation state");
    assert_eq!(
        operation.operation_id.as_deref(),
        Some("0199a2e7-47d8-7c31-a897-c58f415f4f91")
    );
    assert_eq!(operation.files_done, 7);
    assert_eq!(operation.files_total, 19);
    assert_eq!(operation.bytes_done, 11);
    assert_eq!(operation.bytes_total, 23);
    assert_eq!(operation.current_file.as_deref(), Some(cursor.name.as_str()));

    let log = entries.get("logs/cmdr.log").expect("redacted log entry");
    assert!(
        log.contains(&cursor.name),
        "the same raw name must correlate between typed state and logs: {log}"
    );
    manifest
}

#[test]
fn both_zip_pipelines_apply_one_report_context_to_every_diagnostic_surface() {
    let legacy_payload = serde_json::json!({
        "kind": "command",
        "message": LEGACY_BREADCRUMB_SENTINEL,
        "ctx": { "freeForm": "PRIVATE-FREE-FORM-PAYLOAD-SENTINEL" }
    });
    assert!(
        serde_json::from_value::<breadcrumbs::BreadcrumbEvent>(legacy_payload).is_err(),
        "the legacy free-form shape must not be able to enter either manifest"
    );

    let dir = TestDir::new("error-reporter-privacy-archives");
    let log = dir.join("cmdr.log");
    let now = Utc::now();
    std::fs::write(&log, privacy_log(now)).expect("write adversarial log");

    // Preview/send rebuilds construct the context again from the same report ID. Drive each
    // production ZIP pipeline with a separately-created context to pin that reuse contract.
    let streaming_context = redact::RedactionContext::for_report(PRIVACY_REPORT_ID);
    let streaming = build_bundle_streaming(
        PRIVACY_REPORT_ID.to_string(),
        privacy_manifest(&streaming_context),
        vec![log.clone()],
        now - chrono::Duration::hours(1),
        SystemTime::now(),
        &streaming_context,
    )
    .expect("streaming privacy bundle");

    let legacy_context = redact::RedactionContext::for_report(PRIVACY_REPORT_ID);
    let legacy = build_bundle_legacy_window(
        PRIVACY_REPORT_ID.to_string(),
        privacy_manifest(&legacy_context),
        vec![log],
        BundleScope::Window { first_error_at: now },
        now,
        SystemTime::now(),
        &legacy_context,
    )
    .expect("legacy privacy bundle");

    let streaming_manifest = assert_privacy_archive(&streaming);
    let legacy_manifest = assert_privacy_archive(&legacy);
    assert_eq!(
        streaming_manifest.state_history[0].panes[0].path, legacy_manifest.state_history[0].panes[0].path,
        "one report ID must reproduce one correlation context across rebuilds"
    );

    let other_context = redact::RedactionContext::for_report("ERR-CD45Y");
    let other_state = state_history::privacy_fixture_for_test(&other_context);
    assert_ne!(
        streaming_manifest.state_history[0].panes[0].path, other_state[0].panes[0].path,
        "a different report must not reuse the first report's tokens"
    );
}
