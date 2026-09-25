//! The SMB half of the saved-server listing: one row per host, carrying its saved
//! shares as places. Split out of `servers.rs` to keep that file under its length
//! cap; the listing's own rationale stays there. Model:
//! `docs/specs/saved-smb-shares.md`.
//!
//! ❗ **A host's identity is where it dials: address and port.** Never the name a
//! person gave it (that's a label they can change), and never the address alone
//! (two SMB servers on one machine are two hosts: the Docker fixtures are ten on
//! `localhost`). Naming a host once split it in two, because its share history
//! was matched against the name, and the made-up second row then took edits
//! meant for the first.
//!
//! ❗ **The listing is also what Edit and Forget act on.** They look a host up by
//! the id this module gave it ([`smb_host_group`]) and touch exactly the store
//! rows it filed under that host, so "what the row showed" and "what Forget took"
//! can't disagree.

use super::{SavedPlace, SavedServer, ServerNameSource, ServerProtocol};
use crate::network::known_shares::KnownNetworkShare;
use crate::network::{NetworkHost, manual_servers};

/// SMB's port when a store row names none.
const DEFAULT_SMB_PORT: u16 = 445;

/// One saved SMB host, with the store rows the listing filed under it.
pub(crate) struct SmbHostGroup {
    /// The row the hub shows.
    pub server: SavedServer,
    /// What it dials. With [`port`](Self::port), its identity.
    pub host: String,
    pub port: u16,
    /// The name the discovery list gives it, which is what the share list files
    /// its history under (`localhost:11482`, `Naspolya`).
    discovery_name: String,
    /// Whether a manual entry backs it; its id is then that entry's.
    pub manual: bool,
    /// Its sign-in history and its saved shares, as the share store holds them.
    pub rows: Vec<KnownNetworkShare>,
}

/// The SMB hosts, from the manually-typed list and the share store, deduped, each
/// carrying its saved shares as places. See [`smb_host_groups`].
pub(super) fn smb_hosts(
    manual: Vec<manual_servers::ManualServerEntry>,
    known: Vec<KnownNetworkShare>,
    hosts: &[NetworkHost],
) -> Vec<SavedServer> {
    smb_host_groups(manual, known, hosts)
        .into_iter()
        .map(|group| group.server)
        .collect()
}

/// The saved SMB host the listing calls `id`, read from the stores as they are now.
pub(crate) fn smb_host_group(id: &str, manual: Vec<manual_servers::ManualServerEntry>) -> Option<SmbHostGroup> {
    smb_host_groups(
        manual,
        crate::network::known_shares::get_all_known_shares(),
        &crate::network::get_discovered_hosts(),
    )
    .into_iter()
    .find(|group| group.server.id == id)
}

/// The SMB hosts with the rows each one speaks for.
///
/// One group per HOST: a host the user typed by hand is the same host its share
/// history names, and its saved shares hang under it.
///
/// ❗ **Manual entries go first**, because only a manual entry can carry a name a
/// person typed. A share-history row for the same host then only lends it the time
/// it was last used.
pub(crate) fn smb_host_groups(
    manual: Vec<manual_servers::ManualServerEntry>,
    known: Vec<KnownNetworkShare>,
    hosts: &[NetworkHost],
) -> Vec<SmbHostGroup> {
    let manager = crate::file_system::volume::manager::get_volume_manager();
    let mut groups: Vec<SmbHostGroup> = Vec::new();

    for entry in manual {
        let label = entry.label();
        let discovery_name = manual_servers::discovery_name(&entry.address, entry.port);
        groups.push(SmbHostGroup {
            server: SavedServer {
                // ❗ `User` only for a name a person typed. An unnamed entry's
                // label is the ADDRESS they typed (`host` or `host:port`), worn as
                // a stand-in, so a Bonjour name outranks it, the same as a mount's.
                name_source: if entry.is_named() {
                    ServerNameSource::User
                } else {
                    ServerNameSource::Fallback
                },
                id: entry.id,
                protocol: ServerProtocol::Smb,
                display_name: label,
                address: discovery_name.clone(),
                // The account the person typed for it, a preference rather than an
                // identity: the first sign-in prefills it, and the listing skips guest.
                username: entry.username,
                pinned: false,
                // `added_at` is when it was typed, ❌ not when it last answered.
                last_connected_at: None,
                auto_reconnect: None,
                places: Vec::new(),
            },
            host: entry.address,
            port: entry.port,
            discovery_name,
            manual: true,
            rows: Vec::new(),
        });
    }

    // History rows first, so a host the share list signed in to exists before
    // the shares that hang under it look for it.
    let (history, shares): (Vec<_>, Vec<_>) = known.into_iter().partition(|row| !row.is_share());
    for row in history.into_iter().chain(shares) {
        let index = match groups.iter().position(|group| is_host_of(group, &row, hosts)) {
            Some(index) => index,
            None => {
                groups.push(unlisted_host(&row));
                groups.len() - 1
            }
        };
        let group = &mut groups[index];
        let at = &row.last_connected_at;
        if group.server.last_connected_at.as_deref() < Some(at.as_str()) {
            group.server.last_connected_at = Some(at.clone());
        }
        if row.is_share() {
            group.server.places.push(share_place(&row, manager));
        }
        group.rows.push(row);
    }
    groups
}

