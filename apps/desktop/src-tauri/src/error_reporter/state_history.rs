//! Bounded, process-local state history for error-report bundles.
//!
//! Capture clones typed in-memory stores only. Raw names and paths live in this module's
//! private input structs and are transformed with the bundle's [`RedactionContext`] when the
//! manifest is assembled. Nothing here writes to logs or disk.

use crate::file_system::volume::{BackendKind, ConnectionState};
use crate::file_system::write_operations::{
    LifecycleStatus, WriteOperationPhase, WriteOperationType, get_operation_status, list_operations,
};
use crate::ignore_poison::IgnorePoison;
use crate::mcp::PaneStateStore;
use crate::mcp::pane_state::PaneState;
use crate::redact::RedactionContext;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tauri::Manager;

const CAPACITY: usize = 8;
const THROTTLE: Duration = Duration::from_secs(30);

static HISTORY: LazyLock<Mutex<StateHistory>> = LazyLock::new(|| Mutex::new(StateHistory::default()));

#[derive(Default)]
struct StateHistory {
    last_capture_at: Option<Instant>,
    next_sequence: u64,
    snapshots: VecDeque<RawStateSnapshot>,
}

impl StateHistory {
    fn reserve(&mut self, now: Instant) -> Option<u64> {
        if self
            .last_capture_at
            .and_then(|last| now.checked_duration_since(last))
            .is_some_and(|elapsed| elapsed < THROTTLE)
        {
            return None;
        }
        self.last_capture_at = Some(now);
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        Some(sequence)
    }

    fn store(&mut self, snapshot: RawStateSnapshot) {
        let insertion = self
            .snapshots
            .iter()
            .position(|existing| existing.sequence > snapshot.sequence)
            .unwrap_or(self.snapshots.len());
        self.snapshots.insert(insertion, snapshot);
        while self.snapshots.len() > CAPACITY {
            self.snapshots.pop_front();
        }
    }

    #[cfg(test)]
    fn record(&mut self, now: Instant, mut snapshot: RawStateSnapshot) -> bool {
        let Some(sequence) = self.reserve(now) else {
            return false;
        };
        snapshot.sequence = sequence;
        self.store(snapshot);
        true
    }

    fn snapshot(&self) -> Vec<RawStateSnapshot> {
        self.snapshots.iter().cloned().collect()
    }
}

/// Reserve this error's throttled slot and capture typed state away from the logging caller.
pub(super) fn capture_if_due(app: tauri::AppHandle<tauri::Wry>) {
    let sequence = {
        let mut history = HISTORY.lock_ignore_poison();
        let Some(sequence) = history.reserve(Instant::now()) else {
            return;
        };
        sequence
    };
    tauri::async_runtime::spawn(async move {
        let Some(snapshot) = capture(&app, sequence, Utc::now()) else {
            return;
        };
        HISTORY.lock_ignore_poison().store(snapshot);
    });
}

/// Transform the current process's history with one report's correlation context.
pub(super) fn for_report(redaction: &RedactionContext) -> Vec<DiagnosticStateSnapshot> {
    let snapshots = HISTORY.lock_ignore_poison().snapshot();
    redact_snapshots(&snapshots, redaction)
}

fn capture<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    sequence: u64,
    captured_at: DateTime<Utc>,
) -> Option<RawStateSnapshot> {
    let store = app.try_state::<PaneStateStore>()?;
    let left = store.get_left();
    let right = store.get_right();
    Some(RawStateSnapshot {
        sequence,
        captured_at,
        generation: store.get_generation(),
        focused: PaneSide::parse(&store.get_focused_pane()),
        show_hidden: left.show_hidden,
        panes: vec![
            capture_pane(PaneSide::Left, &left),
            capture_pane(PaneSide::Right, &right),
        ],
        operations: capture_operations(),
        recent_listing_error_count: crate::mcp::listing_errors::snapshot().len(),
    })
}

