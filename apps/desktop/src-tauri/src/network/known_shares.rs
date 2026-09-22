//! Known network shares store.
//!
//! Persists metadata about network shares the user has connected to.
//! Enables username pre-fill, auth change detection, and quick reconnect.

use crate::ignore_poison::IgnorePoison;
use crate::network::NetworkHost;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Connection mode used for the last successful connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionMode {
    Guest,
    Credentials,
}

/// Authentication options available for a share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum AuthOptions {
    GuestOnly,
    CredentialsOnly,
    GuestOrCredentials,
}

/// Information about a known network share.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct KnownNetworkShare {
    pub server_name: String,
    pub share_name: String,
    /// Currently only "smb".
    pub protocol: String,
    /// ISO 8601.
    pub last_connected_at: String,
    pub last_connection_mode: ConnectionMode,
    pub last_known_auth_options: AuthOptions,
    /// None for guest.
    pub username: Option<String>,
}

/// One share on one server, named the way the mount reported it. Private, like the
/// list it lives in: the switch is read and set only through
/// [`direct_connection_enabled`] and [`set_direct_connection_enabled`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareRef {
    server_name: String,
    share_name: String,
}

/// The known shares store, persisted to disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownSharesStore {
    #[serde(default)]
    pub known_network_shares: Vec<KnownNetworkShare>,
    /// The shares the user turned "Use Cmdr's fast direct connection" off for.
    ///
    /// An opt-out list rather than a flag on [`KnownNetworkShare`], because that type
    /// records Cmdr's OWN connects (its one writer files server-level rows): a share
    /// macOS mounted has no row, and minting one would invent a connection history
    /// that then shows up in the servers hub. Absence means on, so a store saved
    /// before the setting existed keeps every share on the fast connection. See
    /// [`direct_connection_enabled`].
    #[serde(default)]
    direct_connection_opt_outs: Vec<ShareRef>,
}

/// In-memory cache of known shares, synchronized with disk.
static KNOWN_SHARES: std::sync::OnceLock<Mutex<KnownSharesStore>> = std::sync::OnceLock::new();

fn get_known_shares_mutex() -> &'static Mutex<KnownSharesStore> {
    KNOWN_SHARES.get_or_init(|| Mutex::new(KnownSharesStore::default()))
}

/// Durably writes content to a file using write-to-temp + fsync + rename + parent-dir fsync.
/// On failure, the original file (if any) remains intact. The fsyncs make the write survive a
/// power loss, not just process death. See `crate::config::durable_write_json` for the rationale.
fn atomic_write_json(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    crate::config::durable_write_json(path, &tmp, content)
}

/// Removes a stale `.tmp` file left over from a crash during atomic write.
fn cleanup_tmp_file(path: &Path) {
    let tmp = path.with_extension("json.tmp");
    if tmp.exists() {
        let _ = fs::remove_file(&tmp);
    }
}

/// Where the store lives on disk, set once by [`load_known_shares`]. Kept here so a
/// write needs no `AppHandle`: "Connect directly" records its consent from code the
/// MCP executor also calls, and that holds none. Unset (tests, no data dir) means the
/// store lives in memory only.
static STORE_PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Returns the path to the known shares store file.
fn get_store_path<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Option<PathBuf> {
    crate::config::resolved_app_data_dir(app)
        .ok()
        .map(|dir| dir.join("known-shares.json"))
}

/// Loads known shares from disk into memory.
pub fn load_known_shares<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let Some(path) = get_store_path(app) else {
        return;
    };
    let _ = STORE_PATH.set(path.clone());

    cleanup_tmp_file(&path);

    let store = if let Ok(contents) = fs::read_to_string(&path) {
        serde_json::from_str(&contents).unwrap_or_default()
    } else {
        KnownSharesStore::default()
    };

    *get_known_shares_mutex().lock_ignore_poison() = store;
}

/// Saves known shares from memory to disk.
fn save_known_shares() {
    let Some(path) = STORE_PATH.get() else {
        return;
    };

    let store = get_known_shares_mutex().lock_ignore_poison().clone();

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if let Ok(json) = serde_json::to_string_pretty(&store)
        && let Err(e) = atomic_write_json(path, &json)
    {
        log::warn!("Couldn't write known shares store: {}", e);
    }
}

/// Creates a unique key for a share.
///
/// Case- and NFC-folded, because one share reaches this store spelled more than one
/// way: the frontend saves the name from the server's share list (composed), while
/// the mount paths look it up from `statfs` (decomposed on macOS). A byte key
/// remembers one share as two, and the auth mode saved under one spelling is missing
/// under the other.
fn share_key(server_name: &str, share_name: &str) -> String {
    format!("{}/{}", fold_name(server_name), fold_name(share_name))
}

/// Gets all known network shares.
pub fn get_all_known_shares() -> Vec<KnownNetworkShare> {
    get_known_shares_mutex()
        .lock_ignore_poison()
        .known_network_shares
        .clone()
}

/// Gets a specific known share by server and share name.
pub fn get_known_share(server_name: &str, share_name: &str) -> Option<KnownNetworkShare> {
    let key = share_key(server_name, share_name);
    get_known_shares_mutex()
        .lock_ignore_poison()
        .known_network_shares
        .iter()
        .find(|s| share_key(&s.server_name, &s.share_name) == key)
        .cloned()
}

