//! Quick filter: the pattern a pane narrows its rows to while the user types.
//!
//! The pane's rows are the entries [`visible_rows`](super::visible_rows) shows,
//! so the filter is one more input to THAT predicate, never a second filter
//! point: counts, ranges, selection indices, type-to-jump, and `directory-diff`
//! rows all agree on what a filtered pane is showing.
//!
//! Matching is Total Commander's: the pattern matches ANYWHERE in the name,
//! ignoring case and Unicode form (`cmdr_fs::name_fold`), and `*` / `?` stand
//! for any run of characters / any one character. So `rep` finds
//! `Annual report.pdf`, and `*.pdf` (or just `.pdf`) finds every PDF.

use std::borrow::Cow;

use cmdr_fs::name_fold::fold_name;

/// A non-empty quick-filter pattern, folded once so a match folds only the name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NameFilter {
    /// The pattern, case- and form-folded, as characters for the wildcard walk.
    pattern: Vec<char>,
    /// The same pattern as a string, for the wildcard-free fast path.
    folded: String,
    /// Whether the pattern holds a `*` or `?`.
    has_wildcards: bool,
}

impl NameFilter {
    /// The filter for `raw`, or `None` when there's nothing to filter by (an
    /// empty pattern, or one of only `*`s, matches every name).
    pub(crate) fn new(raw: &str) -> Option<Self> {
        if raw.chars().all(|c| c == '*') {
            return None;
        }
        let folded = fold_name(raw).into_owned();
        Some(Self {
            pattern: folded.chars().collect(),
            has_wildcards: folded.contains(['*', '?']),
            folded,
        })
    }

    /// Whether a row named `name` survives the filter.
    pub(crate) fn matches(&self, name: &str) -> bool {
        let name: Cow<'_, str> = fold_name(name);
        if !self.has_wildcards {
            return name.contains(self.folded.as_str());
        }
        let name: Vec<char> = name.chars().collect();
        contains_glob(&name, &self.pattern)
    }
}

/// Whether `pattern` (with `*` and `?`) matches some run of `text`: a glob with
/// an implied `*` on both ends. Iterative, backtracking only to the last `*`,
/// so it's linear-ish and never recurses on a hostile pattern.
fn contains_glob(text: &[char], pattern: &[char]) -> bool {
    // An implied leading `*`: every start position is the "last star".
    let (mut t, mut p) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = Some((0, 0));
    loop {
        if p == pattern.len() {
            // An implied trailing `*`: matching the whole pattern is enough.
            return true;
        }
        if pattern[p] == '*' {
            star = Some((p + 1, t));
            p += 1;
            continue;
        }
        if t < text.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            t += 1;
            p += 1;
            continue;
        }
        match star {
            Some((star_p, star_t)) if star_t < text.len() => {
                star = Some((star_p, star_t + 1));
                p = star_p;
                t = star_t + 1;
            }
            _ => return false,
        }
    }
}
