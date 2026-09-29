//! Historical persisted-log delivery policy.
//!
//! Groups timestamp-headed records with their continuation lines, drops complete records
//! from known-unsafe historical producer templates, and redacts every retained line.

use super::tail_walker;
use crate::redact;
use chrono::{DateTime, Utc};

struct LogRecordHeader<'a> {
    target: &'a str,
    message: &'a str,
}

/// Parse the file logger's `<iso-ts> LEVEL target  message` header. Untimestamped lines
/// are continuations and deliberately return `None`.
fn parse_log_record_header(line: &str) -> Option<LogRecordHeader<'_>> {
    tail_walker::parse_leading_iso8601(line)?;
    let after_timestamp = line.get(30..)?;
    // The level is a five-column field followed by one separator. `INFO` therefore has
    // two spaces before the target; `DEBUG` has one.
    let mut target_and_message = after_timestamp.get(6..)?;
    // The optional RAM gauge predates some retained logs and sits before the target.
    if target_and_message.starts_with('(') {
        target_and_message = target_and_message.split_once(") ")?.1;
    }
    let (target, message) = target_and_message.split_once("  ")?;
    Some(LogRecordHeader { target, message })
}

/// Historical logs are data at this boundary. These prefixes are fixed templates Cmdr
/// itself emitted before `5c2de4dec`; they do not classify an error or drive app control
/// flow. A whole matching record is withheld because its interpolated backend/OS/CLI text
/// could contain arbitrary user data that lexical redaction cannot prove safe.
fn has_legacy_prefix(message: &str, prefixes: &[&str]) -> bool {
    // allowed-error-string-match: recognizes Cmdr-owned historical log templates for privacy filtering, not external/localized error state; production ZIP tests pin every retained/dropped boundary
    prefixes.iter().any(|prefix| message.starts_with(prefix))
}

/// The current search summary starts with producer-owned structural fields. The old
/// template started with a quoted literal query (or a filter summary), so user text can
/// contain these markers but cannot occupy the producer-owned first field.
fn is_current_search_diagnostic(message: &str, prefix: &str, outcome_fields: &[&str]) -> bool {
    let Some(mut rest) = message.strip_prefix(prefix) else {
        return false;
    };
    for field in [
        ", size=",
        ", modified=",
        ", type=",
        ", case=",
        ", count-only=",
        ", scope=",
        ", exclusions=",
        ", system-exclusions=",
        " → ",
    ] {
        let Some((_, after)) = rest.split_once(field) else {
            return false;
        };
        rest = after;
    }
    outcome_fields.iter().all(|field| {
        let Some((_, after)) = rest.split_once(field) else {
            return false;
        };
        rest = after;
        true
    })
}

fn is_current_discovery_cache_diagnostic(message: &str) -> bool {
    [
        "Host ADDED: serverId=",
        "Host UPDATED: serverId=",
        "Host RESOLVED before FOUND, creating entry: serverId=",
        "Host RESOLVED: serverId=",
        "Host REMOVED: serverId=",
    ]
    .iter()
    // allowed-error-string-match: recognizes producer-owned field names at the start of current discovery templates; old identity values cannot spoof that position
    .any(|prefix| message.starts_with(prefix))
}

