//! Breadcrumb ring buffer for error report triage.
//!
//! Records a rolling window of recent FE/BE events so triagers can see what led up
//! to an error. Sentry-style, but kept tiny: bounded queue, fire-and-forget recording,
//! shipped as part of the error report bundle's manifest.
//!
//! ## Why not just use logs?
//!
//! Logs are noisy and unstructured. Breadcrumbs are a closed set of curated events.
//! The `Command` event is the "what did the user just do" signal:
//! `handleCommandExecute` pushes one on every dispatch, and the most recent such
//! entry is what triagers read first.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;

/// Maximum number of breadcrumbs retained. Once exceeded, the oldest is dropped.
/// Sized so the bundle stays small (each entry is ~100-300 bytes serialized) but
/// covers a meaningful window of activity (typically a few minutes of normal use).
pub const MAX_BREADCRUMBS: usize = 50;

/// Cap on the only retained string payload. Command ids are compile-time labels in
/// the frontend command registry, not user-authored text.
pub const MAX_COMMAND_ID_CHARS: usize = 128;

/// The frontend registry is authoritative. `build.rs` compiles its tuple into
/// this slice so this privacy boundary cannot drift onto a hand-maintained copy.
const COMMAND_IDS: &[&str] = include!(concat!(env!("OUT_DIR"), "/command_ids.rs"));

/// Diagnostic-safe event facts accepted by the breadcrumb buffer.
///
/// `deny_unknown_fields` is the fail-closed IPC boundary: adding a JSON key or sending
/// the former free-form `kind` / `message` / `ctx` shape rejects the whole event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum BreadcrumbEvent {
    Command { command_id: String },
    ErrorReportDialogOpened { has_initial_note: bool },
    ErrorReportAmendDialogOpened,
    ErrorReportDialogClosed,
    FeedbackDialogOpened,
    FeedbackDialogClosed,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Breadcrumb {
    /// ISO-8601 UTC timestamp.
    pub at: String,
    pub event: BreadcrumbEvent,
}

static BUFFER: Mutex<VecDeque<Breadcrumb>> = Mutex::new(VecDeque::new());

fn is_authoritative_command_id(command_id: &str) -> bool {
    command_id.chars().count() <= MAX_COMMAND_ID_CHARS && COMMAND_IDS.contains(&command_id)
}

/// Append a breadcrumb. Drops the oldest entry if the buffer is full.
///
/// Silent on overflow / lock poisoning. Breadcrumbs are best-effort instrumentation,
/// not a feature we'd ever surface a failure for.
pub fn record(event: BreadcrumbEvent) {
    match &event {
        BreadcrumbEvent::Command { command_id } if !is_authoritative_command_id(command_id) => {
            return;
        }
        _ => {}
    }
    let crumb = Breadcrumb {
        at: Utc::now().to_rfc3339(),
        event,
    };
    let Ok(mut guard) = BUFFER.lock() else {
        return;
    };
    if guard.len() >= MAX_BREADCRUMBS {
        guard.pop_front();
    }
    guard.push_back(crumb);
}

/// Snapshot of the current ring buffer, oldest first. Used at bundle-build time.
pub fn snapshot() -> Vec<Breadcrumb> {
    let Ok(guard) = BUFFER.lock() else {
        return Vec::new();
    };
    guard.iter().cloned().collect()
}

