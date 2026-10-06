//! Compare directories (⇧F2): which rows each pane marks against the other.

use super::caching_test_support::{TestListing, TestListingGuard};
use super::compare::{CompareDirectoriesMode, compare_directories};
use super::metadata::FileEntry;
use super::sorting::{DirectorySortMode, SortColumn, SortOrder, sort_entries};

fn file(dir: &str, name: &str, size: u64, modified: u64) -> FileEntry {
    FileEntry {
        size: Some(size),
        modified_at: Some(modified),
        ..FileEntry::new(name.to_string(), format!("{dir}/{name}"), false, false)
    }
}

fn folder(dir: &str, name: &str) -> FileEntry {
    FileEntry::new(name.to_string(), format!("{dir}/{name}"), true, false)
}

fn pane(tag: &str, dir: &str, entries: Vec<FileEntry>) -> TestListingGuard {
    let mut entries = entries;
    sort_entries(
        &mut entries,
        SortColumn::Name,
        SortOrder::Ascending,
        DirectorySortMode::LikeFiles,
    );
    TestListing::new()
        .path(dir)
        .include_hidden(false)
        .entries(entries)
        .insert(tag)
}

fn marked(
    left: &TestListingGuard,
    right: &TestListingGuard,
    mode: CompareDirectoriesMode,
) -> (Vec<String>, Vec<String>) {
    let result = compare_directories(left.id(), false, right.id(), false, mode).expect("both listings are cached");
    let names = |listing: &TestListingGuard, rows: &[usize]| -> Vec<String> {
        let entries = listing.entries();
        let shown: Vec<&FileEntry> = entries.iter().filter(|e| !e.is_hidden).collect();
        rows.iter().map(|&row| shown[row].name.clone()).collect()
    };
    (names(left, &result.left), names(right, &result.right))
}

const L: &str = "/test/compare/left";
const R: &str = "/test/compare/right";

#[test]
fn marks_files_missing_on_the_other_side_and_the_newer_copy_of_the_rest() {
    let left = pane(
        "compare-newer-l",
        L,
        vec![
            file(L, "only-left.txt", 1, 100),
            file(L, "newer-left.txt", 1, 2_000),
            file(L, "same.txt", 1, 1_000),
            file(L, "older-left.txt", 1, 1_000),
        ],
    );
    let right = pane(
        "compare-newer-r",
        R,
        vec![
            file(R, "only-right.txt", 1, 100),
            file(R, "newer-left.txt", 1, 1_000),
            file(R, "same.txt", 1, 1_000),
            file(R, "older-left.txt", 1, 2_000),
        ],
    );

    let (l, r) = marked(&left, &right, CompareDirectoriesMode::NewerAndMissing);

    assert_eq!(l, vec!["newer-left.txt", "only-left.txt"]);
    assert_eq!(r, vec!["older-left.txt", "only-right.txt"]);
}

#[test]
fn a_two_second_difference_counts_as_the_same_time() {
    // FAT and some network shares store time in 2 s steps, so a copy can read a
    // second or two off its original.
    let left = pane("compare-tolerance-l", L, vec![file(L, "a.txt", 1, 1_002)]);
    let right = pane("compare-tolerance-r", R, vec![file(R, "a.txt", 1, 1_000)]);

    assert_eq!(
        marked(&left, &right, CompareDirectoriesMode::NewerAndMissing),
        (vec![], vec![])
    );
}

#[test]
fn folders_are_left_alone() {
    let left = pane(
        "compare-folders-l",
        L,
        vec![folder(L, "only-left-dir"), folder(L, "both")],
    );
    let right = pane("compare-folders-r", R, vec![folder(R, "both")]);

    assert_eq!(
        marked(&left, &right, CompareDirectoriesMode::NewerAndMissing),
        (vec![], vec![])
    );
}

#[test]
fn a_folder_with_the_files_name_is_not_its_counterpart() {
    let left = pane("compare-kind-l", L, vec![file(L, "report", 1, 100)]);
    let right = pane("compare-kind-r", R, vec![folder(R, "report")]);

    assert_eq!(
        marked(&left, &right, CompareDirectoriesMode::NewerAndMissing).0,
        vec!["report"]
    );
}

