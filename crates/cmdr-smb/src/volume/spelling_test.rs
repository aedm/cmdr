//! The matching rule and the correction cache, with no server.
//!
//! The walk itself needs a server that stores names as sent, so it's pinned by
//! the Docker cells in `unicode_names_integration_test.rs`.

use super::*;

/// `fotók` composed, and the same word decomposed (`o` + U+0301).
const NFC: &str = "fot\u{f3}k";
const NFD: &str = "foto\u{301}k";

#[test]
fn a_decomposed_name_finds_its_composed_entry() {
    assert_eq!(
        match_component(["other", NFC], NFD),
        ComponentMatch::Folded(NFC.to_string())
    );
}

#[test]
fn a_composed_name_finds_its_decomposed_entry() {
    assert_eq!(
        match_component(["other", NFD], NFC),
        ComponentMatch::Folded(NFD.to_string())
    );
}

#[test]
fn a_name_in_another_case_finds_its_entry() {
    assert_eq!(
        match_component(["Google Photos"], "GOOGLE PHOTOS"),
        ComponentMatch::Folded("Google Photos".to_string())
    );
    // Case and form at once: what a typed path to an accented folder looks like.
    assert_eq!(
        match_component([NFC], "FOTO\u{301}K"),
        ComponentMatch::Folded(NFC.to_string())
    );
}

/// The exact bytes are the entry the caller named, even beside a look-alike:
/// picking the look-alike is the wrong file.
#[test]
fn an_exact_match_wins_over_look_alikes() {
    assert_eq!(match_component([NFC, NFD], NFD), ComponentMatch::Exact);
    assert_eq!(match_component([NFD, NFC], NFC), ComponentMatch::Exact);
}

/// Two twins and no exact match: refuse, never guess.
#[test]
fn two_look_alikes_and_no_exact_match_are_ambiguous() {
    assert_eq!(match_component([NFC, NFD], "FOT\u{d3}K"), ComponentMatch::Ambiguous);
    assert_eq!(match_component(["Photo", "PHOTO"], "photo"), ComponentMatch::Ambiguous);
}

#[test]
fn nothing_alike_is_absent() {
    assert_eq!(match_component(["a", "b"], NFC), ComponentMatch::Absent);
    assert_eq!(match_component([], NFC), ComponentMatch::Absent);
}

/// The ASCII fast path must agree with the full fold, or an ASCII name and its
/// uppercase twin would stop matching.
#[test]
fn the_ascii_shortcut_folds_like_the_full_path() {
    assert_eq!(fold("readme.txt"), "readme.txt");
    assert_eq!(fold("README.txt"), "readme.txt");
    assert_eq!(fold(NFD), fold(NFC));
}

#[test]
fn a_remembered_correction_comes_back_for_its_directory_only() {
    let cache = SpellingCache::default();
    cache.remember("album", NFD, NFC);

    assert_eq!(cache.get("album", NFD).as_deref(), Some(NFC));
    assert_eq!(cache.get("other", NFD), None);
    assert_eq!(
        cache.get("album", NFC),
        None,
        "keyed on the foreign bytes, not the fold"
    );
}

/// A change in a directory can remove the entry a correction names or plant a
/// twin beside it, so the watcher drops that directory's corrections, and only
/// that directory's.
#[test]
fn a_change_in_a_directory_forgets_its_corrections() {
    let cache = SpellingCache::default();
    cache.remember("album", NFD, NFC);
    cache.remember("album/sub", NFD, NFC);

    cache.forget_dir("album");

    assert_eq!(cache.get("album", NFD), None);
    assert_eq!(cache.get("album/sub", NFD).as_deref(), Some(NFC));
}

#[test]
fn the_cache_stays_bounded() {
    let cache = SpellingCache::default();
    for n in 0..(REMEMBERED_CORRECTIONS + 50) {
        cache.remember(&format!("dir{n}"), NFD, NFC);
    }
    assert_eq!(cache.len(), REMEMBERED_CORRECTIONS);
    assert_eq!(cache.get("dir0", NFD), None, "the oldest goes first");
    assert!(cache.get(&format!("dir{}", REMEMBERED_CORRECTIONS + 49), NFD).is_some());
}

/// Forgetting and relearning must not let the eviction order grow past the cap.
#[test]
fn forgotten_corrections_do_not_pile_up_in_the_eviction_order() {
    let cache = SpellingCache::default();
    for n in 0..(10 * REMEMBERED_CORRECTIONS) {
        let dir = format!("dir{n}");
        cache.remember(&dir, NFD, NFC);
        cache.forget_dir(&dir);
    }
    assert!(cache.inner.lock_ignore_poison().order.len() <= 2 * REMEMBERED_CORRECTIONS + 1);
}
