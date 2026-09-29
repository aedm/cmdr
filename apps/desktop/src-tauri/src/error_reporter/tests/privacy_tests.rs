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
const LEGACY_STATE_SENTINELS: [&str; 16] = [
    "PRIVATE-BARE-FILENAME-ACTIVE.txt",
    "PRIVATE-BARE-FILENAME-ROTATED.txt",
    "PRIVATE-NESTED-TAG-ACTIVE",
    "PRIVATE-NESTED-TAG-ROTATED",
    "PRIVATE-TYPE-TO-JUMP-ACTIVE",
    "PRIVATE-TYPE-TO-JUMP-ROTATED",
    "PRIVATE-FAVORITE-ACTIVE",
    "PRIVATE-FAVORITE-ROTATED",
    "PRIVATE-ARCHIVE-ACTIVE.zip",
    "PRIVATE-ARCHIVE-ROTATED.zip",
    "PRIVATE-MOUNT-PROSE-ACTIVE",
    "PRIVATE-MOUNT-PROSE-ROTATED",
    "PRIVATE-MOUNT-SHARE-ACTIVE",
    "PRIVATE-MOUNT-SHARE-ROTATED",
    "PRIVATE-NESTED-ARCHIVE-SOURCE-ACTIVE",
    "PRIVATE-NESTED-ARCHIVE-SOURCE-ROTATED",
];
const LEGACY_PRODUCER_SENTINELS: [&str; 3] = [
    "PRIVATE-MANUAL-SERVER-PROSE-SENTINEL",
    "PRIVATE-DISKUTIL-PROSE-SENTINEL",
    "PRIVATE-SMB-BACKEND-PROSE-SENTINEL",
];

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

fn timestamp(now: DateTime<Utc>) -> String {
    now.with_timezone(&chrono::Local)
        .format("%Y-%m-%dT%H:%M:%S%.3f%:z")
        .to_string()
}

fn current_privacy_log(now: DateTime<Utc>) -> String {
    let stamp = now.with_timezone(&chrono::Local).format("%Y-%m-%dT%H:%M:%S%.3f%:z");
    format!(
        "{stamp} INFO path={path:?}\n\
         {stamp} INFO smb://PRIVATE-REMOTE-USER:PRIVATE-REMOTE-PASSWORD@PRIVATE-REMOTE-HOST.local/PRIVATE-REMOTE-SHARE/{name}?PRIVATE-QUERY-KEY=PRIVATE-QUERY-SECRET#PRIVATE-FRAGMENT-SECRET\n\
         {stamp} INFO server=Some(\"PRIVATE-SERVER-IDENTITY\") user=PRIVATE-ACCOUNT-IDENTITY\n",
        path = state_history::PRIVACY_TEST_RAW_PATH,
        name = state_history::PRIVACY_TEST_RAW_NAME,
    )
}

/// The exact persisted shape emitted before `05945e910`: one timestamped target/header
/// followed by the `cmdr://state` YAML as untimestamped continuation lines. The nested
/// content deliberately covers values lexical line redaction cannot prove private.
fn historical_state_record(now: DateTime<Utc>, suffix: &str) -> String {
    let stamp = timestamp(now);
    format!(
        "{stamp} DEBUG error_reporter::state_snapshot  State at error time:\n\
         generation: 73\n\
         focused: right\n\
         showHidden: true\n\
         right:\n\
           tabs:\n\
             - i:0 PRIVATE-NESTED-ARCHIVE-SOURCE-{suffix} /Users/private-account/Plans/client secret [active]\n\
           volume: PRIVATE-MOUNT-SHARE-{suffix}\n\
           volumeId: smb-private\n\
           path: /Users/private-account/Plans/client secret\n\
           view: brief\n\
           sort: \"name:asc\"\n\
           totalFiles: 1\n\
           loadedRange: [0, 1]\n\
           cursor:\n\
             index: 0\n\
             name: PRIVATE-BARE-FILENAME-{suffix}.txt\n\
           selected: 1\n\
           typeToJump:\n\
             buffer: \"PRIVATE-TYPE-TO-JUMP-{suffix}\"\n\
             indicatorVisible: true\n\
             indicatorStale: false\n\
             lastMatchedName: \"PRIVATE-BARE-FILENAME-{suffix}.txt\"\n\
           mountError:\n\
             share: \"PRIVATE-MOUNT-SHARE-{suffix}\"\n\
             reason: permission_denied\n\
             message: \"PRIVATE-MOUNT-PROSE-{suffix}\"\n\
           files:\n\
             - i:0 f PRIVATE-BARE-FILENAME-{suffix}.txt [tags:PRIVATE-NESTED-TAG-{suffix}]\n\
         dialogs:\n\
           - type: archive-password\n\
             archive: \"PRIVATE-ARCHIVE-{suffix}.zip\"\n\
             archivePath: \"/Users/private-account/Plans/PRIVATE-ARCHIVE-{suffix}.zip\"\n\
             mode: browse\n\
         favorites:\n\
           - id: favorite-private-{suffix}\n\
             name: \"PRIVATE-FAVORITE-{suffix}\"\n\
             path: \"/Users/private-account/PRIVATE-FAVORITE-{suffix}\"\n"
    )
}

