//! Keyed log fields (`path=`, `smb_path=`, `from=`, …): the only way to recognize a path with no mount prefix in
//! front of it. `DETAILS.md` § "Keyed path fields".

use super::RedactionContext;
use super::context::TokenDomain;
use super::identity_token;
use super::names::{split_cmdr_suffix, unescape_debug};
use super::paths::{dir_token, has_extension_like_suffix, is_safe_parent_dir, redact_leaf};
use super::redactor_regex;
use super::references::redact_host;
use super::whole_len;
use regex::Captures;
use std::borrow::Cow;

/// Path-branch groups: a value one of these claims from its first byte is an absolute path
/// they already know how to redact.
pub(super) const PATH_BRANCHES: &[&str] = &[
    "win_home",
    "unix_home",
    "unix_system",
    "volumes",
    "media",
    "remote_url",
    "unc",
    "url_userinfo",
];

/// Top-level directory names that say where a path lives without saying anything about
/// who owns it. Kept as the first segment of an absolute field value (`path=/private`).
pub(super) const SYSTEM_ROOTS: &[&str] = &[
    "Applications",
    "Library",
    "System",
    "Users",
    "Volumes",
    "bin",
    "cores",
    "dev",
    "etc",
    "home",
    "media",
    "mnt",
    "opt",
    "private",
    "sbin",
    "tmp",
    "usr",
    "var",
];

/// Rewrite a `key=value` path field. Returns (replacement, bytes consumed).
///
/// An absolute value one of the path branches recognizes is handed back: the replacement is
/// just `key=` (plus the opening quote), and `redact_with` resumes at the value, where that
/// branch claims it with its own prefix rules (`$HOME`, `/Volumes/<volume>`, …). Everything
/// else is walked here segment by segment, which is what reaches a share-relative path.
pub(super) fn redact_path_field(caps: &Captures<'_>, context: Option<&RedactionContext>) -> (String, usize) {
    let key = caps.name("pf_key").map_or("", |m| m.as_str());
    let raw = caps.name("pf_value").map_or("", |m| m.as_str());
    let quoted = raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"');
    let quote = if quoted { "\"" } else { "" };
    let head = format!("{key}={quote}");

    let value = if quoted {
        &raw[1..raw.len() - 1]
    } else {
        end_of_bare_value(raw)
    };
    if value.is_empty() || value == "None" || claimed_by_path_branch(value) {
        return (head.clone(), head.len());
    }

    // A quoted value is `{:?}` output: unescape it so `e\u{301}` is one character again (and
    // so a lone `\` in an escape isn't mistaken for a Windows separator).
    let unescaped = if quoted {
        unescape_debug(value)
    } else {
        Cow::Borrowed(value)
    };
    let redacted = redact_relative_path(&unescaped, context);
    let consumed = head.len() + value.len() + quote.len();
    (format!("{head}{redacted}{quote}"), consumed)
}

/// Rewrite one producer-owned identity field while retaining its key and optional wrapper.
pub(super) fn redact_identity_field(caps: &Captures<'_>, context: Option<&RedactionContext>) -> (String, usize) {
    let key = caps.name("if_key").map_or("", |m| m.as_str());
    let raw = caps.name("if_value").map_or("", |m| m.as_str());
    let (prefix, value, suffix) = if raw.starts_with("Some(\"") && raw.ends_with("\")") {
        ("Some(\"", &raw[6..raw.len() - 2], "\")")
    } else {
        ("\"", &raw[1..raw.len() - 1], "\"")
    };
    let value = unescape_debug(value);
    let token = match key {
        "host" | "server" => redact_host(&value, context),
        "share" => identity_token("share", TokenDomain::Volume, &value, context),
        "volumeId" => identity_token("volume-id", TokenDomain::VolumeId, &value, context),
        "serverId" => identity_token("server-id", TokenDomain::ServerId, &value, context),
        "deviceId" => identity_token("device-id", TokenDomain::DeviceId, &value, context),
        _ => value.into_owned(),
    };
    (format!("{key}={prefix}{token}{suffix}"), whole_len(caps))
}

