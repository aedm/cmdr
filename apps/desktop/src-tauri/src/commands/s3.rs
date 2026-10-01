//! The IPC surface for S3 accounts that the protocol-agnostic
//! `commands::servers` facade has no reason to widen: the secret, and whether a
//! mounted place can come back on its own.
//!
//! Pass-throughs. The connect flow lives in `network::s3_volume_wiring`, the
//! place list in `network::s3_known_places`, and the secret store in
//! `network::keychain`.
//!
//! ❗ **The secret is the ACCOUNT's**: one entry per endpoint plus access key id
//! (`S3ConnectionParams::credential_service`, scoped by the key id), which every
//! bucket place under that key reads.

use serde::{Deserialize, Serialize};

use crate::network::keychain::{self, KeychainError};
use crate::network::s3_known_places::S3ProviderChoice;
use crate::network::s3_volume_wiring;
use cmdr_s3::UnattendedReconnect;

/// Whether an S3 volume can actually come back on its own as it stands. The
/// WebDAV twin (`WebdavUnattendedReconnect`), for the same reasons: ❌ never
/// derive it in the frontend from a credential check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum S3UnattendedReconnect {
    /// On, and it works.
    Possible,
    /// The switch is off.
    SwitchOff,
    /// ❗ On, and nothing is stored to redial with: the state a UI warns about.
    NoStoredSecret,
}

impl From<UnattendedReconnect> for S3UnattendedReconnect {
    fn from(answer: UnattendedReconnect) -> Self {
        match answer {
            UnattendedReconnect::Possible => Self::Possible,
            UnattendedReconnect::SwitchOff => Self::SwitchOff,
            UnattendedReconnect::NoStoredSecret => Self::NoStoredSecret,
        }
    }
}

/// The answer when the provider can't make an endpoint, so there's no key to
/// file a secret under. ❗ `Other`, ❌ not `AccessDenied`: nobody said no.
fn not_an_account() -> KeychainError {
    KeychainError::Other("the provider doesn't make an endpoint".to_string())
}

/// The answer when the secret store didn't come back in time.
fn keychain_timed_out() -> KeychainError {
    KeychainError::Other("the secret store didn't answer".to_string())
}

/// Saves the secret access key for one account.
///
/// ❗ **This command IS the "remember the secret" switch**, the WebDAV twin's
/// contract: `has_s3_credentials` reads it back, `delete_s3_credentials` turns
/// it off, and there's no second flag anywhere. On a blocking task: the store
/// can put a Keychain prompt in front of it.
#[tauri::command]
#[specta::specta]
pub async fn save_s3_credentials(
    provider: S3ProviderChoice,
    access_key_id: String,
    secret: String,
) -> Result<(), KeychainError> {
    let access_key_id = access_key_id.trim().to_string();
    let Some(service) = s3_volume_wiring::credential_service(&provider, &access_key_id) else {
        return Err(not_an_account());
    };
    crate::deadline::blocking_with_timeout(
        std::time::Duration::from_secs(15),
        Err(keychain_timed_out()),
        move || keychain::save_credentials(&service, Some(&access_key_id), &access_key_id, &secret),
    )
    .await
}

/// Whether a secret is stored for one account. ❗ No command hands the secret
/// itself to the frontend. A store that didn't answer in time reads as `false`.
#[tauri::command]
#[specta::specta]
pub async fn has_s3_credentials(provider: S3ProviderChoice, access_key_id: String) -> bool {
    let access_key_id = access_key_id.trim().to_string();
    let Some(service) = s3_volume_wiring::credential_service(&provider, &access_key_id) else {
        return false;
    };
    crate::deadline::blocking_with_timeout(std::time::Duration::from_secs(15), false, move || {
        keychain::has_credentials(&service, Some(&access_key_id))
    })
    .await
}

/// Forgets the stored secret for one account, and so for every place under it.
#[tauri::command]
#[specta::specta]
pub async fn delete_s3_credentials(provider: S3ProviderChoice, access_key_id: String) -> Result<(), KeychainError> {
    let access_key_id = access_key_id.trim().to_string();
    let Some(service) = s3_volume_wiring::credential_service(&provider, &access_key_id) else {
        return Err(not_an_account());
    };
    crate::deadline::blocking_with_timeout(
        std::time::Duration::from_secs(15),
        Err(keychain_timed_out()),
        move || keychain::delete_credentials(&service, Some(&access_key_id)),
    )
    .await
}

/// Whether an S3 volume can come back on its own as it stands. `null` when
/// nothing S3 is registered under that id. ❗ Reads the store: ask when a
/// banner renders, ❌ never poll.
#[tauri::command]
#[specta::specta]
pub async fn get_s3_unattended_reconnect(volume_id: String) -> Option<S3UnattendedReconnect> {
    s3_volume_wiring::unattended_reconnect(&volume_id)
        .await
        .map(S3UnattendedReconnect::from)
}
