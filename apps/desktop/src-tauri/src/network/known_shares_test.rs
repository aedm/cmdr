//! Unit tests for `known_shares.rs`: the store's keying, the username hint, and the
//! per-share direct-connection switch.

use super::*;

/// Tests that mutate the global `KNOWN_SHARES` static must hold this lock
/// to prevent cross-test interference (Rust runs tests in parallel).
static SERIAL: Mutex<()> = Mutex::new(());

fn naspolya() -> NetworkHost {
    NetworkHost {
        id: "naspolya-smb-tcp-local".to_string(),
        name: "Naspolya".to_string(),
        hostname: Some("Naspolya.local".to_string()),
        ip_address: Some("192.168.1.111".to_string()),
        port: 445,
        source: crate::network::HostSource::Discovered,
    }
}

/// A store saved before the setting existed has no opt-out list at all, and
/// every share in it must stay on the fast connection: a missing field can't be
/// what switches the behavior off.
#[test]
fn a_store_saved_before_the_setting_existed_leaves_every_share_on_the_fast_connection() {
    let old = r#"{"knownNetworkShares":[{"serverName":"Naspolya","shareName":"naspi","protocol":"smb","lastConnectedAt":"2026-01-06T12:00:00Z","lastConnectionMode":"guest","lastKnownAuthOptions":"guest_only","username":null}]}"#;

    let store: KnownSharesStore = serde_json::from_str(old).expect("an old store still loads");

    assert_eq!(store.known_network_shares.len(), 1, "the old rows survive");
    assert!(!is_opted_out(
        &store.direct_connection_opt_outs,
        &["Naspolya"],
        "naspi",
        &[]
    ));
}

/// `statfs` echoes whichever name form each mount used, so one NAS arrives as
/// an IP on one mount and as its mDNS service name on the next. A choice made
/// under one spelling that the other can't see looks like the toggle resetting
/// itself.
#[test]
fn an_opt_out_holds_under_every_name_form_of_its_server() {
    let hosts = [naspolya()];
    let mut opt_outs = Vec::new();

    apply_choice(&mut opt_outs, "192.168.1.111", "naspi", false, &hosts);

    for form in [
        "192.168.1.111",
        "Naspolya._smb._tcp.local",
        "naspolya.local",
        "Naspolya",
    ] {
        assert!(
            is_opted_out(&opt_outs, &[form], "naspi", &hosts),
            "the opt-out is invisible under {form:?}"
        );
    }
    // The share name folds case and NFC, the way `share_key` does.
    assert!(is_opted_out(&opt_outs, &["Naspolya"], "NASPI", &hosts));
    // Another share on the same server keeps its own answer, and so does another server.
    assert!(!is_opted_out(&opt_outs, &["Naspolya"], "Multimedia", &hosts));
    assert!(!is_opted_out(&opt_outs, &["raspberrypi.local"], "naspi", &hosts));
}

/// The auto path knows a mount's server twice over (the `statfs` spelling and the
/// address it's about to dial), and either one matching is enough.
#[test]
fn any_of_the_callers_server_names_can_match() {
    let hosts = [naspolya()];
    let mut opt_outs = Vec::new();
    apply_choice(&mut opt_outs, "Naspolya._smb._tcp.local", "naspi", false, &hosts);

    assert!(is_opted_out(
        &opt_outs,
        &["somewhere-else", "192.168.1.111"],
        "naspi",
        &hosts
    ));
}

/// Choosing again under another spelling replaces the entry rather than adding a
/// second, and turning it back on clears it under any spelling.
#[test]
fn one_share_has_one_entry_whatever_it_is_called() {
    let hosts = [naspolya()];
    let mut opt_outs = Vec::new();

    apply_choice(&mut opt_outs, "192.168.1.111", "naspi", false, &hosts);
    apply_choice(&mut opt_outs, "Naspolya._smb._tcp.local", "naspi", false, &hosts);
    assert_eq!(opt_outs.len(), 1, "one share, one entry: {opt_outs:?}");

    apply_choice(&mut opt_outs, "naspolya.local", "NASPI", true, &hosts);
    assert!(opt_outs.is_empty(), "turning it back on clears it: {opt_outs:?}");
    assert!(!is_opted_out(&opt_outs, &["192.168.1.111"], "naspi", &hosts));
}

#[test]
fn test_share_key() {
    assert_eq!(share_key("MyNAS", "Documents"), "mynas/documents");
    assert_eq!(share_key("server.local", "Media"), "server.local/media");
}

