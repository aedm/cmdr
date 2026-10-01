//! What the S3 place list owes the hub: one entry per place, found again the
//! way the volume id finds it, and a pin a reconnect can't undo.
//!
//! In-memory only: these run before `load_known_s3_places` names a file, so
//! `save()` is a no-op. Each cell uses a key id of its own, since the store is
//! process-global.

use super::*;

fn place(key: &str, bucket: Option<&str>) -> KnownS3Place {
    KnownS3Place {
        provider: S3ProviderChoice::Aws {
            region: "eu-west-1".to_string(),
        },
        access_key_id: key.to_string(),
        bucket: bucket.map(str::to_string),
        display_name: String::new(),
        auto_reconnect: true,
        pinned: false,
        last_connected_at: "2026-10-01T10:00:00Z".to_string(),
    }
}

fn id_of(entry: &KnownS3Place) -> String {
    entry.volume_id().expect("a valid test provider")
}

#[test]
fn every_bucket_under_one_key_is_its_own_entry() {
    let photos = place("AKIAREMEMBER", Some("photos"));
    let root = place("AKIAREMEMBER", None);
    remember(photos.clone());
    remember(root.clone());
    assert!(find(&id_of(&photos)).is_some());
    assert!(find(&id_of(&root)).is_some());
    assert_ne!(id_of(&photos), id_of(&root));
}

#[test]
fn a_reconnect_cant_repin_a_place_the_user_unpinned() {
    let photos = place("AKIAPINS", Some("photos"));
    remember(KnownS3Place {
        pinned: true,
        ..photos.clone()
    });
    assert!(set_pinned(&id_of(&photos), false));
    remember(KnownS3Place {
        pinned: true,
        display_name: "Holiday".to_string(),
        ..photos.clone()
    });
    let stored = find(&id_of(&photos)).expect("still saved");
    assert!(!stored.pinned, "the stored pin wins on a replace");
    assert_eq!(stored.display_name, "Holiday", "everything else is the new entry");
}

#[test]
fn forgetting_one_place_leaves_its_siblings() {
    let photos = place("AKIAFORGET", Some("photos"));
    let backups = place("AKIAFORGET", Some("backups"));
    remember(photos.clone());
    remember(backups.clone());
    assert!(forget(&id_of(&photos)));
    assert!(find(&id_of(&photos)).is_none());
    assert!(find(&id_of(&backups)).is_some());
    assert!(!forget(&id_of(&photos)), "a second forget finds nothing");
}

#[test]
fn an_unnamed_place_reads_as_its_bucket_or_key_on_its_host() {
    assert_eq!(
        place("AKIALABEL", Some("photos")).label(),
        "photos@s3.eu-west-1.amazonaws.com"
    );
    assert_eq!(place("AKIALABEL", None).label(), "AKIALABEL@s3.eu-west-1.amazonaws.com");
}

#[test]
fn every_place_of_one_account_names_the_same_account() {
    let photos = place("AKIAACCOUNT", Some("photos")).params().expect("valid");
    let root = place("AKIAACCOUNT", None).params().expect("valid");
    assert_eq!(account_id(&photos), account_id(&root));
    assert_eq!(
        account_id(&root),
        place_id(&root),
        "the account id IS the root place's id"
    );
}

#[test]
fn a_store_from_disk_reads_its_provider_by_kind() {
    let stored = r#"{
      "knownS3Places": [
        {
          "provider": { "kind": "other", "endpoint": "http://127.0.0.1:14480", "region": null, "pathStyle": true },
          "accessKeyId": "GK1",
          "bucket": "cmdr-test",
          "displayName": "",
          "lastConnectedAt": "2026-10-01T10:00:00Z"
        }
      ]
    }"#;
    let store: KnownS3PlacesStore = serde_json::from_str(stored).expect("parses");
    let entry = &store.known_s3_places[0];
    assert!(entry.auto_reconnect, "a missing switch reads as on");
    assert!(!entry.pinned, "a missing pin reads as off");
    let params = entry.params().expect("a usable provider");
    assert_eq!(params.port(), 14480);
}
