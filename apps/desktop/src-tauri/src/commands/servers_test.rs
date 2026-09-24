//! The protocol-agnostic server family: what it lists, what it maps, and the
//! one call it refuses.

use super::*;
use crate::network::manual_servers::ManualServerEntry;
use crate::network::sftp_known_servers::KnownSftpServer;
use crate::network::webdav_known_servers::KnownWebdavServer;

/// A host nobody else's cell uses, so this suite can share the process-global
/// stores with whatever runs beside it.
///
/// ❗ **This suite owns `192.0.2.x` and `server_volumes_test.rs` owns
/// `198.51.100.x`, one reserved block each.** Both write the same process-global
/// `KNOWN` store, so under a thread-per-test runner (`cargo test --lib`, which
/// nextest's process-per-test hides) a shared address is one cell removing the
/// entry another is about to look up, or writing it with the other pin. Pick a
/// free last octet inside your OWN block; a number that merely looks unused
/// across both files is how they collided.
///
/// Both blocks are reserved for documentation (RFC 5737) and routed nowhere.
fn sftp_entry(host: &str, pinned: bool) -> KnownSftpServer {
    KnownSftpServer {
        host: host.to_string(),
        port: 2222,
        username: "ada".to_string(),
        display_name: format!("{host} over ssh"),
        remote_root: "/srv/data".to_string(),
        start_folder: None,
        key_file: None,
        use_agent: false,
        auto_reconnect: true,
        pinned,
        last_connected_at: "2026-09-01T00:00:00Z".to_string(),
    }
}

fn webdav_entry(host: &str, pinned: bool) -> KnownWebdavServer {
    KnownWebdavServer {
        url: format!("http://{host}:8080/dav/"),
        username: "ada".to_string(),
        display_name: format!("{host} over dav"),
        remote_root: "/Photos".to_string(),
        start_folder: None,
        auto_reconnect: true,
        pinned,
        last_connected_at: "2026-09-02T00:00:00Z".to_string(),
    }
}

fn manual_entry(address: &str) -> ManualServerEntry {
    ManualServerEntry {
        id: manual_servers::generate_server_id(address, 445),
        display_name: address.to_string(),
        address: address.to_string(),
        port: 445,
        added_at: "2026-09-03T00:00:00Z".to_string(),
        username: None,
    }
}

/// A manual SMB host a person gave a name of their own.
fn named_manual_entry(address: &str, name: &str) -> ManualServerEntry {
    ManualServerEntry {
        display_name: name.to_string(),
        ..manual_entry(address)
    }
}

/// The listing over manual entries alone, with no share history and nothing
/// discovered: the stores come in as arguments, so no cell races another over
/// the process-global copies.
fn saved_servers_of(manual: Vec<ManualServerEntry>) -> Vec<SavedServer> {
    saved_servers(manual, Vec::new(), &[])
}

fn find<'a>(servers: &'a [SavedServer], display_name: &str) -> &'a SavedServer {
    servers
        .iter()
        .find(|s| s.display_name == display_name)
        .unwrap_or_else(|| panic!("{display_name} must be in the listing"))
}

/// ❗ **The listing is the union of three stores**, in one shape the hub renders
/// without branching on protocol.
#[test]
fn the_listing_unions_the_sftp_the_webdav_and_the_smb_stores() {
    let sftp_host = "192.0.2.31";
    let webdav_host = "192.0.2.32";
    let smb_host = "192.0.2.33";
    sftp_known_servers::remember(sftp_entry(sftp_host, true));
    webdav_known_servers::remember(webdav_entry(webdav_host, false));

    let servers = saved_servers_of(vec![manual_entry(smb_host)]);

    let sftp = find(&servers, &format!("{sftp_host} over ssh"));
    assert_eq!(sftp.protocol, ServerProtocol::Sftp);
    assert_eq!(sftp.address, format!("{sftp_host}:2222"));
    assert_eq!(sftp.username.as_deref(), Some("ada"));
    assert!(sftp.pinned, "the store said pinned");
    assert_eq!(sftp.last_connected_at.as_deref(), Some("2026-09-01T00:00:00Z"));

    let webdav = find(&servers, &format!("{webdav_host} over dav"));
    assert_eq!(webdav.protocol, ServerProtocol::Webdav);
    assert_eq!(webdav.address, format!("http://{webdav_host}:8080/dav/"));
    assert!(!webdav.pinned, "the store said unpinned");

    let smb = find(&servers, smb_host);
    assert_eq!(smb.protocol, ServerProtocol::Smb);
    assert_eq!(smb.address, smb_host);
}

