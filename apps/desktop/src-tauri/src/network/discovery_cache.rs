//! The hosts discovery knows, and how much each one is worth.
//!
//! The cache outlives the mDNS browse (`discovery_gate.rs` stops it when nothing
//! needs it), so the Servers view can show known servers the moment it opens. What
//! an entry is good for depends on where it came from:
//!
//! - **Display** takes every entry: [`cached_discovered_hosts`].
//! - **Identity** (does this IP, this `.local` name, and this mDNS service name
//!   belong to one server?) takes only fresh evidence: [`fresh_discovered_hosts`].
//!   An mDNS entry is fresh only while the browse that last resolved it is still
//!   running. ❗ Merging or matching two servers on a stale pairing is how a
//!   reassigned IP would hand one server's mount, index, or password to another.
//!
//! Manual servers and the E2E virtual hosts come from config, not the network, so
//! they're always evidence ([`Evidence::Pinned`]).

use super::{DiscoveryState, HostSource, NetworkDiscoveryStateChanged, NetworkHost, NetworkHostFound};
use super::{NetworkHostLost, NetworkHostResolved};
use crate::ignore_poison::IgnorePoison;
use log::debug;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{Emitter, Manager};
use tauri_specta::Event;

/// Where an entry's address came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Evidence {
    /// Config: a manual server or an E2E virtual host.
    Pinned,
    /// mDNS: the browse that last saw the host, and the one that last resolved its
    /// address (`None` until one has).
    Mdns { seen_in: u64, resolved_in: Option<u64> },
}

#[derive(Debug, Clone)]
struct CachedHost {
    host: NetworkHost,
    evidence: Evidence,
}

/// The hosts, the discovery state the UI shows, and which browse is live. Each
/// browse gets a new number, so an event from one that has stopped can't write into
/// the next, and "resolved by the live browse" is one comparison.
#[derive(Debug)]
pub(crate) struct DiscoveryCache {
    hosts: HashMap<String, CachedHost>,
    state: DiscoveryState,
    live_browse: Option<u64>,
    last_browse: u64,
}

impl Default for DiscoveryCache {
    fn default() -> Self {
        Self {
            hosts: HashMap::new(),
            state: DiscoveryState::Idle,
            live_browse: None,
            last_browse: 0,
        }
    }
}

impl DiscoveryCache {
    /// A new browse is starting: every mDNS entry is stale until it resolves again.
    pub(crate) fn begin_browse(&mut self) -> u64 {
        self.last_browse += 1;
        self.live_browse = Some(self.last_browse);
        self.last_browse
    }

    /// The browse stopped. Its hosts stay for display.
    pub(crate) fn end_browse(&mut self) {
        self.live_browse = None;
        self.state = DiscoveryState::Idle;
    }

    #[cfg(test)]
    pub(crate) fn live_browse(&self) -> Option<u64> {
        self.live_browse
    }

    fn is_live(&self, browse: u64) -> bool {
        self.live_browse == Some(browse)
    }

    fn is_fresh(&self, evidence: Evidence) -> bool {
        match evidence {
            Evidence::Pinned => true,
            Evidence::Mdns { resolved_in, .. } => resolved_in.is_some_and(|browse| self.is_live(browse)),
        }
    }

    pub(crate) fn fresh_hosts(&self) -> Vec<NetworkHost> {
        self.hosts
            .values()
            .filter(|entry| self.is_fresh(entry.evidence))
            .map(|entry| entry.host.clone())
            .collect()
    }

    pub(crate) fn cached_hosts(&self) -> Vec<NetworkHost> {
        self.hosts.values().map(|entry| entry.host.clone()).collect()
    }

    /// A host from config: always evidence. Returns the host to announce.
    pub(crate) fn pinned_found(&mut self, host: NetworkHost) -> NetworkHost {
        debug!(
            "Host {}: serverId={:?}, server={:?}, ip={:?}, host={:?}",
            if self.hosts.contains_key(&host.id) {
                "UPDATED"
            } else {
                "ADDED"
            },
            host.id,
            host.name,
            host.ip_address,
            host.hostname
        );
        let entry = CachedHost {
            host: host.clone(),
            evidence: Evidence::Pinned,
        };
        self.hosts.insert(host.id.clone(), entry);
        host
    }