/// Updates or adds a known network share.
/// Called after a successful connection.
pub fn update_known_share(share: KnownNetworkShare) {
    let key = share_key(&share.server_name, &share.share_name);

    {
        let mut cache = get_known_shares_mutex().lock_ignore_poison();
        // Find and update, or add new
        if let Some(existing) = cache
            .known_network_shares
            .iter_mut()
            .find(|s| share_key(&s.server_name, &s.share_name) == key)
        {
            *existing = share;
        } else {
            cache.known_network_shares.push(share);
        }
    }

    save_known_shares();
}

/// The username to pre-fill a login form for `server_name`, if this server has ever been
/// signed in to.
///
/// A LOOKUP, not a map of every server's hint, for the same reason
/// [`get_known_share`] is one: the key stays on this side. A command handing the
/// frontend a keyed map makes the key part of the IPC contract, and the caller has to
/// rebuild it to read its own answer, so the rule ends up written twice in two
/// languages and drifts. Ask by name and there is one rule, here.
///
/// Matching is [`credential_key`](crate::network::server_identity::credential_key), the
/// same identity the stored PASSWORD is keyed by, so every name form of one server
/// (`Naspolya`, `naspolya.local`, `Naspolya._smb._tcp.local`) finds the hint saved under
/// any other. The share list records whichever name the connect used, while the login
/// form opens on whatever discovery produced; a raw compare misses exactly the case the
/// hint exists for.
///
/// Last match wins: shares are appended in connect order, so the newest username on this
/// server is the one the person most recently signed in as.
pub fn get_username_hint(server_name: &str) -> Option<String> {
    use crate::network::server_identity::credential_key;

    let key = credential_key(server_name);
    get_known_shares_mutex()
        .lock_ignore_poison()
        .known_network_shares
        .iter()
        .filter(|s| credential_key(&s.server_name) == key)
        .filter_map(|s| s.username.clone())
        .next_back()
}

/// Case- and NFC-folds one half of a share's name, the fold [`share_key`] applies.
fn fold_name(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;

    name.nfc().flat_map(char::to_lowercase).collect()
}

/// Whether `entry` names the share `share` on the server any of `server_names` names.
///
/// The server half asks [`same_server`](crate::network::server_identity::same_server)
/// rather than comparing keys, because `statfs` echoes whichever name form each mount
/// used: one NAS is `192.168.1.111` on one mount and `Naspolya._smb._tcp.local` on the
/// next, and a choice the other spelling can't see looks like the switch resetting
/// itself.
fn names_share(entry: &ShareRef, server_names: &[&str], share: &str, hosts: &[NetworkHost]) -> bool {
    use crate::network::server_identity::same_server;

    fold_name(&entry.share_name) == fold_name(share)
        && server_names
            .iter()
            .any(|server| same_server(&entry.server_name, server, hosts))
}

/// Whether `opt_outs` holds the share, under any name form of its server.
fn is_opted_out(opt_outs: &[ShareRef], server_names: &[&str], share: &str, hosts: &[NetworkHost]) -> bool {
    opt_outs
        .iter()
        .any(|entry| names_share(entry, server_names, share, hosts))
}

/// Records (`enabled: false`) or drops (`enabled: true`) the share's opt-out in `opt_outs`.
///
/// Either way every entry naming the share goes first, so a share chosen under two
/// spellings keeps one entry, filed under the newest.
fn apply_choice(opt_outs: &mut Vec<ShareRef>, server_name: &str, share: &str, enabled: bool, hosts: &[NetworkHost]) {
    opt_outs.retain(|entry| !names_share(entry, &[server_name], share, hosts));
    if !enabled {
        opt_outs.push(ShareRef {
            server_name: server_name.to_string(),
            share_name: share.to_string(),
        });
    }
}

/// Whether Cmdr may give this share its own direct smb2 session without being asked
/// right then: the per-share "Use Cmdr's fast direct connection" switch. `true` unless
/// the user turned it off.
///
/// ❗ **Every upgrade nobody clicked for must ask this**, under
/// `smb_upgrade::lock_volume_upgrade` and right before dialing. Today that's one place,
/// `smb_upgrade::register_smb_volume`, which the startup pass, the mount watcher, and
/// Cmdr's own mount all funnel through. A trigger that bypasses it makes the switch do
/// nothing.
///
/// `server_names` takes every spelling the caller has for the server (the `statfs`
/// name and the address it resolved to, say); any one of them matching counts.
pub fn direct_connection_enabled(server_names: &[&str], share: &str) -> bool {
    // Discovery is read BEFORE this store's lock is taken, so the two never nest.
    let hosts = crate::network::get_discovered_hosts();
    let store = get_known_shares_mutex().lock_ignore_poison();
    !is_opted_out(&store.direct_connection_opt_outs, server_names, share, &hosts)
}

/// Records the user's "Use Cmdr's fast direct connection" choice for a share, and
/// persists it. Every "Connect directly" calls this with `true`: asking for the direct
/// session is consent to it, so the switch can't strand someone in a state they can't
/// click their way out of.
pub fn set_direct_connection_enabled(server_name: &str, share: &str, enabled: bool) {
    let hosts = crate::network::get_discovered_hosts();
    {
        let mut store = get_known_shares_mutex().lock_ignore_poison();
        apply_choice(
            &mut store.direct_connection_opt_outs,
            server_name,
            share,
            enabled,
            &hosts,
        );
    }
    save_known_shares();
}

#[cfg(test)]
#[path = "known_shares_test.rs"]
mod tests;
