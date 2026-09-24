//! The SMB half of the saved-server listing: one row per host, carrying its saved
//! shares as places. Split out of `servers.rs` to keep that file under its length
//! cap; the listing's own rationale stays there. Model:
//! `docs/specs/saved-smb-shares.md`.

use super::{SavedPlace, SavedServer, ServerNameSource, ServerProtocol};
use crate::network::{known_shares, manual_servers};

/// The SMB hosts, from the manually-typed list and the share store, deduped, each
/// carrying its saved shares as places.
///
/// One row per HOST: a host the user typed by hand is the same host its share
/// history names, and its saved shares hang under it.
///
/// ❗ **Manual entries go first**, because the dedup keeps the first row it sees
/// and only a manual entry can carry a name a person typed. A share-history row
/// for the same host then only lends it the time it was last used.
pub(super) fn smb_hosts(
    manual: Vec<manual_servers::ManualServerEntry>,
    known: Vec<known_shares::KnownNetworkShare>,
    hosts: &[crate::network::NetworkHost],
) -> Vec<SavedServer> {
    let manager = crate::file_system::volume::manager::get_volume_manager();
    let mut servers: Vec<SavedServer> = Vec::new();

    for entry in manual {
        let label = entry.label();
        servers.push(SavedServer {
            // ❗ `User` only for a name a person typed. An unnamed entry's label
            // is the ADDRESS they typed (`host` or `host:port`), worn as a
            // stand-in, so a Bonjour name outranks it, the same as a mount's.
            name_source: if entry.is_named() {
                ServerNameSource::User
            } else {
                ServerNameSource::Fallback
            },
            id: entry.id,
            protocol: ServerProtocol::Smb,
            display_name: label,
            address: entry.address,
            // The account the person typed for it, a preference rather than an
            // identity: the first sign-in prefills it, and the listing skips guest.
            username: entry.username,
            pinned: false,
            // `added_at` is when it was typed, ❌ not when it last answered.
            last_connected_at: None,
            auto_reconnect: None,
            places: Vec::new(),
        });
    }

    // History rows first, so a host the share list signed in to exists before
    // the shares that hang under it look for it.
    let (history, shares): (Vec<_>, Vec<_>) = known.into_iter().partition(|row| !row.is_share());
    for row in history.into_iter().chain(shares) {
        let at = row.last_connected_at.clone();
        let index = match servers.iter().position(|server| is_host_of(server, &row, hosts)) {
            Some(index) => index,
            None => {
                servers.push(SavedServer {
                    id: manual_servers::generate_server_id(&row.server_name, 445),
                    protocol: ServerProtocol::Smb,
                    display_name: row.server_name.clone(),
                    // ❗ The mount's spelling, which nobody chose: the hub lets a
                    // discovered Bonjour name outrank it.
                    name_source: ServerNameSource::Fallback,
                    address: row.server_name.clone(),
                    // ❗ Not a share's username: that is per-share, and this row
                    // is the HOST. The typed account lives on the manual entry.
                    username: None,
                    pinned: false,
                    last_connected_at: None,
                    auto_reconnect: None,
                    places: Vec::new(),
                });
                servers.len() - 1
            }
        };
        let server = &mut servers[index];
        if server.last_connected_at.as_deref() < Some(at.as_str()) {
            server.last_connected_at = Some(at);
        }
        if row.is_share() {
            server.places.push(share_place(row, manager));
        }
    }
    servers
}

/// Whether the saved SMB host `server` is the one `row` was filed under: by the
/// name the row keeps, case aside, or by identity under either of its names.
fn is_host_of(
    server: &SavedServer,
    row: &known_shares::KnownNetworkShare,
    hosts: &[crate::network::NetworkHost],
) -> bool {
    use crate::network::server_identity::same_server;

    let names = std::iter::once(row.server_name.as_str()).chain(row.address.as_deref());
    names.into_iter().any(|name| {
        server.address.eq_ignore_ascii_case(name)
            || server.display_name.eq_ignore_ascii_case(name)
            || same_server(&server.address, name, hosts)
    })
}

/// A saved share, as a place under its host.
///
/// ❗ Its id is the one its last mount had. One no mount went through gets the id
/// a mount by its host's name would mint, only so the hub has something stable to
/// key it by: nothing in the volume list carries it, which is how the hub knows
/// to open such a share through its host's share list instead.
fn share_place(
    row: known_shares::KnownNetworkShare,
    manager: &crate::file_system::volume::manager::VolumeManager,
) -> SavedPlace {
    let volume_id = row.volume_id.clone().unwrap_or_else(|| {
        cmdr_fs::volume::smb_volume_id(
            row.address.as_deref().unwrap_or(&row.server_name),
            row.port.unwrap_or(445),
            &row.share_name,
        )
    });
    SavedPlace {
        connected: manager.get(&volume_id).is_some(),
        app_root: row
            .mount_path
            .clone()
            .unwrap_or_else(|| format!("smb://{}/{}", row.server_name, row.share_name)),
        name: row.share_name,
        pinned: row.pinned,
        username: row.username,
        volume_id,
    }
}