/// ❗ **Whether a person NAMED a server is a FACT the listing publishes**,
/// because the hub ranks three names and can't tell them apart by looking at the
/// strings. An account label is the user's; an SMB host's label is the address
/// they typed, worn as a stand-in, so a Bonjour name outranks it.
#[test]
fn an_account_label_is_the_users_name_and_an_smb_hosts_address_is_a_stand_in() {
    let sftp_host = "192.0.2.41";
    let smb_host = "192.0.2.42";
    sftp_known_servers::remember(sftp_entry(sftp_host, false));

    let servers = saved_servers_of(vec![manual_entry(smb_host)]);

    assert_eq!(
        find(&servers, &format!("{sftp_host} over ssh")).name_source,
        ServerNameSource::User
    );
    assert_eq!(find(&servers, smb_host).name_source, ServerNameSource::Fallback);
}

/// ❗ **An SMB host a person named is listed by that name, as theirs**, so the
/// hub keeps it over a Bonjour name the way it keeps a named SFTP account.
#[test]
fn a_named_smb_host_is_listed_by_its_name_as_the_users_own() {
    let smb_host = "192.0.2.43";

    let servers = saved_servers_of(vec![named_manual_entry(smb_host, "Sven's NAS")]);

    let smb = find(&servers, "Sven's NAS");
    assert_eq!(smb.name_source, ServerNameSource::User);
    assert_eq!(smb.address, smb_host, "the address stays what was typed");
}

/// ❗ **An SMB host carries the account the person typed for it**, which is what
/// the edit sheet opens on.
#[test]
fn an_smb_host_carries_the_account_typed_for_it() {
    let smb_host = "192.0.2.45";
    let entry = ManualServerEntry {
        username: Some("sven".to_string()),
        ..manual_entry(smb_host)
    };

    let servers = saved_servers_of(vec![entry]);

    assert_eq!(find(&servers, smb_host).username.as_deref(), Some("sven"));
}

/// ❗ **A manual entry wins over a share-history row for the same host**, so a
/// name typed at add time survives the host being opened once: the share list
/// writes a server-level row under the same address, and the dedup keeps the
/// first row it sees.
#[test]
fn a_named_smb_host_keeps_its_name_once_its_shares_were_listed() {
    let smb_host = "192.0.2.44";
    let history = known_shares::KnownNetworkShare {
        server_name: smb_host.to_string(),
        share_name: String::new(),
        protocol: "smb".to_string(),
        last_connected_at: "2026-09-05T00:00:00Z".to_string(),
        last_connection_mode: known_shares::ConnectionMode::Guest,
        last_known_auth_options: known_shares::AuthOptions::GuestOrCredentials,
        username: None,
        address: None,
        port: None,
        volume_id: None,
        mount_path: None,
        pinned: false,
    };

    let servers = saved_servers(vec![named_manual_entry(smb_host, "Office NAS")], vec![history], &[]);

    let rows: Vec<_> = servers.iter().filter(|s| s.address == smb_host).collect();
    assert_eq!(rows.len(), 1, "one host, one row");
    assert_eq!(rows[0].display_name, "Office NAS");
    assert_eq!(
        rows[0].last_connected_at.as_deref(),
        Some("2026-09-05T00:00:00Z"),
        "the share history still says when it was last used"
    );
}

/// ❗ **An SFTP or WebDAV account has exactly ONE place, and its id is the volume
/// id**, so a `saved` row and the volume it becomes are the same thing to a tab,
/// a favorite, and the switcher.
#[test]
fn an_sftp_account_has_one_place_carrying_the_volume_id() {
    let host = "192.0.2.34";
    sftp_known_servers::remember(sftp_entry(host, true));

    let servers = saved_servers_of(Vec::new());
    let sftp = find(&servers, &format!("{host} over ssh"));

    assert_eq!(sftp.places.len(), 1, "one account, one place");
    let place = &sftp.places[0];
    assert_eq!(place.volume_id, cmdr_fs::volume::sftp_volume_id(host, 2222, "ada"));
    assert_eq!(
        place.volume_id, sftp.id,
        "the account IS the place for a one-place protocol"
    );
    assert!(place.pinned);
    assert!(!place.connected, "nothing is registered under that id");
}