    /// The browse `browse` found a host. A cached one keeps its last address for
    /// display, but it isn't evidence until this browse resolves it. Returns the host
    /// to announce, or `None` for an event from a browse that has stopped.
    pub(crate) fn mdns_found(&mut self, host: NetworkHost, browse: u64) -> Option<NetworkHost> {
        if !self.is_live(browse) {
            return None;
        }
        let entry = self.hosts.entry(host.id.clone()).or_insert_with(|| {
            debug!("Host ADDED: serverId={:?}, server={:?}", host.id, host.name);
            CachedHost {
                host: host.clone(),
                evidence: Evidence::Mdns {
                    seen_in: browse,
                    resolved_in: None,
                },
            }
        });
        entry.host.name = host.name;
        entry.evidence = match entry.evidence {
            Evidence::Mdns { resolved_in, .. } => Evidence::Mdns {
                seen_in: browse,
                resolved_in,
            },
            pinned @ Evidence::Pinned => pinned,
        };
        Some(entry.host.clone())
    }

    /// The browse `browse` resolved a host's address. Returns the host to announce as
    /// found first (when resolution beat the find), and the resolved host, or `None`
    /// for an event from a browse that has stopped.
    pub(crate) fn mdns_resolved(
        &mut self,
        host_id: &str,
        name: &str,
        hostname: Option<String>,
        ip_address: Option<String>,
        port: u16,
        browse: u64,
    ) -> Option<(Option<NetworkHost>, NetworkHost)> {
        if !self.is_live(browse) {
            return None;
        }
        let mut found_first = None;
        let entry = self.hosts.entry(host_id.to_string()).or_insert_with(|| {
            debug!("Host RESOLVED before FOUND, creating entry: serverId={host_id:?}, server={name:?}");
            let host = NetworkHost {
                id: host_id.to_string(),
                name: name.to_string(),
                hostname: None,
                ip_address: None,
                port,
                source: HostSource::Discovered,
            };
            found_first = Some(host.clone());
            CachedHost {
                host,
                evidence: Evidence::Mdns {
                    seen_in: browse,
                    resolved_in: None,
                },
            }
        });
        // Within one browse, a later answer fills in what an earlier one gave. Across
        // browses it replaces it: an address from a stopped browse riding along on a
        // fresh entry would pass stale evidence off as fresh.
        let resolved_earlier_in_this_browse =
            matches!(entry.evidence, Evidence::Mdns { resolved_in: Some(earlier), .. } if earlier == browse);
        if resolved_earlier_in_this_browse {
            entry.host.hostname = hostname.or(entry.host.hostname.take());
            entry.host.ip_address = ip_address.or(entry.host.ip_address.take());
        } else {
            entry.host.hostname = hostname;
            entry.host.ip_address = ip_address;
        }
        entry.host.port = port;
        if let Evidence::Mdns { .. } = entry.evidence {
            entry.evidence = Evidence::Mdns {
                seen_in: browse,
                resolved_in: Some(browse),
            };
        }
        debug!(
            "Host RESOLVED: serverId={host_id:?}, host={:?}, ip={:?}, port={port}",
            entry.host.hostname, entry.host.ip_address
        );
        Some((found_first, entry.host.clone()))
    }

    /// The browse `browse` saw a host leave. Returns whether it was removed; an event
    /// from a browse that has stopped removes nothing.
    pub(crate) fn mdns_lost(&mut self, host_id: &str, browse: u64) -> bool {
        self.is_live(browse) && self.remove(host_id)
    }

    /// Records the browse's discovery state. `false` for a browse that has stopped.
    pub(crate) fn mdns_state(&mut self, state: DiscoveryState, browse: u64) -> bool {
        if !self.is_live(browse) {
            return false;
        }
        self.state = state;
        true
    }

