//! Consent records in the `meta` table: which copy version the user accepted, and when.
//!
//! App state, not a preference (agent decision D56): `main.db` is durable, migrated,
//! transactional, and `sqlite3`-inspectable. A partial or absent record reads as no consent,
//! so every gate built on it fails CLOSED. The copy versions belong to the features that own
//! the copy (`crate::ai::cloud_consent::CLOUD_AI_CONSENT_VERSION`), not here.

use rusqlite::Connection;

use super::AgentStoreError;

/// Which consent record to read. Each is a pair of `meta` rows: the accepted copy version (as
/// text) and the unix-secs timestamp it was accepted at.
///
/// Only [`CloudAi`](Self::CloudAi) can be written ([`set_cloud_ai_consent`] /
/// [`clear_cloud_ai_consent`]); the legacy Ask Cmdr record is read-only by construction, since
/// no write fn takes this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentRecord {
    /// "Allow cloud AI": the one consent every cloud LLM call checks (`crate::ai::cloud_consent`).
    CloudAi,
    /// Ask Cmdr's old opt-in. Grants nothing; its only reader is the one-time mapping onto the
    /// `askCmdr.enabled` setting (`commands::agent::ask_cmdr_legacy_opt_in`).
    AskCmdrLegacy,
}

impl ConsentRecord {
    fn keys(self) -> (&'static str, &'static str) {
        match self {
            ConsentRecord::CloudAi => ("cloud_ai_consent_version", "cloud_ai_consent_at"),
            ConsentRecord::AskCmdrLegacy => ("ask_cmdr_consent_version", "ask_cmdr_consent_at"),
        }
    }
}

/// A recorded consent: which copy version the user accepted, and when. Stored in the
/// durable `main.db` (app state, not a preference — agent decision D56), so it's transactional
/// and `sqlite3`-inspectable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConsentRecordView {
    /// The copy version the user accepted. A copy change bumps the owning constant, so a
    /// stale-version record no longer counts as current consent.
    pub version: u32,
    /// Unix secs when consent was recorded.
    pub at: i64,
}

/// Read a `meta` value by key.
fn read_meta(conn: &Connection, key: &str) -> Result<Option<String>, AgentStoreError> {
    let mut stmt = conn.prepare_cached("SELECT value FROM meta WHERE key = ?1")?;
    let mut rows = stmt.query(rusqlite::params![key])?;
    match rows.next()? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

/// The recorded consent, or `None` if the user has never accepted (both keys must be
/// present and parseable). A partial/garbage record reads as no consent, so the gate
/// stays closed rather than silently proceeding.
pub fn get_consent(conn: &Connection, record: ConsentRecord) -> Result<Option<ConsentRecordView>, AgentStoreError> {
    let (version_key, at_key) = record.keys();
    let (Some(version_str), Some(at_str)) = (read_meta(conn, version_key)?, read_meta(conn, at_key)?) else {
        return Ok(None);
    };
    match (version_str.parse::<u32>(), at_str.parse::<i64>()) {
        (Ok(version), Ok(at)) => Ok(Some(ConsentRecordView { version, at })),
        _ => Ok(None),
    }
}

/// Record cloud AI consent for copy `version` at `now` (unix secs). Idempotent — re-accepting
/// overwrites the stored version + timestamp.
pub fn set_cloud_ai_consent(conn: &Connection, version: u32, now: i64) -> Result<(), AgentStoreError> {
    write_consent(conn, ConsentRecord::CloudAi, version, now)
}

fn write_consent(conn: &Connection, record: ConsentRecord, version: u32, now: i64) -> Result<(), AgentStoreError> {
    let (version_key, at_key) = record.keys();
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        rusqlite::params![version_key, version.to_string()],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        rusqlite::params![at_key, now.to_string()],
    )?;
    Ok(())
}

/// Clear any recorded cloud AI consent (the "Allow cloud AI" switch turned off). The legacy
/// Ask Cmdr record is left alone.
pub fn clear_cloud_ai_consent(conn: &Connection) -> Result<(), AgentStoreError> {
    let (version_key, at_key) = ConsentRecord::CloudAi.keys();
    conn.execute(
        "DELETE FROM meta WHERE key IN (?1, ?2)",
        rusqlite::params![version_key, at_key],
    )?;
    Ok(())
}

/// Record Ask Cmdr's own opt-in. Its only caller is the Ask Cmdr consent screen, which the
/// `askCmdr.enabled` switch replaces.
pub fn set_ask_cmdr_consent(conn: &Connection, version: u32, now: i64) -> Result<(), AgentStoreError> {
    write_consent(conn, ConsentRecord::AskCmdrLegacy, version, now)
}

/// Clear Ask Cmdr's own opt-in.
pub fn clear_ask_cmdr_consent(conn: &Connection) -> Result<(), AgentStoreError> {
    let (version_key, at_key) = ConsentRecord::AskCmdrLegacy.keys();
    conn.execute(
        "DELETE FROM meta WHERE key IN (?1, ?2)",
        rusqlite::params![version_key, at_key],
    )?;
    Ok(())
}

/// Seed the legacy Ask Cmdr record the way a pre-split build left it. Tests only: production
/// code can't write it.
#[cfg(test)]
pub(crate) fn set_legacy_ask_cmdr_consent_for_tests(conn: &Connection, version: u32, now: i64) {
    write_consent(conn, ConsentRecord::AskCmdrLegacy, version, now).expect("seed the legacy consent record");
}