fn historical_log(now: DateTime<Utc>, suffix: &str) -> String {
    let before = timestamp(now - chrono::Duration::seconds(5));
    let manual_server = timestamp(now - chrono::Duration::milliseconds(3500));
    let diskutil = timestamp(now - chrono::Duration::seconds(3));
    let smb = timestamp(now - chrono::Duration::seconds(2));
    let backtrace = timestamp(now - chrono::Duration::seconds(1));
    let after = timestamp(now);
    format!(
        "{before} INFO  privacy_test  SAFE-BEFORE-{suffix}\n\
         {}\
         {manual_server} DEBUG network::manual_servers  Unreachable: private.example:445 (PRIVATE-MANUAL-SERVER-PROSE-SENTINEL)\n\
         {diskutil} WARN  network::mount  Failed to unmount /Volumes/private: PRIVATE-DISKUTIL-PROSE-SENTINEL\n\
         diskutil continuation PRIVATE-DISKUTIL-PROSE-SENTINEL\n\
         {smb} WARN  cmdr_smb::volume::session  SmbVolume::read(share=private): PRIVATE-SMB-BACKEND-PROSE-SENTINEL\n\
         backend continuation PRIVATE-SMB-BACKEND-PROSE-SENTINEL\n\
         {backtrace} DEBUG error_reporter::backtrace  Backtrace for retained typed diagnostic:\n\
            0: cmdr_lib::privacy_test::retained_frame_{suffix}\n\
            1: std::panicking::try\n\
         {after} WARN  cmdr_smb::volume::session  SmbVolume::read(share=\"private\"): backend=smb2, error_kind=ConnectionLost\n\
         {after} DEBUG network::manual_servers  Unreachable: host=\"private.example\", port=445, source=os, error_kind=TimedOut, code=60, omitted_bytes=47, omitted_lines=1\n\
         {after} INFO  privacy_test  SAFE-AFTER-{suffix}\n",
        historical_state_record(now - chrono::Duration::seconds(4), suffix),
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
    for private in LEGACY_STATE_SENTINELS.into_iter().chain(LEGACY_PRODUCER_SENTINELS) {
        assert!(
            !archive_text.contains(private),
            "historical private sentinel {private:?} survived:\n{archive_text}"
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

    for file_name in ["logs/cmdr.log", "logs/cmdr.log.1"] {
        let log = entries.get(file_name).unwrap_or_else(|| panic!("missing {file_name}"));
        let suffix = if file_name.ends_with(".1") { "ROTATED" } else { "ACTIVE" };
        for retained in [
            format!("SAFE-BEFORE-{suffix}"),
            format!("cmdr_lib::privacy_test::retained_frame_{suffix}"),
            "std::panicking::try".to_string(),
            "backend=smb2, error_kind=ConnectionLost".to_string(),
            "source=os, error_kind=TimedOut, code=60".to_string(),
            format!("SAFE-AFTER-{suffix}"),
        ] {
            assert!(
                log.contains(&retained),
                "safe record {retained:?} missing from {file_name}: {log}"
            );
        }
    }

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
    let rotated_log = dir.join("cmdr.log.1");
    let now = Utc::now();
    std::fs::write(
        &log,
        format!("{}{}", current_privacy_log(now), historical_log(now, "ACTIVE")),
    )
    .expect("write adversarial active log");
    std::fs::write(&rotated_log, historical_log(now, "ROTATED")).expect("write adversarial rotated log");

    // Preview/send rebuilds construct the context again from the same report ID. Drive each
    // production ZIP pipeline with a separately-created context to pin that reuse contract.
    let streaming_context = redact::RedactionContext::for_report(PRIVACY_REPORT_ID);
    let streaming = build_bundle_streaming(
        PRIVACY_REPORT_ID.to_string(),
        privacy_manifest(&streaming_context),
        vec![log.clone(), rotated_log.clone()],
        now - chrono::Duration::hours(1),
        SystemTime::now(),
        &streaming_context,
    )
    .expect("streaming privacy bundle");

    let legacy_context = redact::RedactionContext::for_report(PRIVACY_REPORT_ID);
    let legacy = build_bundle_legacy_window(
        PRIVACY_REPORT_ID.to_string(),
        privacy_manifest(&legacy_context),
        vec![log, rotated_log],
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
