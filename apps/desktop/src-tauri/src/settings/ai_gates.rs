//! The switches the AI gates read fresh from `settings.json` on every check: Ask Cmdr's on/off
//! and the two held-"no" markers. Each is a plain boolean where only a real JSON `true` counts,
//! so an absent key, a hand-edited string, or an unreadable file all read as the closed answer.
//!
//! Read fresh (not from the startup `Settings`) because each one closes a gate the moment it's
//! written: the send path reads it per send, and the wake readiness is refreshed by an explicit
//! push (`ask_cmdr_enabled_changed`, `cloud_ai_consent_revoke_pending_changed`).

use std::fs;

/// Whether Ask Cmdr is switched on (`askCmdr.enabled`), read by the send gate and the wake
/// readiness. An absent key reads as OFF (fail quiet): the registry default is `false`, and every
/// user who should have it on gets it written explicitly (onboarding, or the one-time mapping
/// from the legacy Ask Cmdr opt-in).
pub fn load_ask_cmdr_enabled<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    load_true_flag(app, parse_ask_cmdr_enabled)
}

fn parse_ask_cmdr_enabled(contents: &str) -> bool {
    parse_true_flag(contents, "askCmdr.enabled")
}

/// Whether a "no" to Ask Cmdr is held for a `main.db` that refused to record it
/// (`askCmdr.consentRevokePending`), from before Ask Cmdr's opt-in became a plain switch. Its one
/// reader is the one-time mapping onto `askCmdr.enabled` (`ask_cmdr_legacy_opt_in`): a held "no"
/// maps to off.
pub fn load_ask_cmdr_consent_revoke_pending<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    load_true_flag(app, parse_ask_cmdr_consent_revoke_pending)
}

fn parse_ask_cmdr_consent_revoke_pending(contents: &str) -> bool {
    parse_true_flag(contents, "askCmdr.consentRevokePending")
}

/// Whether a "no" to cloud AI is held for a `main.db` that refused to record it
/// (`ai.cloudConsentRevokePending`). `ai::cloud_consent::RevokePending::load` is its one reader,
/// which is how every cloud gate sees it. The marker only exists on the path where a revoke was
/// refused; the store record stays the answer everywhere else.
pub fn load_cloud_consent_revoke_pending<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    load_true_flag(app, parse_cloud_consent_revoke_pending)
}

fn parse_cloud_consent_revoke_pending(contents: &str) -> bool {
    parse_true_flag(contents, "ai.cloudConsentRevokePending")
}

/// Read `settings.json` fresh and apply `parse`. An unreadable data dir or file reads as `false`.
fn load_true_flag<R: tauri::Runtime>(app: &tauri::AppHandle<R>, parse: fn(&str) -> bool) -> bool {
    let Ok(data_dir) = crate::config::resolved_app_data_dir(app) else {
        return false;
    };
    let Ok(contents) = fs::read_to_string(data_dir.join("settings.json")) else {
        return false;
    };
    parse(&contents)
}

/// Whether `key` holds a real JSON `true`.
fn parse_true_flag(contents: &str, key: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(contents)
        .ok()
        .and_then(|json| json.get(key).and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The legacy held "no" maps Ask Cmdr off, so only the value the frontend wrote (a JSON `true`)
    /// counts. Anything else, including a stringly `"true"` from a hand edit, leaves the store record
    /// as the answer.
    #[test]
    fn a_held_consent_revoke_reads_only_from_a_real_true() {
        assert!(parse_ask_cmdr_consent_revoke_pending(
            r#"{ "askCmdr.consentRevokePending": true }"#
        ));
        for contents in [
            "{}",
            r#"{ "askCmdr.consentRevokePending": false }"#,
            r#"{ "askCmdr.consentRevokePending": "true" }"#,
            "not json at all",
        ] {
            assert!(
                !parse_ask_cmdr_consent_revoke_pending(contents),
                "{contents} must not read as a held revoke"
            );
        }
    }

    /// Ask Cmdr's switch opens the send gate and the proactive loop, so only a real JSON `true`
    /// turns it on. An absent key reads as off (fail quiet): the one-time mapping and onboarding
    /// write it explicitly for everyone who should have it on.
    #[test]
    fn ask_cmdr_is_on_only_for_a_real_true() {
        assert!(parse_ask_cmdr_enabled(r#"{ "askCmdr.enabled": true }"#));
        for contents in [
            "{}",
            r#"{ "askCmdr.enabled": false }"#,
            r#"{ "askCmdr.enabled": "true" }"#,
            r#"{ "askCmdr.enabled": 1 }"#,
            "not json at all",
        ] {
            assert!(!parse_ask_cmdr_enabled(contents), "{contents} must read as off");
        }
    }

    /// A held "no" to cloud AI closes every cloud gate, so only the value the frontend writes (a
    /// JSON `true`) may hold one. Anything else leaves the store record as the answer.
    #[test]
    fn a_held_cloud_consent_revoke_reads_only_from_a_real_true() {
        assert!(parse_cloud_consent_revoke_pending(
            r#"{ "ai.cloudConsentRevokePending": true }"#
        ));
        for contents in [
            "{}",
            r#"{ "ai.cloudConsentRevokePending": false }"#,
            r#"{ "ai.cloudConsentRevokePending": "true" }"#,
            // The legacy Ask Cmdr marker is a different answer to a different question.
            r#"{ "askCmdr.consentRevokePending": true }"#,
            "not json at all",
        ] {
            assert!(
                !parse_cloud_consent_revoke_pending(contents),
                "{contents} must not read as a held cloud revoke"
            );
        }
    }
}