/// ❗ **An SMB host lists NO places and cannot be pinned in this effort.**
///
/// `known_shares.rs` stores no share rows (its only writer leaves `share_name`
/// empty) and carries no port, and a mounted share's id comes from `statfs`,
/// which normalizes an mDNS name to an IP. So no id derivable from the store
/// would match the mounted volume, and a pin here would point at nothing. SMB
/// places keep reaching the switcher as mounted volumes.
#[test]
fn an_smb_host_lists_no_places_and_is_never_pinned() {
    let smb_host = "192.0.2.35";

    let servers = saved_servers_of(vec![manual_entry(smb_host)]);
    let smb = find(&servers, smb_host);

    assert!(smb.places.is_empty());
    assert!(!smb.pinned);
    assert!(smb.username.is_none(), "an SMB host is not an account yet");
    assert!(
        smb.auto_reconnect.is_none(),
        "SMB has no \"Reconnect automatically\" switch, so there is no row checkbox to fill"
    );
}

/// ❗ **An account nobody named is listed by its derived label, and says so**:
/// the hub shows `username@host`, and knows no person chose it.
#[test]
fn an_unnamed_account_is_listed_by_its_derived_label_as_a_stand_in() {
    let sftp_host = "192.0.2.64";
    let mut unnamed_sftp = sftp_entry(sftp_host, true);
    unnamed_sftp.display_name = String::new();
    sftp_known_servers::remember(unnamed_sftp);
    let webdav_host = "192.0.2.65";
    let mut unnamed_webdav = webdav_entry(webdav_host, true);
    unnamed_webdav.display_name = String::new();
    webdav_known_servers::remember(unnamed_webdav);

    let servers = saved_servers_of(Vec::new());

    for label in [format!("ada@{sftp_host}"), format!("ada@{webdav_host}")] {
        let server = find(&servers, &label);
        assert_eq!(server.name_source, ServerNameSource::Fallback, "{label}");
        assert_eq!(server.places[0].name, label, "the place carries the same label");
    }
}

// ── The outcome mapping ──────────────────────────────────────────────

/// Every SFTP outcome lands on its superset twin.
#[test]
fn every_sftp_outcome_maps_to_its_superset_twin() {
    use cmdr_sftp::transport::{HostKeyPrompt, HostKeyPromptKind};

    let prompt = HostKeyPrompt {
        host: "nas.local".to_string(),
        port: 22,
        algorithm: "ssh-ed25519".to_string(),
        fingerprint: "SHA256:abc".to_string(),
        kind: HostKeyPromptKind::Unknown,
    };
    let cases = [
        (
            SftpConnection::Connected {
                volume_id: "sftp-x".to_string(),
            },
            ServerConnectOutcome::Connected {
                volume_id: "sftp-x".to_string(),
            },
        ),
        (
            SftpConnection::NeedsHostKeyApproval(prompt.clone()),
            ServerConnectOutcome::NeedsHostKeyApproval(prompt),
        ),
        (
            SftpConnection::HostKeyRevoked {
                algorithm: "ssh-rsa".to_string(),
                fingerprint: "SHA256:def".to_string(),
            },
            ServerConnectOutcome::HostKeyRevoked(SftpHostKeyIdentity {
                algorithm: "ssh-rsa".to_string(),
                fingerprint: "SHA256:def".to_string(),
            }),
        ),
        (
            SftpConnection::AuthenticationRejected,
            ServerConnectOutcome::AuthenticationRejected,
        ),
        (SftpConnection::NeedsCredentials, ServerConnectOutcome::NeedsCredentials),
        (SftpConnection::TimedOut, ServerConnectOutcome::TimedOut),
        (SftpConnection::Unreachable, ServerConnectOutcome::Unreachable),
        (SftpConnection::Cancelled, ServerConnectOutcome::Cancelled),
    ];
    for (from, expected) in cases {
        assert_eq!(format!("{:?}", outcome_from_sftp(from)), format!("{expected:?}"));
    }
}