#[test]
fn names_match_across_case_and_unicode_form() {
    // Decomposed on one side, composed and upper-cased on the other: one file to a
    // case-insensitive Mac, so neither side counts it missing.
    let left = pane("compare-fold-l", L, vec![file(L, "Cafe\u{301}.txt", 1, 1_000)]);
    let right = pane("compare-fold-r", R, vec![file(R, "CAF\u{c9}.txt", 1, 1_000)]);

    assert_eq!(
        marked(&left, &right, CompareDirectoriesMode::NewerAndMissing),
        (vec![], vec![])
    );
}

#[test]
fn a_name_two_files_share_by_folding_matches_only_exactly_and_both_ways() {
    // A case-sensitive volume holding `Report` and `report`, against `REPORT`:
    // folding can't say which one `REPORT` is, so neither direction pairs them.
    let left = pane(
        "compare-collide-l",
        L,
        vec![file(L, "Report", 1, 1_000), file(L, "report", 1, 1_000)],
    );
    let right = pane("compare-collide-r", R, vec![file(R, "REPORT", 1, 1_000)]);

    let (l, r) = marked(&left, &right, CompareDirectoriesMode::NewerAndMissing);

    assert_eq!(l, vec!["report", "Report"], "both, in the pane's name order");
    assert_eq!(r, vec!["REPORT"]);
}

#[test]
fn missing_mode_marks_only_what_the_other_side_lacks() {
    let left = pane(
        "compare-missing-l",
        L,
        vec![file(L, "only-left.txt", 1, 1), file(L, "newer.txt", 1, 9_000)],
    );
    let right = pane("compare-missing-r", R, vec![file(R, "newer.txt", 1, 1_000)]);

    assert_eq!(
        marked(&left, &right, CompareDirectoriesMode::Missing),
        (vec!["only-left.txt".to_string()], vec![])
    );
}

#[test]
fn size_mode_marks_both_copies_of_a_file_whose_size_differs() {
    let left = pane(
        "compare-size-l",
        L,
        vec![file(L, "grew.txt", 10, 1_000), file(L, "same.txt", 5, 1)],
    );
    let right = pane(
        "compare-size-r",
        R,
        vec![file(R, "grew.txt", 20, 9_000), file(R, "same.txt", 5, 9_000)],
    );

    assert_eq!(
        marked(&left, &right, CompareDirectoriesMode::SizeAndMissing),
        (vec!["grew.txt".to_string()], vec!["grew.txt".to_string()])
    );
}

#[test]
fn an_unknown_time_never_counts_as_newer() {
    let mut unknown = file(L, "a.txt", 1, 0);
    unknown.modified_at = None;
    let left = pane("compare-unknown-l", L, vec![unknown]);
    let right = pane("compare-unknown-r", R, vec![file(R, "a.txt", 1, 1)]);

    assert_eq!(
        marked(&left, &right, CompareDirectoriesMode::NewerAndMissing),
        (vec![], vec![])
    );
}

#[test]
fn compares_only_the_rows_the_panes_show() {
    // With hidden files off, a dotfile is nothing the pane shows, so it's never
    // marked, and its row numbers skip it.
    let left = pane(
        "compare-hidden-l",
        L,
        vec![file(L, ".hidden-only-left", 1, 1), file(L, "zz-only-left.txt", 1, 1)],
    );
    let right = pane("compare-hidden-r", R, vec![]);

    let result = compare_directories(
        left.id(),
        false,
        right.id(),
        false,
        CompareDirectoriesMode::NewerAndMissing,
    )
    .expect("both listings are cached");

    assert_eq!(result.left, vec![0], "row 0 of the pane is zz-only-left.txt");
}

#[test]
fn a_gone_listing_is_an_error_not_an_empty_answer() {
    let left = pane("compare-gone-l", L, vec![]);

    assert!(
        compare_directories(
            left.id(),
            false,
            "no-such-listing",
            false,
            CompareDirectoriesMode::Missing
        )
        .is_err()
    );
}