/// Where an unquoted field value really ends. The regex takes the rest of the line; the value
/// stops at the first of: the `{path}: {message}` seam, a `, ` (the next field), or a
/// ` key=` (the next field in `smb2`'s space-separated `from=… to=…`). A `)` left over from
/// `fn(share=…, path=…)` goes too, unless the name opened it (`photo (1).jpg`).
///
/// ⚠️ A comma-space inside an unquoted name ends it early and the rest is handed back
/// unredacted. Only `{}`-printed values can hit that, which is why path fields in our own
/// code are printed with `{:?}`.
pub(super) fn end_of_bare_value(raw: &str) -> &str {
    let mut end = raw.len();
    if let Some(seam) = raw.find(": ") {
        end = seam;
    }
    if let Some(comma) = raw[..end].find(", ") {
        end = comma;
    }
    if let Some((space, _)) = raw[..end]
        .match_indices(' ')
        .find(|(i, _)| starts_with_field_key(&raw[i + 1..end]))
    {
        end = space;
    }
    let mut value = &raw[..end];
    while value.ends_with(')') && value.matches(')').count() > value.matches('(').count() {
        value = &value[..value.len() - 1];
    }
    value.trim_end()
}

/// `ident=` at the start of `s`.
pub(super) fn starts_with_field_key(s: &str) -> bool {
    let ident_len = s
        .char_indices()
        .take_while(|&(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
        .count();
    ident_len > 0 && s[ident_len..].starts_with('=')
}

/// Whether a path branch matches `value` from its very first byte.
pub(super) fn claimed_by_path_branch(value: &str) -> bool {
    redactor_regex().captures(value).is_some_and(|caps| {
        caps.get(0).is_some_and(|m| m.start() == 0) && PATH_BRANCHES.iter().any(|g| caps.name(g).is_some())
    })
}

/// Redact a path no branch has a prefix rule for: share-relative (`docs/a b.pdf`,
/// `docs\a b.pdf`), volume-relative (`/docs/a b.pdf`), or a bare name. Same shape rules as
/// every other path: the leaf keeps its extension, an allowlisted parent keeps its name, the
/// rest collapse. Segments that are already tokens pass through, which keeps it idempotent.
pub(super) fn redact_relative_path(value: &str, context: Option<&RedactionContext>) -> String {
    let sep = if value.contains('/') || !value.contains('\\') {
        '/'
    } else {
        '\\'
    };
    let segments: Vec<&str> = value.split(sep).collect();
    let absolute = value.starts_with(sep);
    let Some(leaf_idx) = segments.iter().rposition(|s| !s.is_empty()) else {
        return value.to_string();
    };
    let mut out = String::with_capacity(value.len());
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 {
            out.push(sep);
        }
        let keep = seg.is_empty() || is_redacted_segment(seg) || (absolute && i == 1 && SYSTEM_ROOTS.contains(seg));
        if keep {
            out.push_str(seg);
        } else if i == leaf_idx {
            out.push_str(&redact_leaf(seg, has_extension_like_suffix(seg), context));
        } else if i + 1 == leaf_idx && is_safe_parent_dir(seg) && (context.is_none() || *seg != "Downloads") {
            out.push_str(seg);
        } else {
            out.push_str(&dir_token(seg, context));
        }
    }
    out
}

/// Whether a path segment is already redacted (`<dir>`, `<file:ab12cd>.pdf`, `$HOME`) or
/// carries nothing to redact (`.`, `..`).
pub(super) fn is_redacted_segment(seg: &str) -> bool {
    if matches!(seg, "$HOME" | "~" | "." | "..") {
        return true;
    }
    let (seg, _) = split_cmdr_suffix(seg);
    let Some(rest) = seg.strip_prefix('<') else {
        return false;
    };
    let Some(close) = rest.find('>') else { return false };
    let (label, tail) = (&rest[..close], &rest[close + 1..]);
    let (kind, hash) = label.split_once(':').unwrap_or((label, ""));
    let kind_ok = !kind.is_empty() && kind.chars().all(|c| c.is_ascii_lowercase() || c == '-');
    let hash_ok =
        hash.is_empty() || ((hash.len() == 6 || hash.len() == 12) && hash.chars().all(|c| c.is_ascii_hexdigit()));
    let tail_ok = tail.is_empty()
        || tail
            .strip_prefix('.')
            .is_some_and(|ext| !ext.is_empty() && ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric()));
    kind_ok && hash_ok && tail_ok
}