/// Every WebDAV outcome lands on its superset twin.
///
/// ❗ `AuthMethodUnsupported` stays itself rather than collapsing into
/// `AuthenticationRejected`: a Digest-only server never saw the password, so
/// "check your password" is the wrong fix to offer.
#[test]
fn every_webdav_outcome_maps_to_its_superset_twin_including_the_unsupported_scheme() {
    let cases = [
        (
            WebdavConnection::Connected {
                volume_id: "webdav-x".to_string(),
            },
            ServerConnectOutcome::Connected {
                volume_id: "webdav-x".to_string(),
            },
        ),
        (
            WebdavConnection::AuthenticationRejected,
            ServerConnectOutcome::AuthenticationRejected,
        ),
        (
            WebdavConnection::NeedsCredentials,
            ServerConnectOutcome::NeedsCredentials,
        ),
        (
            WebdavConnection::AuthMethodUnsupported,
            ServerConnectOutcome::AuthMethodUnsupported,
        ),
        (
            WebdavConnection::CertificateUntrusted,
            ServerConnectOutcome::CertificateUntrusted,
        ),
        (
            WebdavConnection::NotAWebdavServer,
            ServerConnectOutcome::NotAWebdavServer,
        ),
        (WebdavConnection::TimedOut, ServerConnectOutcome::TimedOut),
        (WebdavConnection::Unreachable, ServerConnectOutcome::Unreachable),
        (WebdavConnection::Cancelled, ServerConnectOutcome::Cancelled),
    ];
    for (from, expected) in cases {
        assert_eq!(format!("{:?}", outcome_from_webdav(from)), format!("{expected:?}"));
    }
}

// ── Pins ─────────────────────────────────────────────────────────────

/// A pin round-trips through the family and tells the switcher to redraw.
#[test]
fn pinning_a_place_round_trips_and_republishes_the_volume_list() {
    let _recorder = crate::volume_broadcast::recorder_test_lock();
    let host = "192.0.2.36";
    sftp_known_servers::remember(sftp_entry(host, true));
    let volume_id = cmdr_fs::volume::sftp_volume_id(host, 2222, "ada");
    let pinned_now = || {
        sftp_known_servers::all()
            .into_iter()
            .find(|e| e.host == host)
            .expect("the entry stays saved")
            .pinned
    };

    let before = crate::volume_broadcast::volumes_changed_requests();
    assert!(set_place_pinned_inner(&volume_id, false));
    assert!(!pinned_now(), "the unpin reached the store");
    assert!(
        crate::volume_broadcast::volumes_changed_requests() > before,
        "❗ the switcher only learns a row left it from a `volumes-changed`"
    );

    assert!(set_place_pinned_inner(&volume_id, true));
    assert!(pinned_now(), "and back");
}

/// An id nothing saved answers no, rather than pretending.
#[test]
fn pinning_an_unknown_place_is_a_plain_no() {
    assert!(!set_place_pinned_inner("sftp-nothing-was-ever-saved-here", true));
}

// ── Reconnect automatically ──────────────────────────────────────────

/// ❗ **The row menu's "Reconnect automatically" moves that one field and
/// nothing else**, and the listing (which both row menus read) carries it back.
/// The pin is the field a read-modify-`remember` would get wrong, so it's the
/// one checked for collateral.
#[test]
fn switching_reconnect_automatically_on_an_sftp_place_round_trips_through_the_listing() {
    let _recorder = crate::volume_broadcast::recorder_test_lock();
    let host = "192.0.2.71";
    sftp_known_servers::remember(sftp_entry(host, false));
    let volume_id = cmdr_fs::volume::sftp_volume_id(host, 2222, "ada");
    let listed = || find(&saved_servers_of(Vec::new()), &format!("{host} over ssh")).clone();
    assert_eq!(listed().auto_reconnect, Some(true));

    let before = crate::volume_broadcast::volumes_changed_requests();
    assert!(set_place_auto_reconnect_inner(&volume_id, false));
    assert_eq!(listed().auto_reconnect, Some(false), "the switch reached the store");
    assert!(!listed().pinned, "and left the pin where it was");
    assert!(
        crate::volume_broadcast::volumes_changed_requests() > before,
        "❗ the hub re-reads the saved list on `volumes-changed`, so an open hub repaints"
    );

    assert!(set_place_auto_reconnect_inner(&volume_id, true));
    assert_eq!(listed().auto_reconnect, Some(true), "and back");
}