/// A host nobody typed in, as the first store row that names it: dialed where its
/// mount went, or by the name the row was filed under when no mount went through.
fn unlisted_host(row: &KnownNetworkShare) -> SmbHostGroup {
    let host = row.address.clone().unwrap_or_else(|| row.server_name.clone());
    let port = row.port.unwrap_or(DEFAULT_SMB_PORT);
    SmbHostGroup {
        server: SavedServer {
            id: manual_servers::generate_server_id(&host, port),
            protocol: ServerProtocol::Smb,
            display_name: row.server_name.clone(),
            // ❗ The mount's spelling, which nobody chose: the hub lets a
            // discovered Bonjour name outrank it.
            name_source: ServerNameSource::Fallback,
            address: manual_servers::discovery_name(&host, port),
            // ❗ Not a share's username: that is per-share, and this row is the
            // HOST. The typed account lives on the manual entry.
            username: None,
            pinned: false,
            last_connected_at: None,
            auto_reconnect: None,
            places: Vec::new(),
        },
        host,
        port,
        discovery_name: row.server_name.clone(),
        manual: false,
        rows: Vec::new(),
    }
}

/// Whether `row` was filed under the host `group` is.
///
/// - By the discovery name, which is what the share list files a host's history
///   and shares under (it includes the port off 445).
/// - By where a mount dialed, on the SAME port: an address alone names a machine,
///   and one machine can run several servers.
/// - A row with no address (history, or a share an Add named) knows no port, so
///   its server name only matches a host on 445, by identity under any spelling.
fn is_host_of(group: &SmbHostGroup, row: &KnownNetworkShare, hosts: &[NetworkHost]) -> bool {
    use crate::network::server_identity::same_server;

    if row.server_name.eq_ignore_ascii_case(&group.discovery_name) {
        return true;
    }
    match row.address.as_deref() {
        Some(address) => {
            row.port.unwrap_or(DEFAULT_SMB_PORT) == group.port
                && (address.eq_ignore_ascii_case(&group.host) || same_server(address, &group.host, hosts))
        }
        None => group.port == DEFAULT_SMB_PORT && same_server(&row.server_name, &group.host, hosts),
    }
}

/// A saved share, as a place under its host.
///
/// ❗ Its id is the one its last mount had. One no mount went through gets the id
/// a mount by its host's name would mint, only so the hub has something stable to
/// key it by: nothing in the volume list carries it, which is how the hub knows
/// to open such a share through its host's share list instead.
fn share_place(row: &KnownNetworkShare, manager: &crate::file_system::volume::manager::VolumeManager) -> SavedPlace {
    let volume_id = crate::network::known_shares::place_id(row);
    SavedPlace {
        connected: manager.get(&volume_id).is_some(),
        app_root: row
            .mount_path
            .clone()
            .unwrap_or_else(|| format!("smb://{}/{}", row.server_name, row.share_name)),
        name: row.share_name.clone(),
        pinned: row.pinned,
        username: row.username.clone(),
        volume_id,
    }
}