    /// Drops every mDNS host the live browse `browse` hasn't seen: it went away while
    /// nothing was browsing. Returns the ids dropped; nothing if `browse` has stopped.
    pub(crate) fn drop_unseen(&mut self, browse: u64) -> Vec<String> {
        if !self.is_live(browse) {
            return Vec::new();
        }
        let unseen: Vec<String> = self
            .hosts
            .iter()
            .filter(|(_, entry)| matches!(entry.evidence, Evidence::Mdns { seen_in, .. } if seen_in != browse))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &unseen {
            self.remove(id);
        }
        unseen
    }

    fn remove(&mut self, host_id: &str) -> bool {
        let Some(removed) = self.hosts.remove(host_id) else {
            return false;
        };
        debug!(
            "Host REMOVED: serverId={:?}, server={:?}, ip={:?}",
            removed.host.id, removed.host.name, removed.host.ip_address
        );
        true
    }
}

/// The one cache, shared by discovery, the manual-server store, and every reader.
static CACHE: Mutex<Option<DiscoveryCache>> = Mutex::new(None);

fn with_cache<T>(f: impl FnOnce(&mut DiscoveryCache) -> T) -> T {
    let mut guard = CACHE.lock_ignore_poison();
    f(guard.get_or_insert_with(DiscoveryCache::default))
}

/// Hosts whose name ↔ address pairing is current: pinned ones, and mDNS ones the
/// running browse resolved. ❗ The only list identity may be decided on: matching a
/// mount to a server, deduping servers or volumes, picking saved credentials.
pub fn fresh_discovered_hosts() -> Vec<NetworkHost> {
    with_cache(|cache| cache.fresh_hosts())
}

/// Every host discovery knows, stale ones included. ❗ For display only.
pub fn cached_discovered_hosts() -> Vec<NetworkHost> {
    with_cache(|cache| cache.cached_hosts())
}

/// Gets the current discovery state.
pub fn get_discovery_state_value() -> DiscoveryState {
    with_cache(|cache| cache.state)
}

/// Starts a browse's generation. See [`DiscoveryCache::begin_browse`].
pub(crate) fn begin_browse() -> u64 {
    with_cache(DiscoveryCache::begin_browse)
}

/// Ends the live browse and tells the UI discovery is idle.
pub(crate) fn end_browse<R: tauri::Runtime>(app_handle: &(impl Emitter<R> + Manager<R>)) {
    with_cache(DiscoveryCache::end_browse);
    let _ = NetworkDiscoveryStateChanged {
        state: DiscoveryState::Idle,
    }
    .emit(app_handle);
}

/// Drains the cached host map and resets discovery state to `Idle`. Pure
/// mutation: returns the IDs of hosts that were removed so the caller can
/// emit `network-host-lost` for each. Testable without a Tauri runtime.
pub(crate) fn drain_discovered_hosts() -> Vec<String> {
    with_cache(|cache| {
        let ids: Vec<String> = cache.hosts.keys().cloned().collect();
        cache.hosts.clear();
        cache.state = DiscoveryState::Idle;
        ids
    })
}

/// Clears all discovered hosts and resets discovery state to `Idle`.
/// Called when networking is disabled via the user toggle so the frontend store empties
/// without waiting for `network-host-lost` events from a stopped daemon.
pub fn clear_discovered_hosts<R: tauri::Runtime>(app_handle: &(impl Emitter<R> + Manager<R>)) {
    let removed_ids = drain_discovered_hosts();
    for id in removed_ids {
        let _ = NetworkHostLost { id }.emit(app_handle);
    }
    let _ = NetworkDiscoveryStateChanged {
        state: DiscoveryState::Idle,
    }
    .emit(app_handle);
}

/// A host from config (a manual server, an E2E virtual host) joins the list.
pub(crate) fn on_host_found<R: tauri::Runtime>(host: NetworkHost, app_handle: &(impl Emitter<R> + Manager<R>)) {
    let host = with_cache(|cache| cache.pinned_found(host));
    let _ = NetworkHostFound { host }.emit(app_handle);
}

/// A host leaves the list (a manual server removed).
pub(crate) fn on_host_lost<R: tauri::Runtime>(host_id: &str, app_handle: &(impl Emitter<R> + Manager<R>)) {
    if with_cache(|cache| cache.remove(host_id)) {
        let _ = NetworkHostLost {
            id: host_id.to_string(),
        }
        .emit(app_handle);
    }
}