#[cfg(test)]
pub fn reset_for_test() {
    if let Ok(mut g) = BUFFER.lock() {
        g.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serializes tests in this module so they don't race on the global BUFFER
    // when nextest runs them in parallel.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn records_only_typed_command_and_boolean_facts() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset_for_test();
        record(BreadcrumbEvent::Command {
            command_id: "pane.switch".to_string(),
        });
        record(BreadcrumbEvent::ErrorReportDialogOpened { has_initial_note: true });

        let snap = snapshot();
        let serialized = serde_json::to_value(&snap).expect("typed breadcrumbs serialize");

        assert_eq!(
            serialized[0]["event"],
            serde_json::json!({ "type": "command", "commandId": "pane.switch" })
        );
        assert_eq!(
            serialized[1]["event"],
            serde_json::json!({ "type": "errorReportDialogOpened", "hasInitialNote": true })
        );
        assert!(serialized[0]["at"].as_str().is_some());
        assert!(serialized[1]["at"].as_str().is_some());
    }

    #[test]
    fn rejects_unknown_fields_and_nested_values_instead_of_serializing_them() {
        let sentinels = [
            serde_json::json!({
                "type": "command",
                "commandId": "pane.switch",
                "PRIVATE_KEY_SENTINEL": { "nested": ["PRIVATE_VALUE_SENTINEL"] }
            }),
            serde_json::json!({
                "type": "errorReportDialogOpened",
                "hasInitialNote": true,
                "PRIVATE_KEY_SENTINEL": "PRIVATE_VALUE_SENTINEL"
            }),
            serde_json::json!({
                "kind": "command",
                "message": "PRIVATE_MESSAGE_SENTINEL",
                "ctx": { "PRIVATE_KEY_SENTINEL": { "nested": "PRIVATE_VALUE_SENTINEL" } }
            }),
        ];

        for sentinel in sentinels {
            assert!(
                serde_json::from_value::<BreadcrumbEvent>(sentinel).is_err(),
                "open or unknown breadcrumb shapes must fail closed"
            );
        }
    }

    #[test]
    fn ring_buffer_drops_oldest_when_full() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset_for_test();
        for command_id in COMMAND_IDS.iter().take(MAX_BREADCRUMBS + 5) {
            record(BreadcrumbEvent::Command {
                command_id: (*command_id).to_string(),
            });
        }
        let snap = snapshot();
        assert_eq!(snap.len(), MAX_BREADCRUMBS);
        assert_eq!(
            snap[0].event,
            BreadcrumbEvent::Command {
                command_id: COMMAND_IDS[5].to_string()
            }
        );
        assert_eq!(
            snap.last().expect("snapshot is non-empty").event,
            BreadcrumbEvent::Command {
                command_id: COMMAND_IDS[MAX_BREADCRUMBS + 4].to_string()
            }
        );
    }

    #[test]
    fn rejects_empty_oversized_and_unknown_command_ids() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset_for_test();
        record(BreadcrumbEvent::Command {
            command_id: String::new(),
        });
        record(BreadcrumbEvent::Command {
            command_id: "c".repeat(MAX_COMMAND_ID_CHARS + 1),
        });
        record(BreadcrumbEvent::Command {
            command_id: "pane.privateNote".to_string(),
        });
        assert!(snapshot().is_empty());
    }

    #[test]
    fn rust_validator_tracks_every_frontend_command_id_and_rejects_near_misses() {
        let authored: Vec<&str> = include_str!("../../../src/lib/commands/command-ids.ts")
            .split_once("export const COMMAND_IDS = [")
            .and_then(|(_, after_start)| after_start.split_once("] as const").map(|(body, _)| body))
            .expect("the frontend command-id tuple keeps its documented shape")
            .lines()
            .filter_map(|line| {
                line.trim()
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix("',"))
            })
            .collect();

        assert_eq!(
            COMMAND_IDS, authored,
            "the generated Rust vocabulary must match COMMAND_IDS in order"
        );
        for id in authored {
            assert!(
                is_authoritative_command_id(id),
                "Rust must accept frontend command id `{id}`"
            );
            let near_miss = format!("{id}.private");
            assert!(
                !is_authoritative_command_id(&near_miss),
                "Rust must reject a near miss of `{id}`"
            );
        }
        assert!(!is_authoritative_command_id("pane.switcH"));
        assert!(!is_authoritative_command_id("pane.switch "));
    }

    #[test]
    fn direct_ipc_command_records_a_registry_id_but_drops_a_short_private_string() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset_for_test();
        for command_id in ["pane.switch", "private medical note"] {
            let event = serde_json::from_value(serde_json::json!({
                "type": "command",
                "commandId": command_id
            }))
            .expect("the closed enum still deserializes the command variant at the IPC edge");

            crate::commands::error_reporter::record_breadcrumb(event);
        }

        let recorded: Vec<BreadcrumbEvent> = snapshot().into_iter().map(|breadcrumb| breadcrumb.event).collect();
        assert_eq!(
            recorded,
            [BreadcrumbEvent::Command {
                command_id: "pane.switch".to_string()
            }],
            "unregistered text must never enter the breadcrumb buffer"
        );
    }
}
