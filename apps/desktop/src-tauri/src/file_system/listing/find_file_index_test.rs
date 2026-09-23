//! `find_file_index` for a name spelled another way than the listing holds it.
//!
//! A cursor name that didn't come from this listing (a path carried over from
//! the macOS kernel mount, MCP naming a file, a reveal from Finder) can spell an
//! accented name decomposed while the share stores it composed. The cursor
//! should still land on that row, and only when exactly one row answers.

use super::FileEntry;
use super::caching_test_support::{TestListing, TestListingGuard};
use super::operations::find_file_index;

fn listing(tag: &str, names: &[&str]) -> TestListingGuard {
    let entries = names
        .iter()
        .map(|name| FileEntry::new((*name).to_string(), format!("/album/{name}"), false, false))
        .collect();
    TestListing::new()
        .volume("test")
        .path("/album")
        .entries(entries)
        .insert(tag)
}

#[test]
fn an_exact_name_finds_its_row() {
    let listing = listing("find-exact", &["a.jpg", "r\u{e9}sz.jpg"]);

    assert_eq!(find_file_index(listing.id(), "r\u{e9}sz.jpg", true).unwrap(), Some(1));
}

#[test]
fn a_name_in_another_unicode_form_or_case_finds_its_one_look_alike() {
    let listing = listing("find-folded", &["a.jpg", "r\u{e9}sz.jpg"]);

    assert_eq!(find_file_index(listing.id(), "re\u{301}sz.jpg", true).unwrap(), Some(1));
    assert_eq!(find_file_index(listing.id(), "R\u{c9}SZ.JPG", true).unwrap(), Some(1));
}

/// The exact spelling wins even beside a look-alike: it's the row the caller named.
#[test]
fn an_exact_match_beats_a_look_alike() {
    let listing = listing("find-exact-first", &["re\u{301}sz.jpg", "r\u{e9}sz.jpg"]);

    assert_eq!(find_file_index(listing.id(), "r\u{e9}sz.jpg", true).unwrap(), Some(1));
}

/// Two look-alikes and no exact match: no guess, the caller's own fallback runs.
#[test]
fn two_look_alikes_and_no_exact_match_find_nothing() {
    let listing = listing("find-ambiguous", &["re\u{301}sz.jpg", "R\u{e9}sz.jpg"]);

    assert_eq!(find_file_index(listing.id(), "r\u{e9}sz.jpg", true).unwrap(), None);
}
