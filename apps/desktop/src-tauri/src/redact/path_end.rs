//! Where a path found in prose really ends. The path branches over-match on purpose (spaces are legal in
//! labels and filenames), and this gives the trailing prose back. `DETAILS.md` § "Finding the end of a path".

use super::names::ends_with_unicode_escape;
use super::paths::{ends_filename, ends_sentence, has_extension_like_suffix};

/// Split a greedy path capture into (path, trailing_noise). The regex matches spaces inside
/// paths so both multi-word labels (`/Volumes/My Backup Drive/...`) and multi-word filenames
/// (`Invoice for Acme Corp.pdf`) land whole; that also sweeps up trailing English text like
/// `... .png now` or `... .rs:42:5`. We pull back the tail here before the rewriter runs,
/// then re-emit the tail verbatim in the dispatch output.
///
/// ❗ **This is the only thing standing between a filename and an uploaded bundle.** The
/// regex deliberately over-matches, so a boundary rule that gives back too much doesn't
/// merely lose prose, it publishes the part of the name it handed back. Both directions have
/// bitten: an over-eager capture once truncated 98 reports at `/Volumes/<volume>`, and an
/// over-cautious one shipped ` at 01.13.03 PM-2.jpeg` verbatim.
///
/// Trimmed, in order:
/// - everything from the first `": "`, the `{path}: {message}` seam nearly every caller
///   formats with. macOS forbids `:` in a filename, so this can't cut a real name short.
/// - everything after the first token that ENDS the path: one carrying a letter-led
///   extension (`report.pdf`, `PM-2.jpeg`) or ending a sentence (`naspi-1)`, `state.`).
/// - trailing sentence-ending punctuation (`,`, `;`, `!`, `?`, `)`, `]`, `}`)
/// - trailing `:<digits>` groups (line/column markers like `:42:5`)
/// - a trailing RUN of space-separated words that are lowercase-initial AND carry no
///   extension (` failed to open`). The run never eats into the first segment after the
///   last `/`, which is what keeps `/Volumes/naspi and then it failed` down to `naspi`. It
///   never runs when the path reaches the seam: the seam already said where it ends.
pub(super) fn split_trailing_noise(s: &str) -> (&str, &str) {
    split_trailing_noise_with(s, true)
}

/// [`split_trailing_noise`], optionally without its last rule (the lowercase prose run). A keyed
/// field's value is a path by declaration, so a lowercase last word there (`Medical records`)
/// is part of the name: trimming it split one folder into a token plus a bare word, and gave
/// the same folder a different token than its quoted spelling on the next line.
pub(super) fn split_trailing_noise_with(s: &str, trim_prose_run: bool) -> (&str, &str) {
    let bytes = s.as_bytes();
    let mut end = bytes.len();

    // The `{path}: {message}` seam. Everything from it belongs to the message.
    let seam = s.find(": ");
    if let Some(seam) = seam {
        end = seam;
    }

    // Left to right: the first token that ends a filename or ends a sentence is the last
    // thing that can belong to the path. Scanning forwards is what separates
    // `report.pdf for alice@example.com` (cut after `report.pdf`) from
    // `Screenshot 2026-09-04 at 01.13.03 PM-2.jpeg` (no letter-led extension until the very
    // end, so the whole name stays). A backwards scan can't tell those apart: the email's
    // `.com` looks exactly like a filename extension from the right.
    {
        let mut offset = 0usize;
        for token in s[..end].split(' ') {
            let token_end = offset + token.len();
            if !token.is_empty() && (ends_filename(token) || ends_sentence(token)) {
                end = token_end;
                break;
            }
            offset = token_end + 1; // step over the space
        }
    }

    // First: trim sentence-ending punctuation, one at a time.
    end = trim_closing_punctuation(s, end);

    // Repeatedly strip `:<digits>` suffixes (e.g. `:42`, `:42:5`).
    loop {
        let mut i = end;
        // consume digits from the right
        while i > 0 && bytes[i - 1].is_ascii_digit() {
            i -= 1;
        }
        if i < end && i > 0 && bytes[i - 1] == b':' {
            end = i - 1;
        } else {
            break;
        }
    }

    // Trim a RUN of trailing lowercase, extension-less words (a sentence continuation).
    //
    // Not when the path runs right up to the seam: then the seam already marks its end, and
    // a lowercase last word is part of a name (`/Volumes/x/summer trip: failed`). Trimming it
    // there shipped `trip` verbatim.
    //
    // The floor is the first word after the last `/`: that word is a real path segment
    // however lowercase it looks, so `/Volumes/naspi and then it failed` keeps `naspi`.
    // An extension ends the run on the spot, because a word carrying one is part of the
    // filename, not prose — that is what holds `my secret notes.txt` together.
    if trim_prose_run && seam != Some(end) {
        let floor = s[..end].rfind('/').map_or(0, |i| i + 1);
        loop {
            let mut i = end;
            while i > floor && bytes[i - 1] != b' ' {
                i -= 1;
            }
            // `i` is the start of the last word; require a space before it, and stay
            // above the floor so we never eat the segment itself.
            if i <= floor || i == end || bytes[i - 1] != b' ' {
                break;
            }
            let word = &s[i..end];
            let starts_lower = word.chars().next().is_some_and(|c| c.is_ascii_lowercase());
            if !starts_lower || has_extension_like_suffix(word) || looks_like_name_fragment(word) {
                break;
            }
            end = i - 1;
            while end > floor && bytes[end - 1] == b' ' {
                end -= 1;
            }
        }
    }

    // Finally, strip a trailing `.` or `,` that was exposed by the above steps.
    end = trim_closing_punctuation(s, end);

    // SAFETY: we only advance `end` on ASCII byte boundaries.
    (&s[..end], &s[end..])
}

/// Step `end` back over closing punctuation (`,` `;` `!` `?` `)` `]` `}`), one at a time.
/// The `}` closing a `\u{301}` escape stays: it's the middle of a `{:?}`-printed name.
fn trim_closing_punctuation(s: &str, mut end: usize) -> usize {
    let bytes = s.as_bytes();
    while end > 0 {
        let b = bytes[end - 1];
        if b == b'}' && ends_with_unicode_escape(&s[..end]) {
            break;
        }
        if matches!(b, b',' | b';' | b'!' | b'?' | b')' | b']' | b'}') {
            end -= 1;
        } else {
            break;
        }
    }
    end
}

/// A word prose doesn't produce: an inner dot (`me\u{301}retek.jpg.cmdr-tmp-…`, whose
/// extension is too odd for [`has_extension_like_suffix`]) or a `{:?}` escape. Such a word
/// is the tail of a filename, and trimming it as prose ships it verbatim.
fn looks_like_name_fragment(word: &str) -> bool {
    word.contains('\\') || word.find('.').is_some_and(|i| i > 0 && i + 1 < word.len())
}