/// The WebDAV store has its own writer; the same round trip through it.
#[test]
fn switching_reconnect_automatically_on_a_webdav_place_round_trips_through_the_listing() {
    let _recorder = crate::volume_broadcast::recorder_test_lock();
    let host = "192.0.2.72";
    webdav_known_servers::remember(webdav_entry(host, true));
    let volume_id = cmdr_fs::volume::webdav_volume_id(host, 8080, "ada");
    let listed = || find(&saved_servers_of(Vec::new()), &format!("{host} over dav")).clone();

    assert!(set_place_auto_reconnect_inner(&volume_id, false));
    assert_eq!(listed().auto_reconnect, Some(false));
    assert!(listed().pinned, "the pin stays put");
}

/// An id nothing saved answers no: a one-shot connection has nothing to persist.
#[test]
fn switching_reconnect_automatically_on_an_unknown_place_is_a_plain_no() {
    assert!(!set_place_auto_reconnect_inner(
        "sftp-nothing-was-ever-saved-here",
        false
    ));
}

// ── Connecting a saved place ─────────────────────────────────────────

/// An id nothing saved is a typed refusal, ❌ never a dial into the dark.
#[tokio::test]
async fn connecting_a_place_nothing_saved_is_a_typed_refusal() {
    let refusal = connect_saved_place(
        "sftp-nothing-was-ever-saved-here".to_string(),
        "attempt".to_string(),
        None,
        None,
    )
    .await
    .expect_err("an unsaved id has nothing to dial");
    assert!(matches!(refusal, SavedPlaceRefusal::NoSuchServer { .. }));
}

/// ❗ **A registered volume is never re-dialed.** Re-dialing would register a
/// second volume under the same id; a session that dropped is mended by
/// `reconnect_volume_with_credentials`, which is what enforces the read-only
/// username rule and the never-seeds rule.
#[tokio::test]
async fn connecting_a_place_that_is_already_registered_is_refused() {
    let host = "192.0.2.37";
    sftp_known_servers::remember(sftp_entry(host, true));
    let volume_id = cmdr_fs::volume::sftp_volume_id(host, 2222, "ada");

    let manager = crate::file_system::volume::manager::get_volume_manager();
    manager.register(
        &volume_id,
        std::sync::Arc::new(cmdr_fs::volume::InMemoryVolume::new("stand-in")),
    );

    let refusal = connect_saved_place(volume_id.clone(), "attempt".to_string(), None, None)
        .await
        .expect_err("a registered volume is mended, never re-dialed");
    assert!(matches!(refusal, SavedPlaceRefusal::AlreadyConnected { .. }));

    manager.unregister(&volume_id);
}

// ── Forgetting a server ──────────────────────────────────────────────

/// ❗ **A forgotten server is GONE, and the pane hears so first.**
///
/// `VolumeUnmounted` is what redirects a pane standing on the place;
/// `volumes-changed` is what takes the row out of the store. The other order
/// leaves the pane on a volume nothing can name, which is the bug this cell
/// exists to keep out. Asserting on the recorded GENERATION rather than on
/// wall-clock order is what makes it un-flaky.
#[tokio::test]
#[allow(
    clippy::await_holding_lock,
    reason = "the lock serializes the process-global broadcast recorders for the whole cell; holding it across the await IS the point"
)]
async fn forgetting_a_server_tells_the_panes_before_it_takes_the_row_away() {
    let _recorder = crate::volume_broadcast::recorder_test_lock();
    let host = "192.0.2.41";
    sftp_known_servers::remember(sftp_entry(host, true));
    let volume_id = cmdr_fs::volume::sftp_volume_id(host, 2222, "ada");

    let before = crate::volume_broadcast::volumes_changed_requests();
    assert!(forget_server(volume_id.clone()).await, "the entry was there");

    assert!(
        !sftp_known_servers::all().iter().any(|e| e.host == host),
        "the entry left the store"
    );
    let (gone_id, gone_at) = crate::volume_broadcast::last_volume_gone().expect("the panes were told");
    assert_eq!(gone_id, volume_id);
    assert_eq!(
        gone_at, before,
        "❗ the gone event went out BEFORE anything asked for a republish"
    );
    assert!(
        crate::volume_broadcast::volumes_changed_requests() > before,
        "and the republish followed, so the row leaves the switcher"
    );
}