fn is_known_unsafe_historical_record(target: &str, message: &str) -> bool {
    if target == "error_reporter::state_snapshot" {
        // The removed target wrote `State at error time:` followed by arbitrary MCP YAML,
        // or an unavailable-message carrying arbitrary failure prose. Nothing emits it now.
        return true;
    }

    match target {
        "search::engine" => {
            if has_legacy_prefix(message, &["Search completed: "]) {
                !is_current_search_diagnostic(
                    message,
                    "Search completed: pattern=",
                    &[" matches (returning ", " hidden), took "],
                )
            } else if has_legacy_prefix(message, &["Count-only search: "]) {
                !is_current_search_diagnostic(
                    message,
                    "Count-only search: pattern=",
                    &[" matches (", " hidden), took "],
                )
            } else {
                false
            }
        }
        "network::discovery_cache" => !is_current_discovery_cache_diagnostic(message),
        "volume" => {
            has_legacy_prefix(
                message,
                &[
                    "no stored credentials for ",
                    "the secret store wouldn't keep the credentials for ",
                    "the secret store wouldn't remember this server (",
                    "couldn't write the trusted SSH host keys: ",
                ],
            ) || (has_legacy_prefix(message, &["couldn't write "])
                && message.find(": source=os, error_kind=").is_none())
        }
        "network::mdns_discovery" => has_legacy_prefix(
            message,
            &[
                "Failed to create mDNS daemon: ",
                "Failed to start mDNS browse: ",
                "Couldn't spawn the mDNS event thread: ",
                "mDNS unhandled event: ",
            ],
        ),
        "network::manual_servers" => {
            has_legacy_prefix(message, &["Unreachable: "]) && message.find(", source=os, error_kind=").is_none()
        }
        "network::mount" => has_legacy_prefix(message, &["Failed to unmount ", "Failed to run diskutil unmount for "]),
        "network::mount_linux" => has_legacy_prefix(message, &["gio mount of "]),
        "network::smb_client" => has_legacy_prefix(
            message,
            &[
                "smb2 list_shares failed ",
                "Guest failed with auth error: ",
                "smbutil with Keychain failed: ",
                "Guest failed with non-auth error: ",
                "smb2 authenticated list failed: ",
                "smbclient with auth also failed: ",
            ],
        ),
        "network::smb_smbclient" => {
            has_legacy_prefix(message, &["smbclient -L //"])
                && message.find(". stderr: ").is_some()
                && message.find(" | stdout: ").is_some()
        }
        "network::smb_smbutil" => has_legacy_prefix(message, &["smbutil failed: exit="]),
        "network::smb_upgrade" => has_legacy_prefix(message, &["Direct connect didn't reach the server ("]),
        "cmdr_sftp::errors" => {
            has_legacy_prefix(message, &["SFTP "])
                && (message.find(": the server reports no such file (").is_some()
                    || message.find(": the server refused access (").is_some())
        }
        "cmdr_sftp::volume::writes" => has_legacy_prefix(
            message,
            &["SftpVolume::write_from_stream: couldn't remove the partial "],
        ),
        "cmdr_smb::volume::scan" => {
            has_legacy_prefix(message, &["SmbVolume::scan_for_copy_batch(share="])
                && message.find("): backend=smb2, error_kind=").is_none()
        }
        "cmdr_smb::volume::scan_pool" => {
            has_legacy_prefix(message, &["smb scan pool: extra session "])
                && message.find(" failed to open (").is_some()
        }
        "cmdr_smb::volume::session" => {
            has_legacy_prefix(message, &["SmbVolume::"])
                && message.find("(share=").is_some()
                && message.find("backend=smb2, error_kind=").is_none()
        }
        "cmdr_smb::volume::watcher" => has_legacy_prefix(message, &["smb_watcher("]),
        "cmdr_webdav::errors" => {
            has_legacy_prefix(message, &["WebDAV "]) && !has_legacy_prefix(message, &["WebDAV path="])
        }
        "crash_reporter::panic" => has_legacy_prefix(message, &["Panic on thread `"]),
        "crash_reporter" => has_legacy_prefix(message, &["Contained a panic inside a foreign parser on thread "]),
        _ => false,
    }
}

/// Group physical lines into timestamp-headed records, then apply the delivery policy to
/// the whole record. Both ZIP builders converge here before lexical redaction.
pub(super) fn filter_and_redact_log_records(
    lines: impl IntoIterator<Item = String>,
    lower_bound: Option<DateTime<Utc>>,
    redaction: &redact::RedactionContext,
) -> Vec<String> {
    let mut output = Vec::new();
    let mut record = Vec::new();

    for line in lines {
        if tail_walker::parse_leading_iso8601(&line).is_some() && !record.is_empty() {
            append_safe_record(&mut output, &mut record, lower_bound, redaction);
        }
        record.push(line);
    }
    append_safe_record(&mut output, &mut record, lower_bound, redaction);
    output
}

fn append_safe_record(
    output: &mut Vec<String>,
    record: &mut Vec<String>,
    lower_bound: Option<DateTime<Utc>>,
    redaction: &redact::RedactionContext,
) {
    let first_line = record.first().map(String::as_str);
    let in_window = first_line
        .and_then(tail_walker::parse_leading_iso8601)
        .is_none_or(|timestamp| lower_bound.is_none_or(|cutoff| timestamp >= cutoff));
    let safe_shape = first_line
        .and_then(parse_log_record_header)
        .is_none_or(|header| !is_known_unsafe_historical_record(header.target, header.message));
    let keep = in_window && safe_shape;
    if keep {
        output.extend(record.drain(..).map(|line| redaction.redact_line(&line).into_owned()));
    } else {
        record.clear();
    }
}
