//! The S3 half of the servers facade: an account's saved places grouped into
//! one hub row, and the saved entry a target describes. Split out of `servers.rs` to keep that
//! file under its length cap; the facade's rationale stays there.

use super::wire::{SavedPlace, SavedServer, ServerNameSource, ServerProtocol};
use crate::network::s3_known_places;
use crate::network::saved_server_fields;

/// Every saved S3 place, grouped under its ACCOUNT: one [`SavedServer`] per
/// endpoint plus access key id, its places the saved buckets (and the root,
/// when saved), each with its own id and pin.
///
/// ❗ The account's id is its ROOT place's id (`s3_known_places::account_id`),
/// which names the account whether or not the root is saved. The account row
/// carries no pin of its own: its places do, the way an SMB host's shares do.
pub(super) fn s3_accounts(
    manager: &crate::file_system::volume::manager::VolumeManager,
    app_roots: &std::collections::HashMap<String, String>,
) -> Vec<SavedServer> {
    let mut accounts: Vec<SavedServer> = Vec::new();
    for entry in s3_known_places::all() {
        let Ok(params) = entry.params() else {
            log::warn!(target: "volume", "a saved S3 place's provider no longer makes an endpoint; leaving it out of the listing");
            continue;
        };
        let volume_id = s3_known_places::place_id(&params);
        let Some(app_root) = app_roots.get(&volume_id).cloned() else {
            log::warn!(target: "volume", "a saved S3 place has no place in the volume listing; leaving it out");
            continue;
        };
        let account_id = s3_known_places::account_id(&params);
        let place = SavedPlace {
            connected: manager.get(&volume_id).is_some(),
            volume_id,
            name: entry.label(),
            pinned: entry.pinned,
            app_root,
            username: Some(entry.access_key_id.clone()),
        };
        let is_root = entry.bucket.is_none();
        match accounts.iter_mut().find(|account| account.id == account_id) {
            Some(account) => {
                account.places.push(place);
                if account.last_connected_at.as_deref() < Some(entry.last_connected_at.as_str()) {
                    account.last_connected_at = Some(entry.last_connected_at.clone());
                }
                if is_root {
                    account.display_name = entry.label();
                    account.name_source = ServerNameSource::of_account(&entry.display_name);
                    account.auto_reconnect = Some(entry.auto_reconnect);
                }
            }
            None => accounts.push(SavedServer {
                id: account_id,
                protocol: ServerProtocol::S3,
                // The root's label when it's saved; else the account's stand-in.
                display_name: if is_root {
                    entry.label()
                } else {
                    saved_server_fields::server_label("", &entry.access_key_id, params.host())
                },
                name_source: if is_root {
                    ServerNameSource::of_account(&entry.display_name)
                } else {
                    ServerNameSource::Fallback
                },
                address: s3_address(&entry.provider, params.host()),
                username: Some(entry.access_key_id.clone()),
                pinned: false,
                last_connected_at: Some(entry.last_connected_at.clone()),
                auto_reconnect: Some(entry.auto_reconnect),
                places: vec![place],
            }),
        }
    }
    accounts
}

/// What the hub shows as an S3 account's address: the endpoint URL for "Other",
/// the endpoint host for a preset.
fn s3_address(provider: &s3_known_places::S3ProviderChoice, host: &str) -> String {
    match provider {
        s3_known_places::S3ProviderChoice::Other { endpoint, .. } => endpoint.trim().to_string(),
        _ => host.to_string(),
    }
}

/// The saved entry an S3 target describes, trimmed the way a dial trims it so
/// the two derive one id. ❗ `pinned: true` is only ever read for a NEW entry
/// (`s3_known_places::remember` keeps a stored pin).
pub(super) fn s3_place(
    provider: s3_known_places::S3ProviderChoice,
    access_key_id: String,
    bucket: Option<String>,
    display_name: String,
    auto_reconnect: bool,
) -> s3_known_places::KnownS3Place {
    s3_known_places::KnownS3Place {
        provider,
        access_key_id: access_key_id.trim().to_string(),
        bucket: bucket.map(|b| b.trim().to_string()).filter(|b| !b.is_empty()),
        display_name,
        auto_reconnect,
        pinned: true,
        last_connected_at: chrono::Utc::now().to_rfc3339(),
    }
}

#[cfg(test)]
#[path = "s3_accounts_test.rs"]
mod s3_accounts_test;