/// An id nothing saved answers no and tells nobody: a spurious `VolumeUnmounted`
/// would send a pane home for no reason.
#[tokio::test]
#[allow(
    clippy::await_holding_lock,
    reason = "the lock serializes the process-global broadcast recorders for the whole cell; holding it across the await IS the point"
)]
async fn forgetting_a_server_nothing_saved_is_a_plain_no() {
    let _recorder = crate::volume_broadcast::recorder_test_lock();
    let before = crate::volume_broadcast::last_volume_gone();
    assert!(!forget_server("sftp-nothing-was-ever-saved-here".to_string()).await);
    assert_eq!(
        crate::volume_broadcast::last_volume_gone(),
        before,
        "nothing was forgotten, so nothing was announced"
    );
}

/// ❗ **Disconnecting a place tells the panes too**, or a pane keeps a volume id
/// the registry no longer answers for and every listing on it fails instead of
/// redirecting. Unlike a forget, the ROW survives (it becomes `saved`).
#[tokio::test]
#[allow(
    clippy::await_holding_lock,
    reason = "the lock serializes the process-global broadcast recorders for the whole cell; holding it across the await IS the point"
)]
async fn disconnecting_a_place_that_has_no_session_announces_nothing() {
    let _recorder = crate::volume_broadcast::recorder_test_lock();
    let host = "192.0.2.42";
    sftp_known_servers::remember(sftp_entry(host, true));
    let volume_id = cmdr_fs::volume::sftp_volume_id(host, 2222, "ada");

    let before = crate::volume_broadcast::last_volume_gone();
    assert!(
        !disconnect_place(volume_id).await,
        "a saved-but-not-connected place has no session to drop"
    );
    assert_eq!(
        crate::volume_broadcast::last_volume_gone(),
        before,
        "and nothing was announced, so no pane goes home for nothing"
    );
}

// ── Saving without dialing, and the start folder ─────────────────────

fn sftp_target(host: &str, port: u16, username: &str, start_folder: Option<&str>) -> ServerTarget {
    ServerTarget::Sftp {
        display_name: "Edited".to_string(),
        host: host.to_string(),
        port,
        username: username.to_string(),
        remote_root: "/srv/data".to_string(),
        start_folder: start_folder.map(str::to_string),
        key_file: None,
        use_agent: false,
        auto_reconnect: true,
    }
}

fn webdav_target(url: &str, username: &str, start_folder: Option<&str>) -> ServerTarget {
    ServerTarget::Webdav {
        display_name: "Edited".to_string(),
        url: url.to_string(),
        username: username.to_string(),
        remote_root: "/Photos".to_string(),
        start_folder: start_folder.map(str::to_string),
        auto_reconnect: true,
    }
}

/// ❗ **A start folder outside the root is refused, and NOTHING is written**:
/// the stored entry keeps every field it had, the name included.
#[tokio::test]
async fn saving_a_start_folder_outside_the_root_is_refused_and_writes_nothing() {
    let host = "192.0.2.61";
    sftp_known_servers::remember(sftp_entry(host, true));

    let outcome = update_saved_server(sftp_target(host, 2222, "ada", Some("/srv/data-1"))).await;

    assert_eq!(outcome, SavedServerOutcome::StartFolderOutsideRoot);
    let stored = sftp_known_servers::find(host, 2222, "ada").expect("the entry stays saved");
    assert_eq!(
        stored.display_name,
        format!("{host} over ssh"),
        "not even the name moved"
    );
    assert_eq!(stored.start_folder, None);
}

