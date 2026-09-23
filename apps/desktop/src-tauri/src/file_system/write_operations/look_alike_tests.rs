//! The look-alike finder: form-only matches, exact names, and when it lists.

use std::path::Path;

use super::*;
use crate::file_system::volume::InMemoryVolume;

fn entry(name: &str) -> FileEntry {
    FileEntry::new(name.to_string(), format!("/dir/{name}"), false, false)
}

#[test]
fn one_entry_spelled_in_another_form_is_the_look_alike() {
    let entries = [entry("caf\u{e9}.txt"), entry("other.txt")];
    match among(&entries, "cafe\u{301}.txt") {
        LookAlike::One(found) => assert_eq!(found.name, "caf\u{e9}.txt"),
        other => panic!("expected the composed entry, got {other:?}"),
    }
}

#[test]
fn an_exact_name_answers_for_itself() {
    let entries = [entry("caf\u{e9}.txt"), entry("cafe\u{301}.txt")];
    assert!(matches!(among(&entries, "cafe\u{301}.txt"), LookAlike::None));
}

#[test]
fn a_name_differing_in_case_is_not_a_look_alike() {
    let entries = [entry("Report.docx")];
    assert!(matches!(among(&entries, "report.docx"), LookAlike::None));
}

#[test]
fn two_look_alikes_and_no_exact_name_is_several() {
    // "élő" three ways: fully composed, fully decomposed, and half of each.
    let entries = [entry("\u{e9}l\u{151}"), entry("e\u{301}lo\u{30b}")];
    assert!(matches!(among(&entries, "\u{e9}lo\u{30b}"), LookAlike::Several));
}

#[tokio::test]
async fn a_byte_exact_volume_lists_to_find_the_look_alike() {
    let volume = InMemoryVolume::new("v");
    volume.create_directory(Path::new("/dir")).await.unwrap();
    volume.create_file(Path::new("/dir/caf\u{e9}.txt"), b"x").await.unwrap();
    match look_alike_in(&volume, Path::new("/dir"), "cafe\u{301}.txt").await {
        Ok(LookAlike::One(found)) => assert_eq!(found.path, "/dir/caf\u{e9}.txt"),
        other => panic!("expected the composed entry, got {other:?}"),
    }
}

#[tokio::test]
async fn a_folder_that_is_not_there_holds_no_look_alike() {
    let volume = InMemoryVolume::new("v");
    assert!(matches!(
        look_alike_in(&volume, Path::new("/nowhere"), "cafe\u{301}.txt").await,
        Ok(LookAlike::None)
    ));
}
