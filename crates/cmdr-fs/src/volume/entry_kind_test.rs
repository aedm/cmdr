//! `EntryKind`: the link bit wins over the directory bit, and the trait default
//! reads it off `get_metadata`, so a backend whose metadata marks its links gets
//! a link-aware answer without an override.

use std::path::Path;

use super::EntryKind;
use crate::entry::FileEntry;
use crate::volume::{InMemoryVolume, Volume};

fn entry(is_directory: bool, is_symlink: bool) -> FileEntry {
    FileEntry::new("album".to_string(), "/album".to_string(), is_directory, is_symlink)
}

#[test]
fn a_link_to_a_folder_is_a_link_not_a_directory() {
    assert_eq!(EntryKind::of(&entry(true, true)), EntryKind::Symlink);
    assert_eq!(EntryKind::of(&entry(false, true)), EntryKind::Symlink);
    assert_eq!(EntryKind::of(&entry(true, false)), EntryKind::Directory);
    assert_eq!(EntryKind::of(&entry(false, false)), EntryKind::File);
}

#[tokio::test]
async fn the_trait_default_answers_from_get_metadata() {
    let volume = InMemoryVolume::new("V");
    volume.create_directory(Path::new("/dir")).await.unwrap();
    volume.create_file(Path::new("/dir/a.txt"), b"a").await.unwrap();

    assert_eq!(
        volume.entry_kind(Path::new("/dir")).await.unwrap(),
        EntryKind::Directory
    );
    assert_eq!(
        volume.entry_kind(Path::new("/dir/a.txt")).await.unwrap(),
        EntryKind::File
    );
    assert!(volume.entry_kind(Path::new("/missing")).await.is_err());
}