/// The start folder is stored normalized, and the root itself stores as none,
/// so one landing has one spelling.
#[tokio::test]
async fn saving_a_start_folder_under_the_root_stores_it_normalized() {
    let host = "192.0.2.62";
    sftp_known_servers::remember(sftp_entry(host, true));
    let stored = || sftp_known_servers::find(host, 2222, "ada").expect("the entry stays saved");

    let deeper = update_saved_server(sftp_target(host, 2222, "ada", Some("/srv/data/photos/"))).await;
    assert_eq!(deeper, SavedServerOutcome::Saved);
    assert_eq!(stored().start_folder.as_deref(), Some("/srv/data/photos"));
    assert_eq!(stored().display_name, "Edited");

    let at_root = update_saved_server(sftp_target(host, 2222, "ada", Some("/srv/data"))).await;
    assert_eq!(at_root, SavedServerOutcome::Saved);
    assert_eq!(stored().start_folder, None);
}

/// ❗ **A saved edit republishes the volume list, whatever it changed.** The
/// switcher's name and landing come from that list, and neither an unconnected
/// place's edit nor a start-folder-only one moves anything in the registry that
/// would announce it. The hub re-reads the saved list on the same broadcast. A
/// refusal wrote nothing, so it asks for nothing.
#[tokio::test]
#[allow(
    clippy::await_holding_lock,
    reason = "the lock serializes the process-global broadcast recorders for the whole cell; holding it across the await IS the point"
)]
async fn saving_an_edit_republishes_the_volume_list_and_a_refusal_does_not() {
    let _recorder = crate::volume_broadcast::recorder_test_lock();
    let host = "192.0.2.66";
    sftp_known_servers::remember(sftp_entry(host, true));

    let before = crate::volume_broadcast::volumes_changed_requests();
    let refused = update_saved_server(sftp_target(host, 2222, "ada", Some("/srv/data-1"))).await;
    assert_eq!(refused, SavedServerOutcome::StartFolderOutsideRoot);
    assert_eq!(
        crate::volume_broadcast::volumes_changed_requests(),
        before,
        "a refusal changed nothing, so nothing is republished"
    );

    let saved = update_saved_server(sftp_target(host, 2222, "ada", Some("/srv/data/photos"))).await;
    assert_eq!(saved, SavedServerOutcome::Saved);
    assert!(
        crate::volume_broadcast::volumes_changed_requests() > before,
        "❗ without it the switcher keeps the old name and landing until something else republishes"
    );
}

/// WebDAV takes the same rule through the same family, so the frontend branches
/// on protocol nowhere new.
#[tokio::test]
async fn saving_a_webdav_start_folder_outside_the_root_is_refused_and_writes_nothing() {
    let host = "192.0.2.63";
    webdav_known_servers::remember(webdav_entry(host, true));
    let url = format!("http://{host}:8080/dav/");

    let outcome = update_saved_server(webdav_target(&url, "ada", Some("/Documents"))).await;

    assert_eq!(outcome, SavedServerOutcome::StartFolderOutsideRoot);
    let stored = webdav_known_servers::find(&url, "ada").expect("the entry stays saved");
    assert_eq!(
        stored.display_name,
        format!("{host} over dav"),
        "not even the name moved"
    );
}

/// ❗ **An add with a start folder outside the root is refused BEFORE dialing**,
/// so nothing is registered and nothing is saved. Port 1 on loopback refuses
/// connections, so a dial that slipped through would answer `Unreachable`.
#[tokio::test]
async fn connecting_with_a_start_folder_outside_the_root_is_refused_before_dialing() {
    let _secrets = crate::test_support::isolate_secrets();
    let username = "start-folder-refusal";

    let sftp = connect_server(
        sftp_target("127.0.0.1", 1, username, Some("/srv")),
        "start-folder-refusal-sftp".to_string(),
        None,
    )
    .await;
    assert!(
        matches!(sftp, ServerConnectOutcome::StartFolderOutsideRoot),
        "got {sftp:?}"
    );
    assert!(
        sftp_known_servers::find("127.0.0.1", 1, username).is_none(),
        "nothing was saved"
    );

    let url = "http://127.0.0.1:1/dav/";
    let webdav = connect_server(
        webdav_target(url, username, Some("/Documents")),
        "start-folder-refusal-webdav".to_string(),
        None,
    )
    .await;
    assert!(
        matches!(webdav, ServerConnectOutcome::StartFolderOutsideRoot),
        "got {webdav:?}"
    );
    assert!(webdav_known_servers::find(url, username).is_none(), "nothing was saved");
}

