//! The one-time bridge from Ask Cmdr's old opt-in to its plain on/off switch (`askCmdr.enabled`).
//!
//! Ask Cmdr used to have its own consent screen; cloud consent now lives in `ai::cloud_consent`,
//! and Ask Cmdr is a feature switch. The frontend's startup step asks this command once, while
//! `askCmdr.enabled` has never been set explicitly, and writes the switch from the answer:
//! somebody who accepted Ask Cmdr's opt-in (any version) keeps it on, somebody who never did (or
//! whose "no" is still held) gets it off. The legacy record grants nothing else.

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::agent::AgentDb;
use crate::agent::store::{self, AgentStoreError, ConsentRecord, ConsentRecordView};

const LOG_TARGET: &str = "agent::ipc";

/// What the legacy Ask Cmdr opt-in says, for the one-time `askCmdr.enabled` mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum LegacyAskCmdrOptIn {
    /// The user accepted Ask Cmdr's opt-in (any copy version) and no "no" is held: map to on.
    Recorded,
    /// Never accepted, or a "no" is still held for the store: map to off.
    NotRecorded,
    /// The store couldn't be read: write nothing, and ask again next launch.
    StoreUnavailable,
}

/// Whether the user once opted into Ask Cmdr, read from the legacy record in `main.db` and the
/// legacy held-"no" marker (`askCmdr.consentRevokePending`).
#[tauri::command]
#[specta::specta]
pub async fn ask_cmdr_legacy_opt_in(app: AppHandle) -> LegacyAskCmdrOptIn {
    let held = crate::settings::load_ask_cmdr_consent_revoke_pending(&app);
    let Some(db_path) = app.try_state::<AgentDb>().map(|db| db.db_path().to_path_buf()) else {
        return LegacyAskCmdrOptIn::StoreUnavailable;
    };
    let record = tauri::async_runtime::spawn_blocking(move || {
        let conn = store::open_read_connection(&db_path)?;
        store::get_consent(&conn, ConsentRecord::AskCmdrLegacy)
    })
    .await;
    let record = match record {
        Ok(record) => record,
        Err(e) => {
            log::warn!(target: LOG_TARGET, "reading the legacy Ask Cmdr opt-in didn't finish: {e}");
            return LegacyAskCmdrOptIn::StoreUnavailable;
        }
    };
    if let Err(e) = &record {
        log::warn!(target: LOG_TARGET, "reading the legacy Ask Cmdr opt-in failed: {e}");
    }
    legacy_opt_in(record, held)
}

/// The pure decision: an unreadable store is `StoreUnavailable`; a held "no" beats any record;
/// any recorded version (a stale copy included) counts as having opted in.
fn legacy_opt_in(record: Result<Option<ConsentRecordView>, AgentStoreError>, held: bool) -> LegacyAskCmdrOptIn {
    match record {
        Err(_) => LegacyAskCmdrOptIn::StoreUnavailable,
        Ok(Some(_)) if !held => LegacyAskCmdrOptIn::Recorded,
        Ok(_) => LegacyAskCmdrOptIn::NotRecorded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(version: u32) -> Result<Option<ConsentRecordView>, AgentStoreError> {
        Ok(Some(ConsentRecordView {
            version,
            at: 1_780_000_000,
        }))
    }

    #[test]
    fn a_current_opt_in_maps_to_on() {
        assert_eq!(legacy_opt_in(record(4), false), LegacyAskCmdrOptIn::Recorded);
    }

    /// They asked for Ask Cmdr, whatever wording they accepted it under.
    #[test]
    fn a_stale_opt_in_maps_to_on_too() {
        assert_eq!(legacy_opt_in(record(1), false), LegacyAskCmdrOptIn::Recorded);
    }

    /// A "no" the store refused to record still wins over the record it couldn't clear.
    #[test]
    fn a_held_no_maps_to_off_over_a_record() {
        assert_eq!(legacy_opt_in(record(4), true), LegacyAskCmdrOptIn::NotRecorded);
    }

    #[test]
    fn never_opting_in_maps_to_off() {
        assert_eq!(legacy_opt_in(Ok(None), false), LegacyAskCmdrOptIn::NotRecorded);
    }

    /// Nothing is written on doubt: the frontend retries next launch.
    #[test]
    fn an_unreadable_store_writes_nothing() {
        let unreadable = Err(AgentStoreError::Io(std::io::Error::other("locked")));
        assert_eq!(legacy_opt_in(unreadable, false), LegacyAskCmdrOptIn::StoreUnavailable);
    }
}