/// The browse `browse` found a host.
pub(crate) fn on_mdns_host_found<R: tauri::Runtime>(
    host: NetworkHost,
    browse: u64,
    app_handle: &(impl Emitter<R> + Manager<R>),
) {
    if let Some(host) = with_cache(|cache| cache.mdns_found(host, browse)) {
        let _ = NetworkHostFound { host }.emit(app_handle);
    }
}

/// The browse `browse` resolved a host's address.
pub(crate) fn on_mdns_host_resolved<R: tauri::Runtime>(
    resolved: MdnsResolution,
    browse: u64,
    app_handle: &(impl Emitter<R> + Manager<R>),
) {
    let MdnsResolution {
        host_id,
        name,
        hostname,
        ip_address,
        port,
    } = resolved;
    let Some((found_first, host)) =
        with_cache(|cache| cache.mdns_resolved(&host_id, &name, hostname, ip_address, port, browse))
    else {
        return;
    };
    if let Some(host) = found_first {
        let _ = NetworkHostFound { host }.emit(app_handle);
    }
    let _ = NetworkHostResolved { host }.emit(app_handle);
}

/// What mDNS resolved about one host.
pub(crate) struct MdnsResolution {
    pub host_id: String,
    pub name: String,
    pub hostname: Option<String>,
    pub ip_address: Option<String>,
    pub port: u16,
}

/// The browse `browse` saw a host leave.
pub(crate) fn on_mdns_host_lost<R: tauri::Runtime>(
    host_id: &str,
    browse: u64,
    app_handle: &(impl Emitter<R> + Manager<R>),
) {
    if with_cache(|cache| cache.mdns_lost(host_id, browse)) {
        let _ = NetworkHostLost {
            id: host_id.to_string(),
        }
        .emit(app_handle);
    }
}

/// The browse `browse` changed its discovery state.
pub(crate) fn on_mdns_state_changed<R: tauri::Runtime>(
    state: DiscoveryState,
    browse: u64,
    app_handle: &(impl Emitter<R> + Manager<R>),
) {
    if with_cache(|cache| cache.mdns_state(state, browse)) {
        let _ = NetworkDiscoveryStateChanged { state }.emit(app_handle);
    }
}

/// The browse `browse` has had time to hear from every host still on the network:
/// drop the cached ones it didn't.
pub(crate) fn settle_browse<R: tauri::Runtime>(browse: u64, app_handle: &(impl Emitter<R> + Manager<R>)) {
    for id in with_cache(|cache| cache.drop_unseen(browse)) {
        let _ = NetworkHostLost { id }.emit(app_handle);
    }
}

/// Information needed to resolve a host, extracted without holding mutex long.
pub struct HostResolutionInfo {
    pub id: String,
    pub name: String,
    pub hostname: Option<String>,
    pub ip_address: Option<String>,
    pub port: u16,
    pub source: HostSource,
}

/// Gets the information needed to resolve a host. Brief mutex hold.
pub fn get_host_for_resolution(host_id: &str) -> Option<HostResolutionInfo> {
    with_cache(|cache| {
        cache.hosts.get(host_id).map(|entry| HostResolutionInfo {
            id: entry.host.id.clone(),
            name: entry.host.name.clone(),
            hostname: entry.host.hostname.clone(),
            ip_address: entry.host.ip_address.clone(),
            port: entry.host.port,
            source: entry.host.source,
        })
    })
}

/// Updates a host with resolved hostname and IP, for display. Brief mutex hold.
///
/// A DNS answer to "where is this host now" doesn't make an mDNS pairing fresh, so the
/// entry's evidence stays as it was.
pub fn update_host_resolution(host_id: &str, hostname: String, ip_address: Option<String>) -> Option<NetworkHost> {
    with_cache(|cache| {
        let entry = cache.hosts.get_mut(host_id)?;
        entry.host.hostname = Some(hostname);
        entry.host.ip_address = ip_address;
        Some(entry.host.clone())
    })
}

#[cfg(test)]
#[path = "discovery_cache_test.rs"]
mod tests;