/// ❗ A URL that isn't `http`(s) is answered before anything is dialed: the
/// WebDAV arm parses the address first, so a garbled scheme never reaches the
/// wiring and nothing is registered or saved.
#[tokio::test]
async fn connecting_a_webdav_target_with_a_non_http_url_answers_invalid_url_without_dialing() {
    let username = "invalid-url";
    let url = "ftp://127.0.0.1/dav/";

    let outcome = connect_server(
        webdav_target(url, username, None),
        "invalid-url-attempt".to_string(),
        None,
    )
    .await;

    assert!(matches!(outcome, ServerConnectOutcome::InvalidUrl), "got {outcome:?}");
    assert!(webdav_known_servers::find(url, username).is_none(), "nothing was saved");
}

/// An id nothing saved has no secret to forget, so the menu item stays off
/// rather than offering to revoke a credential that was never stored.
#[tokio::test]
async fn an_unsaved_id_has_no_remembered_secret() {
    assert!(!has_server_secret("sftp-nothing-was-ever-saved-here".to_string()).await);
}

/// A saved share row, as a mount through Cmdr files it.
fn share_row(
    server_name: &str,
    share: &str,
    username: Option<&str>,
    volume_id: Option<&str>,
) -> known_shares::KnownNetworkShare {
    known_shares::KnownNetworkShare {
        server_name: server_name.to_string(),
        share_name: share.to_string(),
        protocol: "smb".to_string(),
        last_connected_at: "2026-09-24T10:00:00Z".to_string(),
        last_connection_mode: known_shares::ConnectionMode::Credentials,
        last_known_auth_options: known_shares::AuthOptions::GuestOrCredentials,
        username: username.map(str::to_string),
        address: volume_id.map(|_| server_name.to_string()),
        port: None,
        volume_id: volume_id.map(str::to_string),
        mount_path: volume_id.map(|_| format!("/Volumes/{share}")),
        pinned: volume_id.is_some(),
    }
}

/// ❗ **A saved share is a place under its host** (cmdr-reports#7: "user +
/// server + share"), carrying the account it opens as and the id its last mount
/// had, so the hub can open it and a pin points at the real volume.
#[test]
fn a_saved_share_is_a_place_under_its_host_with_its_account() {
    let smb_host = "192.0.2.46";
    let servers = saved_servers(
        vec![named_manual_entry(smb_host, "Sven's NAS")],
        vec![share_row(smb_host, "Container", Some("sven"), Some("smb-container"))],
        &[],
    );

    let host = find(&servers, "Sven's NAS");
    assert_eq!(host.places.len(), 1);
    let place = &host.places[0];
    assert_eq!(place.volume_id, "smb-container");
    assert_eq!(place.name, "Container");
    assert_eq!(place.username.as_deref(), Some("sven"));
    assert_eq!(place.app_root, "/Volumes/Container");
    assert!(place.pinned);
    assert_eq!(host.last_connected_at.as_deref(), Some("2026-09-24T10:00:00Z"));
}

/// A share on a host nobody typed in still gets a host row to hang under.
#[test]
fn a_saved_share_on_a_host_nobody_typed_in_brings_its_host() {
    let servers = saved_servers(
        Vec::new(),
        vec![share_row("192.0.2.47", "photos", None, Some("smb-photos"))],
        &[],
    );

    let host = find(&servers, "192.0.2.47");
    assert_eq!(host.protocol, ServerProtocol::Smb);
    assert_eq!(host.places.len(), 1);
    assert_eq!(host.places[0].username, None, "a guest share has no account");
}

/// A share an Add named but nothing mounted yet is listed with a stand-in id and
/// its `smb://` address, which is how the hub knows to open it through the host.
#[test]
fn a_share_no_mount_went_through_is_listed_by_its_address() {
    let servers = saved_servers(
        vec![named_manual_entry("192.0.2.48", "Office")],
        vec![share_row("192.0.2.48", "Scans", Some("ada"), None)],
        &[],
    );

    let place = &find(&servers, "Office").places[0];
    assert_eq!(place.app_root, "smb://192.0.2.48/Scans");
    assert!(!place.pinned);
}