fn capture_pane(side: PaneSide, pane: &PaneState) -> RawPaneSnapshot {
    let volume = pane
        .volume_id
        .as_deref()
        .and_then(|id| crate::file_system::volume::manager::get_volume_manager().get(id));
    let cursor = pane
        .cursor_index
        .checked_sub(pane.loaded_start)
        .and_then(|index| pane.files.get(index))
        .map(|entry| RawEntryIdentity {
            name: entry.name.clone(),
            path: entry.path.clone(),
            role: if entry.is_directory {
                EntryRole::Folder
            } else {
                EntryRole::File
            },
        });
    let (selected_files, selected_folders) = pane
        .selected_indices
        .iter()
        .filter_map(|index| index.checked_sub(pane.loaded_start))
        .filter_map(|index| pane.files.get(index))
        .fold((0usize, 0usize), |(files, folders), entry| {
            if entry.is_directory {
                (files, folders + 1)
            } else {
                (files + 1, folders)
            }
        });
    RawPaneSnapshot {
        side,
        path: pane.path.clone(),
        volume_id: pane.volume_id.clone(),
        volume_name: pane.volume_name.clone(),
        backend: volume.as_ref().map(|volume| volume.backend_kind().into()),
        connection: volume.and_then(|volume| volume.connection_state()).map(Into::into),
        view: PaneView::parse(&pane.view_mode),
        sort_field: PaneSortField::parse(&pane.sort_field),
        sort_order: PaneSortOrder::parse(&pane.sort_order),
        total_files: pane.total_files,
        loaded_count: pane.files.len(),
        cursor_index: pane.cursor_index,
        cursor,
        selected_count: pane.selected_indices.len(),
        selected_files,
        selected_folders,
        tab_count: pane.tabs.len(),
    }
}

fn capture_operations() -> Vec<RawOperationSnapshot> {
    list_operations()
        .into_iter()
        .map(|operation| {
            let progress = get_operation_status(&operation.operation_id);
            RawOperationSnapshot {
                operation_id: keep_operation_id(&operation.operation_id),
                operation_type: operation.operation_type,
                lifecycle: operation.status,
                phase: progress.as_ref().map(|status| status.phase),
                source: operation.source,
                destination: operation.destination,
                current_file: progress.as_ref().and_then(|status| status.current_file.clone()),
                files_done: progress.as_ref().map_or(0, |status| status.files_done),
                files_total: progress.as_ref().map_or(0, |status| status.files_total),
                bytes_done: progress.as_ref().map_or(0, |status| status.bytes_done),
                bytes_total: progress.as_ref().map_or(0, |status| status.bytes_total),
            }
        })
        .collect()
}

fn keep_operation_id(id: &str) -> Option<String> {
    uuid::Uuid::parse_str(id)
        .ok()
        .filter(|id| id.get_version() == Some(uuid::Version::SortRand))
        .map(|_| id.to_string())
}

fn redact_snapshots(snapshots: &[RawStateSnapshot], redaction: &RedactionContext) -> Vec<DiagnosticStateSnapshot> {
    snapshots
        .iter()
        .map(|snapshot| DiagnosticStateSnapshot {
            captured_at: snapshot.captured_at.to_rfc3339(),
            generation: snapshot.generation,
            focused: snapshot.focused,
            show_hidden: snapshot.show_hidden,
            panes: snapshot
                .panes
                .iter()
                .map(|pane| DiagnosticPaneSnapshot {
                    side: pane.side,
                    path: redaction.redact_path(&pane.path),
                    volume_id: pane.volume_id.as_deref().map(|id| redaction.redact_volume_id(id)),
                    volume_name: pane
                        .volume_name
                        .as_deref()
                        .map(|name| redaction.redact_volume_name(name)),
                    backend: pane.backend,
                    connection: pane.connection,
                    view: pane.view,
                    sort_field: pane.sort_field,
                    sort_order: pane.sort_order,
                    total_files: pane.total_files,
                    loaded_count: pane.loaded_count,
                    cursor_index: pane.cursor_index,
                    cursor: pane.cursor.as_ref().map(|cursor| DiagnosticEntryIdentity {
                        name: redaction.redact_name(&cursor.name, cursor.role == EntryRole::Folder),
                        path: redaction.redact_path(&cursor.path),
                        role: cursor.role,
                    }),
                    selected_count: pane.selected_count,
                    selected_files: pane.selected_files,
                    selected_folders: pane.selected_folders,
                    tab_count: pane.tab_count,
                })
                .collect(),
            operations: snapshot
                .operations
                .iter()
                .map(|operation| DiagnosticOperationSnapshot {
                    operation_id: operation.operation_id.clone(),
                    operation_type: operation.operation_type,
                    lifecycle: operation.lifecycle,
                    phase: operation.phase,
                    source: operation.source.as_deref().map(|path| redaction.redact_path(path)),
                    destination: operation.destination.as_deref().map(|path| redaction.redact_path(path)),
                    current_file: operation
                        .current_file
                        .as_deref()
                        .map(|name| redaction.redact_name(name, false)),
                    files_done: operation.files_done,
                    files_total: operation.files_total,
                    bytes_done: operation.bytes_done,
                    bytes_total: operation.bytes_total,
                })
                .collect(),
            recent_listing_error_count: snapshot.recent_listing_error_count,
        })
        .collect()
}