/// One accented share reaches this store composed (the frontend's share list)
/// and decomposed (`statfs` on the mount), so a byte-keyed store remembers it
/// twice and the auth mode saved on one spelling is missing on the other.
/// Reported as ERR-ABXW4.
#[test]
fn share_key_folds_unicode_normalization() {
    assert_eq!(
        share_key("Szabolcs-DS224", "R\u{e9}gi NAS"),
        share_key("Szabolcs-DS224", "Re\u{301}gi NAS")
    );
}

#[test]
fn test_connection_mode_serialization() {
    let guest = ConnectionMode::Guest;
    let creds = ConnectionMode::Credentials;

    assert_eq!(serde_json::to_string(&guest).unwrap(), r#""guest""#);
    assert_eq!(serde_json::to_string(&creds).unwrap(), r#""credentials""#);

    let guest_back: ConnectionMode = serde_json::from_str(r#""guest""#).unwrap();
    assert_eq!(guest_back, ConnectionMode::Guest);
}

#[test]
fn test_auth_options_serialization() {
    let guest_only = AuthOptions::GuestOnly;
    let creds_only = AuthOptions::CredentialsOnly;
    let both = AuthOptions::GuestOrCredentials;

    assert_eq!(serde_json::to_string(&guest_only).unwrap(), r#""guest_only""#);
    assert_eq!(serde_json::to_string(&creds_only).unwrap(), r#""credentials_only""#);
    assert_eq!(serde_json::to_string(&both).unwrap(), r#""guest_or_credentials""#);
}

#[test]
fn test_known_share_serialization() {
    let share = KnownNetworkShare {
        server_name: "Alpha".to_string(),
        share_name: "Documents".to_string(),
        protocol: "smb".to_string(),
        last_connected_at: "2026-01-03T21:00:00Z".to_string(),
        last_connection_mode: ConnectionMode::Credentials,
        last_known_auth_options: AuthOptions::GuestOrCredentials,
        username: Some("david".to_string()),
    };

    let json = serde_json::to_string_pretty(&share).unwrap();
    assert!(json.contains(r#""serverName": "Alpha""#));
    assert!(json.contains(r#""shareName": "Documents""#));
    assert!(json.contains(r#""lastConnectionMode": "credentials""#));
    assert!(json.contains(r#""lastKnownAuthOptions": "guest_or_credentials""#));
    assert!(json.contains(r#""username": "david""#));

    // Round-trip
    let parsed: KnownNetworkShare = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.server_name, "Alpha");
    assert_eq!(parsed.share_name, "Documents");
    assert_eq!(parsed.last_connection_mode, ConnectionMode::Credentials);
}

#[test]
fn test_store_serialization() {
    let store = KnownSharesStore {
        known_network_shares: vec![
            KnownNetworkShare {
                server_name: "Alpha".to_string(),
                share_name: "Documents".to_string(),
                protocol: "smb".to_string(),
                last_connected_at: "2026-01-03T21:00:00Z".to_string(),
                last_connection_mode: ConnectionMode::Credentials,
                last_known_auth_options: AuthOptions::GuestOrCredentials,
                username: Some("david".to_string()),
            },
            KnownNetworkShare {
                server_name: "Bravo".to_string(),
                share_name: "media".to_string(),
                protocol: "smb".to_string(),
                last_connected_at: "2026-01-02T15:30:00Z".to_string(),
                last_connection_mode: ConnectionMode::Guest,
                last_known_auth_options: AuthOptions::GuestOnly,
                username: None,
            },
        ],
        direct_connection_opt_outs: Vec::new(),
    };

    let json = serde_json::to_string_pretty(&store).unwrap();
    assert!(json.contains("knownNetworkShares"));

    // Round-trip
    let parsed: KnownSharesStore = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.known_network_shares.len(), 2);
}

#[test]
fn test_in_memory_operations() {
    let _guard = SERIAL.lock().unwrap();
    // Test the in-memory cache operations directly
    let cache = get_known_shares_mutex();

    // Clear any previous state
    if let Ok(mut c) = cache.lock() {
        c.known_network_shares.clear();
    }

    // Get all should return empty
    let all = get_all_known_shares();
    assert!(all.is_empty());

    // Add a share directly to cache (simulating update without app handle)
    if let Ok(mut c) = cache.lock() {
        c.known_network_shares.push(KnownNetworkShare {
            server_name: "TestServer".to_string(),
            share_name: "TestShare".to_string(),
            protocol: "smb".to_string(),
            last_connected_at: "2026-01-06T12:00:00Z".to_string(),
            last_connection_mode: ConnectionMode::Guest,
            last_known_auth_options: AuthOptions::GuestOnly,
            username: None,
        });
    }

    // Should find it now
    let found = get_known_share("TestServer", "TestShare");
    assert!(found.is_some());
    assert_eq!(found.unwrap().share_name, "TestShare");

    // Case-insensitive lookup
    let found_lower = get_known_share("testserver", "testshare");
    assert!(found_lower.is_some());

    // Clean up
    if let Ok(mut c) = cache.lock() {
        c.known_network_shares.clear();
    }
}

#[test]
fn test_username_hints() {
    let _guard = SERIAL.lock().unwrap();
    let cache = get_known_shares_mutex();

    // Clear and set up test data
    if let Ok(mut c) = cache.lock() {
        c.known_network_shares.clear();
        c.known_network_shares.push(KnownNetworkShare {
            server_name: "Server1".to_string(),
            share_name: "Share1".to_string(),
            protocol: "smb".to_string(),
            last_connected_at: "2026-01-06T12:00:00Z".to_string(),
            last_connection_mode: ConnectionMode::Credentials,
            last_known_auth_options: AuthOptions::CredentialsOnly,
            username: Some("alice".to_string()),
        });
        c.known_network_shares.push(KnownNetworkShare {
            server_name: "Server2".to_string(),
            share_name: "Share2".to_string(),
            protocol: "smb".to_string(),
            last_connected_at: "2026-01-06T12:00:00Z".to_string(),
            last_connection_mode: ConnectionMode::Guest,
            last_known_auth_options: AuthOptions::GuestOnly,
            username: None,
        });
    }

    assert_eq!(get_username_hint("Server1"), Some("alice".to_string()));
    assert_eq!(get_username_hint("Server2"), None); // No username for guest-only
    assert_eq!(get_username_hint("nobody-here"), None);

    // Clean up
    if let Ok(mut c) = cache.lock() {
        c.known_network_shares.clear();
    }
}

/// The hint has to survive the server arriving under another of its names. The
/// login form opens on whatever `NetworkHost` discovery produced (an mDNS instance
/// name), while the share was saved under the name the connect used, so a lookup
/// that compared raw strings prefilled nothing for the exact case it exists for.
#[test]
fn a_username_hint_is_found_under_every_name_form_of_its_server() {
    let _guard = SERIAL.lock().unwrap();
    let cache = get_known_shares_mutex();

    if let Ok(mut c) = cache.lock() {
        c.known_network_shares.clear();
        c.known_network_shares.push(KnownNetworkShare {
            server_name: "Naspolya".to_string(),
            share_name: "naspi".to_string(),
            protocol: "smb".to_string(),
            last_connected_at: "2026-01-06T12:00:00Z".to_string(),
            last_connection_mode: ConnectionMode::Credentials,
            last_known_auth_options: AuthOptions::CredentialsOnly,
            username: Some("david".to_string()),
        });
    }

    for form in [
        "Naspolya",
        "naspolya",
        "Naspolya.local",
        "naspolya.local.",
        "Naspolya._smb._tcp.local",
    ] {
        assert_eq!(
            get_username_hint(form),
            Some("david".to_string()),
            "no hint found for the server spelled {form:?}"
        );
    }
    // A different server keeps its own answer.
    assert_eq!(get_username_hint("raspberrypi.local"), None);

    if let Ok(mut c) = cache.lock() {
        c.known_network_shares.clear();
    }
}

/// Shares are appended in connect order, so the LAST one carrying a username is the
/// most recent thing the person actually signed in as.
#[test]
fn the_newest_username_on_a_server_wins() {
    let _guard = SERIAL.lock().unwrap();
    let cache = get_known_shares_mutex();

    if let Ok(mut c) = cache.lock() {
        c.known_network_shares.clear();
        for (share, user) in [("old", Some("alice")), ("newer", Some("bob")), ("guest", None)] {
            c.known_network_shares.push(KnownNetworkShare {
                server_name: "Naspolya".to_string(),
                share_name: share.to_string(),
                protocol: "smb".to_string(),
                last_connected_at: "2026-01-06T12:00:00Z".to_string(),
                last_connection_mode: ConnectionMode::Credentials,
                last_known_auth_options: AuthOptions::CredentialsOnly,
                username: user.map(str::to_string),
            });
        }
    }

    assert_eq!(get_username_hint("naspolya"), Some("bob".to_string()));

    if let Ok(mut c) = cache.lock() {
        c.known_network_shares.clear();
    }
}

/// Concurrent threads adding distinct shares must not lose any writes.
#[test]
fn concurrent_in_memory_updates_no_lost_writes() {
    let _guard = SERIAL.lock().unwrap();
    let cache = get_known_shares_mutex();

    // Clear previous state
    if let Ok(mut c) = cache.lock() {
        c.known_network_shares.clear();
    }

    let thread_count = 20;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(thread_count));
    let mut handles = Vec::new();

    for i in 0..thread_count {
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait(); // All threads start at the same time
            let key = format!("server-{}", i);
            if let Ok(mut c) = get_known_shares_mutex().lock() {
                c.known_network_shares.push(KnownNetworkShare {
                    server_name: key.clone(),
                    share_name: "share".to_string(),
                    protocol: "smb".to_string(),
                    last_connected_at: "2026-01-01T00:00:00Z".to_string(),
                    last_connection_mode: ConnectionMode::Guest,
                    last_known_auth_options: AuthOptions::GuestOnly,
                    username: None,
                });
            }
        }));
    }

    for h in handles {
        h.join().expect("thread panicked");
    }

    let all = get_all_known_shares();
    assert_eq!(
        all.len(),
        thread_count,
        "Expected {} shares but got {}. A concurrent write was lost.",
        thread_count,
        all.len()
    );

    // Clean up
    if let Ok(mut c) = cache.lock() {
        c.known_network_shares.clear();
    }
}

/// Concurrent reads while another thread writes should not panic or return corrupt data.
#[test]
fn concurrent_read_during_write() {
    let _guard = SERIAL.lock().unwrap();
    let cache = get_known_shares_mutex();

    // Seed with initial data
    if let Ok(mut c) = cache.lock() {
        c.known_network_shares.clear();
        c.known_network_shares.push(KnownNetworkShare {
            server_name: "seed".to_string(),
            share_name: "share".to_string(),
            protocol: "smb".to_string(),
            last_connected_at: "2026-01-01T00:00:00Z".to_string(),
            last_connection_mode: ConnectionMode::Guest,
            last_known_auth_options: AuthOptions::GuestOnly,
            username: None,
        });
    }

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let mut handles = Vec::new();

    // Writer thread: adds shares
    let b = barrier.clone();
    handles.push(std::thread::spawn(move || {
        b.wait();
        for i in 0..50 {
            if let Ok(mut c) = get_known_shares_mutex().lock() {
                c.known_network_shares.push(KnownNetworkShare {
                    server_name: format!("writer-{}", i),
                    share_name: "share".to_string(),
                    protocol: "smb".to_string(),
                    last_connected_at: "2026-01-01T00:00:00Z".to_string(),
                    last_connection_mode: ConnectionMode::Guest,
                    last_known_auth_options: AuthOptions::GuestOnly,
                    username: None,
                });
            }
        }
    }));

    // Two reader threads: read all shares repeatedly
    for _ in 0..2 {
        let b = barrier.clone();
        handles.push(std::thread::spawn(move || {
            b.wait();
            for _ in 0..100 {
                let shares = get_all_known_shares();
                // Must always have at least the seed share
                assert!(!shares.is_empty(), "Read returned empty during concurrent write");
            }
        }));
    }

    for h in handles {
        h.join().expect("thread panicked");
    }

    // Clean up
    if let Ok(mut c) = cache.lock() {
        c.known_network_shares.clear();
    }
}

/// Rapid sequential updates to the same share should keep the last value.
#[test]
fn rapid_sequential_updates_same_share() {
    let _guard = SERIAL.lock().unwrap();
    let cache = get_known_shares_mutex();

    // Clear previous state
    if let Ok(mut c) = cache.lock() {
        c.known_network_shares.clear();
    }

    let iterations = 100;
    for i in 0..iterations {
        if let Ok(mut c) = cache.lock() {
            let key = share_key("rapid-server", "rapid-share");
            if let Some(existing) = c
                .known_network_shares
                .iter_mut()
                .find(|s| share_key(&s.server_name, &s.share_name) == key)
            {
                existing.last_connected_at = format!("2026-01-01T00:00:{:02}Z", i);
                existing.username = Some(format!("user-{}", i));
            } else {
                c.known_network_shares.push(KnownNetworkShare {
                    server_name: "rapid-server".to_string(),
                    share_name: "rapid-share".to_string(),
                    protocol: "smb".to_string(),
                    last_connected_at: format!("2026-01-01T00:00:{:02}Z", i),
                    last_connection_mode: ConnectionMode::Credentials,
                    last_known_auth_options: AuthOptions::GuestOrCredentials,
                    username: Some(format!("user-{}", i)),
                });
            }
        }
    }

    let share = get_known_share("rapid-server", "rapid-share").expect("share should exist");
    assert_eq!(share.username, Some(format!("user-{}", iterations - 1)));

    // Only one entry should exist (upsert, not duplicate)
    let all = get_all_known_shares();
    let rapid_count = all.iter().filter(|s| s.server_name == "rapid-server").count();
    assert_eq!(rapid_count, 1, "Rapid updates should not create duplicate entries");

    // Clean up
    if let Ok(mut c) = cache.lock() {
        c.known_network_shares.clear();
    }
}
