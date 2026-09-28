//! How a name was spelled on its way into the log: Cmdr's own temp suffixes and Rust's `{:?}` escapes. Both have to
//! be seen through, or one file reads as several (`DETAILS.md` § "Report-scoped token identity").

use std::borrow::Cow;

/// The staging suffixes Cmdr puts on names it's still writing. Their UUID tail carries no
/// PII, and keeping it lets a triager see which temp became which file.
pub(super) const CMDR_TEMP_SUFFIXES: &[&str] = &[".cmdr-tmp-", ".cmdr-temp-", ".cmdr-staging-"];

/// Split `photo.jpg.cmdr-tmp-3f2a…` into (`photo.jpg`, `.cmdr-tmp-3f2a…`). The tail must be
/// hex and dashes, so a user's own `notes.cmdr-tmp-plan.txt` doesn't qualify.
pub(super) fn split_cmdr_suffix(seg: &str) -> (&str, &str) {
    for marker in CMDR_TEMP_SUFFIXES {
        if let Some(at) = seg.find(marker)
            && at > 0
        {
            let id = &seg[at + marker.len()..];
            if !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
                return (&seg[..at], &seg[at..]);
            }
        }
    }
    (seg, "")
}

/// Undo Rust's `{:?}` string escapes (`\u{301}`, `\\`, `\"`, `\n`, …). Anything that isn't
/// a well-formed escape stays as written.
pub(super) fn unescape_debug(s: &str) -> Cow<'_, str> {
    if !s.contains('\\') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('\\') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let (decoded, used) = match after.chars().next() {
            Some('\\') => (Some('\\'), 1),
            Some('"') => (Some('"'), 1),
            Some('\'') => (Some('\''), 1),
            Some('n') => (Some('\n'), 1),
            Some('t') => (Some('\t'), 1),
            Some('r') => (Some('\r'), 1),
            Some('0') => (Some('\0'), 1),
            Some('u') => match after.strip_prefix("u{").and_then(|t| t.split_once('}')) {
                Some((hex, _)) if (1..=6).contains(&hex.len()) => {
                    match u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
                        Some(c) => (Some(c), hex.len() + 3),
                        None => (None, 0),
                    }
                }
                _ => (None, 0),
            },
            _ => (None, 0),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &after[used..];
            }
            None => {
                out.push('\\');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Whether `s` ends with the `}` of a `\u{…}` escape, which is part of a name, not the end
/// of a sentence.
pub(super) fn ends_with_unicode_escape(s: &str) -> bool {
    let Some(body) = s.strip_suffix('}') else { return false };
    body.rfind("\\u{").is_some_and(|at| {
        let hex = &body[at + 3..];
        (1..=6).contains(&hex.len()) && hex.chars().all(|c| c.is_ascii_hexdigit())
    })
}