#[derive(Debug, Clone)]
struct RawStateSnapshot {
    sequence: u64,
    captured_at: DateTime<Utc>,
    generation: u64,
    focused: Option<PaneSide>,
    show_hidden: bool,
    panes: Vec<RawPaneSnapshot>,
    operations: Vec<RawOperationSnapshot>,
    recent_listing_error_count: usize,
}

#[derive(Debug, Clone)]
struct RawPaneSnapshot {
    side: PaneSide,
    path: String,
    volume_id: Option<String>,
    volume_name: Option<String>,
    backend: Option<ReportBackend>,
    connection: Option<ReportConnectionState>,
    view: Option<PaneView>,
    sort_field: Option<PaneSortField>,
    sort_order: Option<PaneSortOrder>,
    total_files: usize,
    loaded_count: usize,
    cursor_index: usize,
    cursor: Option<RawEntryIdentity>,
    selected_count: usize,
    selected_files: usize,
    selected_folders: usize,
    tab_count: usize,
}

#[derive(Debug, Clone)]
struct RawEntryIdentity {
    name: String,
    path: String,
    role: EntryRole,
}

#[derive(Debug, Clone)]
struct RawOperationSnapshot {
    operation_id: Option<String>,
    operation_type: WriteOperationType,
    lifecycle: LifecycleStatus,
    phase: Option<WriteOperationPhase>,
    source: Option<String>,
    destination: Option<String>,
    current_file: Option<String>,
    files_done: usize,
    files_total: usize,
    bytes_done: u64,
    bytes_total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticStateSnapshot {
    pub captured_at: String,
    pub generation: u64,
    pub focused: Option<PaneSide>,
    pub show_hidden: bool,
    pub panes: Vec<DiagnosticPaneSnapshot>,
    pub operations: Vec<DiagnosticOperationSnapshot>,
    pub recent_listing_error_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticPaneSnapshot {
    pub side: PaneSide,
    pub path: String,
    pub volume_id: Option<String>,
    pub volume_name: Option<String>,
    pub backend: Option<ReportBackend>,
    pub connection: Option<ReportConnectionState>,
    pub view: Option<PaneView>,
    pub sort_field: Option<PaneSortField>,
    pub sort_order: Option<PaneSortOrder>,
    pub total_files: usize,
    pub loaded_count: usize,
    pub cursor_index: usize,
    pub cursor: Option<DiagnosticEntryIdentity>,
    pub selected_count: usize,
    pub selected_files: usize,
    pub selected_folders: usize,
    pub tab_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticEntryIdentity {
    pub name: String,
    pub path: String,
    pub role: EntryRole,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticOperationSnapshot {
    pub operation_id: Option<String>,
    pub operation_type: WriteOperationType,
    pub lifecycle: LifecycleStatus,
    pub phase: Option<WriteOperationPhase>,
    pub source: Option<String>,
    pub destination: Option<String>,
    pub current_file: Option<String>,
    pub files_done: usize,
    pub files_total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum PaneSide {
    Left,
    Right,
}

impl PaneSide {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum EntryRole {
    File,
    Folder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum PaneView {
    Brief,
    Full,
}

impl PaneView {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "brief" => Some(Self::Brief),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum PaneSortField {
    Name,
    Extension,
    Size,
    Modified,
    Relevance,
}

impl PaneSortField {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "name" => Some(Self::Name),
            "extension" => Some(Self::Extension),
            "size" => Some(Self::Size),
            "modified" => Some(Self::Modified),
            "relevance" => Some(Self::Relevance),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum PaneSortOrder {
    Ascending,
    Descending,
}

impl PaneSortOrder {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "asc" => Some(Self::Ascending),
            "desc" => Some(Self::Descending),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ReportBackend {
    Local,
    Smb,
    Sftp,
    Webdav,
    Mtp,
    Adb,
    Archive,
    GitPortal,
}

impl From<BackendKind> for ReportBackend {
    fn from(value: BackendKind) -> Self {
        match value {
            BackendKind::Local => Self::Local,
            BackendKind::Smb => Self::Smb,
            BackendKind::Sftp => Self::Sftp,
            BackendKind::Webdav => Self::Webdav,
            BackendKind::Mtp => Self::Mtp,
            BackendKind::Adb => Self::Adb,
            BackendKind::Archive => Self::Archive,
            BackendKind::GitPortal => Self::GitPortal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ReportConnectionState {
    Direct,
    OsMount,
    Disconnected,
    NeedsSignIn,
    NeedsHostKeyApproval,
    Saved,
}

impl From<ConnectionState> for ReportConnectionState {
    fn from(value: ConnectionState) -> Self {
        match value {
            ConnectionState::Direct => Self::Direct,
            ConnectionState::OsMount => Self::OsMount,
            ConnectionState::Disconnected => Self::Disconnected,
            ConnectionState::NeedsSignIn => Self::NeedsSignIn,
            ConnectionState::NeedsHostKeyApproval => Self::NeedsHostKeyApproval,
            ConnectionState::Saved => Self::Saved,
        }
    }
}

#[cfg(test)]
pub(super) const PRIVACY_TEST_RAW_NAME: &str = "PRIVATE-NAME-SENTINEL-ASYMMETRIC.pdf";
#[cfg(test)]
pub(super) const PRIVACY_TEST_RAW_PATH: &str = "/Users/private-account/Projects/PRIVATE-NAME-SENTINEL-ASYMMETRIC.pdf";
#[cfg(test)]
pub(super) const PRIVACY_TEST_EXTERNAL_PROSE: &str = "PRIVATE-EXTERNAL-PROSE-SENTINEL-ASYMMETRIC";

/// Build adversarial raw state through the same pane projection and report transform as
/// production. The omitted MCP-only fields deliberately carry prose and names so archive-level
/// tests prove that functional MCP state did not become diagnostic payload by accident.
#[cfg(test)]
pub(super) fn privacy_fixture_for_test(redaction: &RedactionContext) -> Vec<DiagnosticStateSnapshot> {
    use crate::mcp::pane_state::{MountErrorInfo, PaneFileEntry, TabInfo, TypeToJumpInfo};

    let pane = PaneState {
        path: PRIVACY_TEST_RAW_PATH.to_string(),
        volume_id: Some("smb-private-host-private-share-0123456789abcdef".to_string()),
        volume_name: Some("PRIVATE-VOLUME-NAME-SENTINEL".to_string()),
        files: vec![
            PaneFileEntry {
                name: PRIVACY_TEST_RAW_NAME.to_string(),
                path: PRIVACY_TEST_RAW_PATH.to_string(),
                ..Default::default()
            },
            PaneFileEntry {
                name: PRIVACY_TEST_EXTERNAL_PROSE.to_string(),
                path: format!("/Users/private-account/{PRIVACY_TEST_EXTERNAL_PROSE}"),
                is_directory: true,
                ..Default::default()
            },
        ],
        cursor_index: 0,
        view_mode: "full".to_string(),
        selected_indices: vec![0, 1],
        sort_field: "size".to_string(),
        sort_order: "desc".to_string(),
        total_files: 91,
        loaded_end: 2,
        show_hidden: true,
        tabs: vec![TabInfo {
            id: PRIVACY_TEST_EXTERNAL_PROSE.to_string(),
            path: format!("/Users/private-account/{PRIVACY_TEST_EXTERNAL_PROSE}"),
            pinned: true,
            active: true,
        }],
        type_to_jump: Some(TypeToJumpInfo {
            buffer: PRIVACY_TEST_EXTERNAL_PROSE.to_string(),
            indicator_visible: true,
            indicator_stale: false,
            last_matched_name: Some(PRIVACY_TEST_EXTERNAL_PROSE.to_string()),
        }),
        mount_error: Some(MountErrorInfo {
            share: "PRIVATE-REMOTE-SHARE-SENTINEL".to_string(),
            reason: "timeout".to_string(),
            message: PRIVACY_TEST_EXTERNAL_PROSE.to_string(),
        }),
        ..Default::default()
    };
    let mut raw_pane = capture_pane(PaneSide::Right, &pane);
    raw_pane.backend = Some(ReportBackend::Smb);
    raw_pane.connection = Some(ReportConnectionState::NeedsSignIn);

    let raw = RawStateSnapshot {
        sequence: 0,
        captured_at: DateTime::parse_from_rfc3339("2027-01-15T08:00:00+00:00")
            .expect("test timestamp is valid")
            .with_timezone(&Utc),
        generation: 73,
        focused: Some(PaneSide::Right),
        show_hidden: true,
        panes: vec![raw_pane],
        operations: vec![RawOperationSnapshot {
            operation_id: Some("0199a2e7-47d8-7c31-a897-c58f415f4f91".to_string()),
            operation_type: WriteOperationType::Copy,
            lifecycle: LifecycleStatus::Paused,
            phase: Some(WriteOperationPhase::Copying),
            source: Some(PRIVACY_TEST_RAW_PATH.to_string()),
            destination: Some(
                "smb://private-user:private-password@private-host.local/private-share/private-folder".to_string(),
            ),
            current_file: Some(PRIVACY_TEST_RAW_NAME.to_string()),
            files_done: 7,
            files_total: 19,
            bytes_done: 11,
            bytes_total: 23,
        }],
        recent_listing_error_count: 5,
    };
    redact_snapshots(&[raw], redaction)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_system::write_operations::{LifecycleStatus, WriteOperationPhase, WriteOperationType};
    use crate::redact::RedactionContext;
    use chrono::{TimeZone, Utc};
    use std::time::{Duration, Instant};

    const RAW_NAME: &str = "PRIVATE-NAME-ASYMMETRIC";
    const RAW_PATH: &str = "/Users/private/Projects/PRIVATE-NAME-ASYMMETRIC/report.pdf";
    const OPERATION_ID: &str = "0199a2e7-47d8-7c31-a897-c58f415f4f91";

    fn raw(sequence: u64) -> RawStateSnapshot {
        RawStateSnapshot {
            sequence,
            captured_at: Utc.timestamp_opt(1_800_000_000 + sequence as i64, 0).unwrap(),
            generation: 40 + sequence,
            focused: Some(PaneSide::Right),
            show_hidden: true,
            panes: vec![RawPaneSnapshot {
                side: PaneSide::Right,
                path: RAW_PATH.to_string(),
                volume_id: Some(format!("smb-{RAW_NAME}-0123456789abcdef")),
                volume_name: Some(RAW_NAME.to_string()),
                backend: Some(ReportBackend::Smb),
                connection: Some(ReportConnectionState::NeedsSignIn),
                view: Some(PaneView::Full),
                sort_field: Some(PaneSortField::Size),
                sort_order: Some(PaneSortOrder::Descending),
                total_files: 91,
                loaded_count: 17,
                cursor_index: 13,
                cursor: Some(RawEntryIdentity {
                    name: RAW_NAME.to_string(),
                    path: RAW_PATH.to_string(),
                    role: EntryRole::File,
                }),
                selected_count: 5,
                selected_files: 2,
                selected_folders: 3,
                tab_count: 4,
            }],
            operations: vec![RawOperationSnapshot {
                operation_id: Some(OPERATION_ID.to_string()),
                operation_type: WriteOperationType::Copy,
                lifecycle: LifecycleStatus::Paused,
                phase: Some(WriteOperationPhase::Copying),
                source: Some(RAW_PATH.to_string()),
                destination: Some(format!("smb://{RAW_NAME}/share/out")),
                current_file: Some(RAW_NAME.to_string()),
                files_done: 7,
                files_total: 19,
                bytes_done: 11,
                bytes_total: 23,
            }],
            recent_listing_error_count: 5,
        }
    }

    #[test]
    fn history_throttles_at_thirty_seconds_caps_at_eight_and_stays_oldest_first() {
        let mut history = StateHistory::default();
        let start = Instant::now();
        for i in 0..10 {
            let at = start + Duration::from_secs(i * 30);
            assert!(history.record(at, raw(i)), "the exact 30-second boundary is due");
            assert!(!history.record(at + Duration::from_secs(29), raw(100 + i)));
        }

        let snapshots = history.snapshot();
        assert_eq!(snapshots.len(), 8);
        assert_eq!(
            snapshots.iter().map(|s| s.sequence).collect::<Vec<_>>(),
            (2..10).collect::<Vec<_>>()
        );
    }

    #[test]
    fn report_transform_correlates_within_one_report_separates_reports_and_keeps_typed_facts() {
        let first = RedactionContext::for_test([0x11; 32], "ERR-FIRST");
        let second = RedactionContext::for_test([0x11; 32], "ERR-SECOND");
        let first_report = redact_snapshots(&[raw(1), raw(2)], &first);
        let second_report = redact_snapshots(&[raw(1)], &second);

        let first_json = serde_json::to_string(&first_report).unwrap();
        let second_json = serde_json::to_string(&second_report).unwrap();
        assert!(!first_json.contains(RAW_NAME));
        assert!(!first_json.contains(RAW_PATH));
        assert!(!second_json.contains(RAW_NAME));
        assert!(!second_json.contains(RAW_PATH));
        assert_eq!(first_report[0].panes[0].path, first_report[1].panes[0].path);
        assert_ne!(first_report[0].panes[0].path, second_report[0].panes[0].path);

        let pane = &first_report[0].panes[0];
        assert_eq!(pane.backend, Some(ReportBackend::Smb));
        assert_eq!(pane.connection, Some(ReportConnectionState::NeedsSignIn));
        assert_eq!(pane.selected_files, 2);
        assert_eq!(pane.selected_folders, 3);
        let operation = &first_report[0].operations[0];
        assert_eq!(operation.operation_id.as_deref(), Some(OPERATION_ID));
        assert_eq!(operation.lifecycle, LifecycleStatus::Paused);
        assert_eq!(operation.files_done, 7);
        assert_eq!(operation.bytes_total, 23);
    }

    #[test]
    fn unknown_strings_and_non_random_operation_ids_fail_closed() {
        assert_eq!(PaneView::parse("PRIVATE VIEW"), None);
        assert_eq!(PaneSortField::parse("PRIVATE SORT"), None);
        assert_eq!(PaneSortOrder::parse("PRIVATE ORDER"), None);
        assert_eq!(PaneSide::parse("PRIVATE SIDE"), None);
        assert_eq!(keep_operation_id("copy-PRIVATE-NAME-ASYMMETRIC"), None);
        assert_eq!(keep_operation_id(OPERATION_ID).as_deref(), Some(OPERATION_ID));
    }

    #[test]
    fn a_new_process_history_starts_empty_instead_of_loading_a_previous_session() {
        let mut old_process = StateHistory::default();
        assert!(old_process.record(Instant::now(), raw(1)));
        assert_eq!(old_process.snapshot().len(), 1);

        let new_process = StateHistory::default();
        assert!(new_process.snapshot().is_empty());
    }
}
