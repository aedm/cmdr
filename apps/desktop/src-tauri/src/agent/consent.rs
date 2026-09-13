//! The consent gate: Ask Cmdr's privacy line made STRUCTURAL, not just a UI affordance.
//!
//! The rail's frontend gate is the UX layer; this is the backend enforcement. Every send
//! (`commands::agent::ask_cmdr_send_message`) checks [`has_current_consent`] before it
//! creates a thread or resolves the LLM, so nothing reaches a provider without a recorded,
//! current opt-in — the claim `agent/CLAUDE.md` and the consent screen make. The wake loop's
//! readiness (`wake::snapshot`) reads the same function, and so does the status the rail and
//! Settings show.

use rusqlite::Connection;
use tauri::{AppHandle, Runtime};

use crate::agent::store;

/// The consent-copy version the user must have accepted for the gate to open. **Bump this
/// whenever the `askCmdr.consent.*` copy changes materially**, so a stale acceptance no
/// longer counts and users re-consent to the new wording. The copy itself lives in the
/// frontend catalog; this integer is its machine-checkable version, recorded in `main.db`.
pub const CONSENT_COPY_VERSION: u32 = 4;

/// Whether a "no" to Ask Cmdr is still waiting to reach `main.db`.
///
/// Onboarding's "no AI" pick revokes consent in `main.db`. When the store refuses that write
/// twice, the frontend holds the answer in `settings.json` (`askCmdr.consentRevokePending`)
/// and retries until the store takes it. It lives there because `main.db` is the store that
/// just refused. ⚠️ **Every consent check takes it**, so the "no" holds on the very next check
/// and the retry only makes the store catch up: that's why it's an argument of
/// [`has_current_consent`] rather than something a caller may remember to check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevokePending {
    No,
    Yes,
}

impl RevokePending {
    /// Read fresh from `settings.json`. An absent or unreadable marker reads as `No`, since the
    /// store record is then the only answer there is; the marker only ever exists on the path
    /// where a revoke was refused, and a failing settings read must not shut Ask Cmdr for
    /// everyone else.
    pub fn load<R: Runtime>(app: &AppHandle<R>) -> Self {
        if crate::settings::load_ask_cmdr_consent_revoke_pending(app) {
            Self::Yes
        } else {
            Self::No
        }
    }
}

/// Whether the user has accepted the CURRENT consent copy. Fails CLOSED: an absent record,
/// a stale version, an unreadable store, or a "no" still held for the store all read as "not
/// consented", so a send is refused rather than proceeding on doubt.
pub fn has_current_consent(conn: &Connection, revoke: RevokePending) -> bool {
    revoke == RevokePending::No
        && matches!(store::get_consent(conn), Ok(Some(consent)) if consent.version == CONSENT_COPY_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrated_conn() -> Connection {
        let conn = crate::sqlite_util::open_in_memory().expect("in-memory db");
        store::run_migrations(&conn, store::MIGRATIONS).expect("migrate");
        conn
    }

    #[test]
    fn no_record_is_not_consented() {
        let conn = migrated_conn();
        assert!(
            !has_current_consent(&conn, RevokePending::No),
            "a fresh DB with no consent record ⇒ gate closed"
        );
    }

    #[test]
    fn a_stale_copy_version_is_not_consented() {
        let conn = migrated_conn();
        // An older accepted version no longer counts once the copy (and the constant) moved on.
        store::set_consent(&conn, CONSENT_COPY_VERSION.wrapping_sub(1), 1_780_000_000).expect("set");
        assert!(
            !has_current_consent(&conn, RevokePending::No),
            "a stale copy version ⇒ gate closed"
        );
    }

    #[test]
    fn the_current_copy_version_is_consented() {
        let conn = migrated_conn();
        store::set_consent(&conn, CONSENT_COPY_VERSION, 1_780_000_000).expect("set");
        assert!(
            has_current_consent(&conn, RevokePending::No),
            "accepting the current copy ⇒ gate open"
        );
    }

    #[test]
    fn a_held_revoke_closes_the_gate_over_a_consent_the_store_still_records() {
        // The store refused the revoke, so the record is still there; the held "no" wins.
        let conn = migrated_conn();
        store::set_consent(&conn, CONSENT_COPY_VERSION, 1_780_000_000).expect("set");
        assert!(
            !has_current_consent(&conn, RevokePending::Yes),
            "a revoke held for the store ⇒ gate closed, whatever the store still says"
        );
    }
}
