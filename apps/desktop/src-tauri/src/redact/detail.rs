//! External-text fields: OS, server, and CLI prose that producers log in full as `detail={:?}`,
//! `stderr={:?}`, or `stdout={:?}`. Local logs keep it (capped at 1 KiB by `cmdr_fs::log_detail`);
//! a report gets it redacted with the report's context and capped at
//! [`REPORT_DETAIL_MAX_CHARS`]. `DETAILS.md` § "External-text fields".

use super::context::TokenDomain;
use super::fields::identity_field_token;
use super::names::unescape_debug;
use super::paths::{has_extension_like_suffix, redact_leaf};
use super::{REPORT_DETAIL_MAX_CHARS, RedactionContext, identity_token, redact_with, redactor_regex, whole_len};
use regex::Captures;

/// Shortest keyed value worth scrubbing from prose by its spelling. Shorter ones (`tv`, `pi`)
/// are too likely to be part of an ordinary word.
const MIN_ECHOED_IDENTITY_CHARS: usize = 3;

/// A raw identity the line's own keyed fields name, paired with the token it gets there.
pub(super) struct EchoedIdentity {
    raw: String,
    token: String,
}

/// Collect the identities the line's keyed fields name (`host=`, `share=`, `user=`, a
/// `path=` leaf, …), so an external-text field on the same line can scrub them when the prose
/// repeats one bare: `tree connect failed: "Private Share"` has no path or key to catch it.
/// The external-text fields themselves are skipped: prose never teaches the scrubber a name.
pub(super) fn echoed_identities(line: &str, context: &RedactionContext) -> Vec<EchoedIdentity> {
    let mut found = Vec::new();
    for caps in redactor_regex().captures_iter(line) {
        let (raw, token) = if caps.name("identity_field").is_some() {
            let key = caps.name("if_key").map_or("", |m| m.as_str());
            let value = quoted_value(caps.name("if_value").map_or("", |m| m.as_str()));
            let raw = unescape_debug(value).into_owned();
            let token = identity_field_token(key, &raw, Some(context));
            (raw, token)
        } else if caps.name("account").is_some() {
            let value = quoted_value(caps.name("account_value").map_or("", |m| m.as_str()));
            let raw = unescape_debug(value).into_owned();
            let token = identity_token("user", TokenDomain::Userinfo, &raw, Some(context));
            (raw, token)
        } else if caps.name("path_field").is_some() {
            let value = caps.name("pf_value").map_or("", |m| m.as_str());
            if !value.starts_with('"') {
                continue;
            }
            let path = unescape_debug(quoted_value(value)).into_owned();
            let Some(leaf) = path.rsplit(['/', '\\']).find(|seg| !seg.is_empty()) else {
                continue;
            };
            let token = redact_leaf(leaf, has_extension_like_suffix(leaf), Some(context));
            (leaf.to_string(), token)
        } else {
            continue;
        };
        if raw != "None" && raw.chars().count() >= MIN_ECHOED_IDENTITY_CHARS {
            found.push(EchoedIdentity { raw, token });
        }
    }
    // Longest first, so a share named `Plans` can't split `Client Plans` before it's seen.
    found.sort_by_key(|identity| std::cmp::Reverse(identity.raw.len()));
    found
}

/// Strip `Some("…")` or `"…"` wrappers; anything else is returned as is.
fn quoted_value(raw: &str) -> &str {
    raw.strip_prefix("Some(\"")
        .and_then(|rest| rest.strip_suffix("\")"))
        .or_else(|| raw.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')))
        .unwrap_or(raw)
}

/// Rewrite one quoted external-text field. Returns (replacement, bytes consumed).
///
/// Works on the UNESCAPED text so a `\"` around a quoted name in the prose can't split a path
/// match and leave a bare quote behind, then escapes the result again with `{:?}`, which is
/// what keeps the field's closing quote exact however the cap falls. The ordinary scanner runs
/// first; bare repeats of the line's own keyed identities are scrubbed after it.
pub(super) fn redact_detail_field(
    caps: &Captures<'_>,
    context: &RedactionContext,
    echoed: &[EchoedIdentity],
) -> (String, usize) {
    let key = caps.name("df_key").map_or("", |m| m.as_str());
    let raw = caps.name("df_value").map_or("", |m| m.as_str());
    let text = unescape_debug(quoted_value(raw));

    let mut redacted = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        redacted.push_str(&redact_with(line, Some(context)));
    }
    for identity in echoed {
        redacted = replace_whole_word(&redacted, &identity.raw, &identity.token);
    }
    (format!("{key}={:?}", cap_chars(&redacted)), whole_len(caps))
}

/// Replace `needle` wherever it isn't glued to a neighboring letter or digit, so a share named
/// `Plans` leaves `Planscape` alone.
fn replace_whole_word(haystack: &str, needle: &str, replacement: &str) -> String {
    let mut out = String::with_capacity(haystack.len());
    let mut rest = haystack;
    while let Some(at) = rest.find(needle) {
        let before = rest[..at].chars().next_back();
        let after = rest[at + needle.len()..].chars().next();
        let glued = before.is_some_and(char::is_alphanumeric) || after.is_some_and(char::is_alphanumeric);
        out.push_str(&rest[..at]);
        out.push_str(if glued { needle } else { replacement });
        rest = &rest[at + needle.len()..];
    }
    out.push_str(rest);
    out
}

/// At most [`REPORT_DETAIL_MAX_CHARS`] chars, the last one `…` when anything was cut. Idempotent:
/// a capped value is exactly at the limit, so a second pass leaves it alone.
fn cap_chars(text: &str) -> String {
    if text.chars().count() <= REPORT_DETAIL_MAX_CHARS {
        return text.to_string();
    }
    let mut out: String = text.chars().take(REPORT_DETAIL_MAX_CHARS - 1).collect();
    out.push('…');
    out
}
