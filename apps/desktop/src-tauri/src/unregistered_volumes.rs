//! Why a volume id has no volume registered under it.
//!
//! Every site that looks a volume up and finds nothing asks this one question, so
//! a listing, a copy or move, a delete, a copy preview, and a new folder all tell
//! the same story about the same id:
//!
//! - **Not connected**: a device its provider lists (an ADB phone before its
//!   pane's connect lands, or after an eject), a saved SFTP / WebDAV server, or a
//!   saved SMB share whose mount isn't there (never mounted, or ejected), that
//!   nothing has connected. Opening it in a pane is what connects it.
//! - **Gone**: any other id, which is a volume that left the registry (an unmount
//!   race).
//!
//! ❗ Asking never dials: both answers come from cached state (the provider's last
//! device list, the saved-server stores). ❌ Never word a not-connected id as
//! `DeviceDisconnected`, which says a session dropped that never existed, or as
//! `NotFound`, which the frontend reads as "this folder was deleted" and walks the
//! pane off the device. Each caller maps the answer into its own vocabulary.

/// Why no volume is registered under an id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unregistered {
    /// A listed device or a saved server that nothing has connected yet.
    NotConnected,
    /// An id nothing lists or saves: a volume that left the registry.
    Gone,
}

/// Classifies an id the volume registry had nothing for. Ask it only after a
/// lookup came back empty: it doesn't consult the registry itself.
pub(crate) async fn why_unregistered(volume_id: &str) -> Unregistered {
    let listed = crate::device_volumes::provider_for_volume_id(volume_id).await.is_some()
        || crate::server_volumes::place_root(volume_id).is_some()
        || crate::network::known_shares::share_by_volume_id(volume_id).is_some();
    if listed {
        Unregistered::NotConnected
    } else {
        Unregistered::Gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::known_shares::{self, AuthOptions, ConnectionMode, KnownNetworkShare};

    /// ❗ **A saved SMB share nobody has mounted is not connected, ❌ not gone.** As
    /// `Gone`, its listing read as `NotFound`, the pane walked up to `~` while the
    /// share's own mount was still on its way, and the pane never came back to it
    /// (QA 2026-09-25: Enter on an ejected share's row landed on the home folder).
    #[tokio::test]
    async fn a_saved_smb_share_nobody_mounted_is_not_connected() {
        let volume_id = "smb-198-51-100-61-11482-public-unregistered-test";
        known_shares::remember_share(KnownNetworkShare {
            server_name: "198.51.100.61:11482".to_string(),
            share_name: "public".to_string(),
            protocol: "smb".to_string(),
            last_connected_at: "2026-09-25T00:00:00Z".to_string(),
            last_connection_mode: ConnectionMode::Guest,
            last_known_auth_options: AuthOptions::GuestOrCredentials,
            username: None,
            address: Some("198.51.100.61".to_string()),
            port: Some(11482),
            volume_id: Some(volume_id.to_string()),
            mount_path: Some("/Volumes/public-unregistered-test".to_string()),
            pinned: true,
        });

        assert_eq!(why_unregistered(volume_id).await, Unregistered::NotConnected);
    }
}
