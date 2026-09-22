//! `list_as_stored` against a byte-exact volume that knows a foreign spelling.

use std::path::Path;

use super::*;
use crate::file_system::listing::caching_test_support::SpelledVolume;
use crate::file_system::volume::InMemoryVolume;

/// `fotók` composed (how the server stores it) and decomposed (how the macOS
/// kernel mount spelled it for the pane).
const STORED: &str = "/fot\u{f3}k";
const FOREIGN: &str = "/foto\u{301}k";

async fn album() -> InMemoryVolume {
    let volume = InMemoryVolume::new("Share");
    volume
        .create_directory(Path::new(STORED))
        .await
        .expect("seed the album");
    volume
        .create_file(&Path::new(STORED).join("photo.jpg"), b"jpeg")
        .await
        .expect("seed a photo");
    volume
}

#[tokio::test]
async fn a_path_that_lists_as_given_never_asks_for_another_spelling() {
    let volume = SpelledVolume::new(album().await);

    let listed = list_as_stored(&volume, Path::new(STORED), None, None)
        .await
        .expect("the stored spelling lists");

    assert_eq!(listed.path, Path::new(STORED));
    assert_eq!(listed.stored_spelling_of(Path::new(STORED)), None);
    assert_eq!(volume.resolves(), 0, "the happy path costs nothing extra");
}

/// The pane asked for the kernel mount's spelling: the listing comes back under
/// the server's, and so does every child, so the next click inside is exact.
#[tokio::test]
async fn a_foreign_spelling_lists_the_stored_directory_under_its_stored_path() {
    let volume = SpelledVolume::new(album().await).resolving(FOREIGN, Ok(Some(STORED)));

    let listed = list_as_stored(&volume, Path::new(FOREIGN), None, None)
        .await
        .expect("the foreign spelling resolves and lists");

    assert_eq!(listed.path, Path::new(STORED));
    assert_eq!(listed.stored_spelling_of(Path::new(FOREIGN)).as_deref(), Some(STORED));
    let child = &listed.entries.first().expect("the photo is listed").path;
    assert!(
        child.starts_with(STORED),
        "{child} must carry the stored spelling of its directory"
    );
}

/// No other spelling found: the pane gets the listing's own `NotFound`, which is
/// what drives its "this folder is gone" walk-up.
#[tokio::test]
async fn a_miss_with_no_other_spelling_keeps_its_not_found() {
    let volume = SpelledVolume::new(album().await);

    let result = list_as_stored(&volume, Path::new("/gone"), None, None).await;

    assert!(
        matches!(result, Err(VolumeError::NotFound(_))),
        "got {:?}",
        result.err()
    );
    assert_eq!(volume.resolves(), 1);
}

/// Two look-alikes: the refusal reaches the pane as itself, never a guess.
#[tokio::test]
async fn a_look_alike_refusal_reaches_the_pane() {
    let volume =
        SpelledVolume::new(album().await).resolving(FOREIGN, Err(VolumeError::AmbiguousName(FOREIGN.to_string())));

    let result = list_as_stored(&volume, Path::new(FOREIGN), None, None).await;

    assert!(
        matches!(result, Err(VolumeError::AmbiguousName(_))),
        "got {:?}",
        result.err()
    );
}

/// A pane open in an accented folder when the share moves from the kernel mount
/// to a direct connection holds the kernel's spelling of the folder and of every
/// entry. The respell re-reads it through the new backend: the listing moves to
/// the stored spelling, so the watcher's reports (in server bytes) find it, and
/// its entries are replaced by ones the new backend can open.
#[tokio::test]
async fn a_backend_swap_respells_an_open_listing_and_its_entries() {
    use crate::file_system::listing::caching::find_listings_for_path_on_volume;
    use crate::file_system::listing::caching_test_support::TestListing;
    use crate::file_system::volume::manager::get_volume_manager;

    let volume_id = format!("respell-{}", uuid::Uuid::new_v4());
    let volume = SpelledVolume::new(album().await).resolving(FOREIGN, Ok(Some(STORED)));
    get_volume_manager().register(&volume_id, std::sync::Arc::new(volume));
    let kernel_entry = FileEntry::new("photo.jpg".to_string(), format!("{FOREIGN}/photo.jpg"), false, false);
    let listing = TestListing::new()
        .volume(&volume_id)
        .path(FOREIGN)
        .entries(vec![kernel_entry])
        .insert("respell");

    respell_listing(volume_id.clone(), listing.id().to_string(), FOREIGN.into()).await;
    let found_by_server_bytes = find_listings_for_path_on_volume(Some(&volume_id), Path::new(STORED));
    get_volume_manager().unregister(&volume_id);

    listing.with_listing(|cached| {
        assert_eq!(cached.path.as_path(), Path::new(STORED));
        let paths: Vec<&str> = cached.entries().iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![format!("{STORED}/photo.jpg")],
            "entries carry the stored bytes"
        );
    });
    assert_eq!(
        found_by_server_bytes.len(),
        1,
        "a change reported in the server's bytes must reach the open pane"
    );
}

/// Only a miss is a spelling question: any other refusal is what it is.
#[tokio::test]
async fn a_refusal_other_than_a_miss_is_not_resolved() {
    let volume = SpelledVolume::new(album().await)
        .resolving(FOREIGN, Ok(Some(STORED)))
        .listings_refused_with(VolumeError::PermissionDenied(FOREIGN.to_string()));

    let result = list_as_stored(&volume, Path::new(FOREIGN), None, None).await;

    assert!(
        matches!(result, Err(VolumeError::PermissionDenied(_))),
        "got {:?}",
        result.err()
    );
    assert_eq!(volume.resolves(), 0, "a refusal is not a spelling question");
}
