//! An S3 account's places as the hub lists them: grouped under one account row,
//! each with its own id and pin, and the account's id the one its root names.

use super::super::outcomes::outcome_from_s3;
use super::super::wire::{ServerConnectOutcome, ServerNameSource, ServerProtocol};
use super::{s3_accounts, s3_place};
use crate::network::s3_known_places::{self, KnownS3Place, S3ProviderChoice};
use crate::network::s3_volume_wiring::S3Connection;

fn place(key: &str, bucket: Option<&str>, name: &str, pinned: bool) -> KnownS3Place {
    KnownS3Place {
        provider: S3ProviderChoice::Wasabi {
            region: "eu-central-1".to_string(),
        },
        access_key_id: key.to_string(),
        bucket: bucket.map(str::to_string),
        display_name: name.to_string(),
        auto_reconnect: true,
        pinned,
        last_connected_at: "2026-10-01T00:00:00Z".to_string(),
    }
}

fn listed(key: &str) -> Vec<super::SavedServer> {
    let manager = crate::file_system::volume::manager::get_volume_manager();
    let app_roots = crate::server_volumes::server_places()
        .into_iter()
        .map(|place| (place.id, place.app_root))
        .collect();
    s3_accounts(manager, &app_roots)
        .into_iter()
        .filter(|account| account.username.as_deref() == Some(key))
        .collect()
}

#[test]
fn an_accounts_buckets_list_as_places_under_one_row() {
    // A key no other cell uses: the store is process-global.
    let key = "AKIAGROUPED";
    s3_known_places::remember(place(key, Some("photos"), "", true));
    s3_known_places::remember(place(key, Some("backups"), "Backups", false));

    let accounts = listed(key);
    assert_eq!(accounts.len(), 1, "one row per account");
    let account = &accounts[0];
    assert_eq!(account.protocol, ServerProtocol::S3);
    assert_eq!(account.address, "s3.eu-central-1.wasabisys.com");
    assert!(!account.pinned, "the account row carries no pin; its places do");
    assert_eq!(account.name_source, ServerNameSource::Fallback);
    let params = place(key, None, "", false).params().expect("valid");
    assert_eq!(account.id, s3_known_places::account_id(&params));

    let mut places: Vec<(&str, bool)> = account
        .places
        .iter()
        .map(|place| (place.name.as_str(), place.pinned))
        .collect();
    places.sort_unstable();
    assert_eq!(
        places,
        vec![("Backups", false), ("photos@s3.eu-central-1.wasabisys.com", true)]
    );
    assert!(
        account
            .places
            .iter()
            .all(|p| p.app_root.ends_with("/photos") || p.app_root.ends_with("/backups")),
        "each place is rooted at its bucket"
    );
}

#[test]
fn a_saved_root_names_its_account() {
    let key = "AKIANAMEDROOT";
    s3_known_places::remember(place(key, Some("photos"), "", true));
    s3_known_places::remember(place(key, None, "Work", true));

    let accounts = listed(key);
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].display_name, "Work");
    assert_eq!(accounts[0].name_source, ServerNameSource::User);
    assert_eq!(accounts[0].places.len(), 2);
}

#[test]
fn a_target_and_its_saved_entry_derive_one_id() {
    // The add form sends what the person typed; the store keys what a dial
    // trims. ❗ Both must land on one id, or "Add" saves a row "Add and open"
    // can't find.
    let typed = s3_place(
        S3ProviderChoice::Aws {
            region: "eu-west-1".to_string(),
        },
        " AKIATRIM ".to_string(),
        Some(" photos ".to_string()),
        String::new(),
        true,
    );
    let clean = place("AKIATRIM", Some("photos"), "", false);
    let clean = KnownS3Place {
        provider: S3ProviderChoice::Aws {
            region: "eu-west-1".to_string(),
        },
        ..clean
    };
    assert_eq!(typed.volume_id(), clean.volume_id());
}

#[test]
fn every_s3_outcome_keeps_its_own_superset_twin() {
    let cases = [
        (S3Connection::KeysRejected, "authentication_rejected"),
        (S3Connection::AccessDenied, "access_denied"),
        (S3Connection::BucketListRefused, "bucket_list_refused"),
        (S3Connection::BucketNotFound, "bucket_not_found"),
        (S3Connection::ClockSkewed, "clock_skewed"),
        (S3Connection::NotAnS3Endpoint, "not_an_s3_endpoint"),
        (S3Connection::InvalidProvider, "invalid_url"),
        (S3Connection::NeedsCredentials, "needs_credentials"),
        (S3Connection::CertificateUntrusted, "certificate_untrusted"),
    ];
    for (connection, wire) in cases {
        let outcome = outcome_from_s3(connection);
        let json = serde_json::to_value(&outcome).expect("serializes");
        assert_eq!(json["outcome"], wire, "{outcome:?}");
    }
    let region = outcome_from_s3(S3Connection::RegionMismatch {
        region: Some("us-west-2".to_string()),
    });
    assert!(matches!(region, ServerConnectOutcome::RegionMismatch { region: Some(ref r) } if r == "us-west-2"));
}
