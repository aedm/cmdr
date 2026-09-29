//! Shared, path-shape-preserving redactor for log lines, panic messages, and error bundles.
//!
//! The hot path is [`redact_line`], called once per log line by the error reporter.
//! The crash reporter uses [`redact_panic_message`] (a thin alias kept for test parity).
//!
//! # Design
//!
//! One composed regex with named capture groups drives a single pass over each line.
//! The dispatch closure inspects which group matched and calls the appropriate rewriter.
//! This is ~2× faster than chaining `replace_all` calls per pattern and keeps all the
//! redaction rules in one place.
//!
//! # Path-shape preservation
//!
//! `/Users/john/Documents/budget.pdf` becomes `$HOME/Documents/<file>.pdf`. We keep the
//! extension and the immediate parent dir name, but only if that dir name is in the
//! allowlist (`Documents`, `Downloads`, `Desktop`, ...). Unknown parent dirs collapse to
//! `<dir>` so we never leak project-like names (`SecretProjectName`).
//!
//! # Report-scoped correlation
//!
//! [`redact_line`] emits bare `<dir>` / `<file>` tokens, useful but indistinguishable
//! when a log line mentions the same directory twenty times. The error reporter builds a
//! [`RedactionContext`] from the report ID. It emits 12-hex tokens derived from an ephemeral
//! process secret, report ID, identity domain, and normalized name. Preview and send rebuilds
//! of one report correlate; another report or process does not.
//!
//! # Coverage
//!
//! See `CLAUDE.md` in this directory for the pattern table and the runbook for adding
//! a new pattern.

use regex::{Captures, Regex};
use std::borrow::Cow;
use std::sync::OnceLock;

mod context;
mod fields;
mod names;
mod paths;
#[cfg(test)]
mod reference_tests;
mod references;
#[cfg(test)]
mod tests;

pub use context::RedactionContext;
use context::TokenDomain;
use fields::*;
use names::*;
use paths::*;
use references::*;

/// Parent directory names we consider safe to keep verbatim in redacted output.
/// Anything else collapses to `<dir>` to avoid leaking project-like names.
const SAFE_PARENT_DIR_NAMES: &[&str] = &[
    "Documents",
    "Downloads",
    "Desktop",
    "Library",
    "src",
    "Pictures",
    "Movies",
    "Music",
    "Public",
    "AppData",
    "Application Support",
];

/// Redact one line with the stable unsalted compatibility policy used by ordinary MCP.
///
/// Returns a [`Cow::Borrowed`] when no redaction was needed so we don't allocate
/// on lines like `"Reconciler: switched to live mode"` that have no PII at all.
///
/// Produces bare `<dir>` / `<file>` tokens. Report builders use
/// [`RedactionContext::redact_line`] for stricter coverage and report-local correlation.
pub fn redact_line(line: &str) -> Cow<'_, str> {
    redact_with(line, None)
}

impl RedactionContext {
    /// Redact one line with report-local, domain-separated correlation tokens.
    pub fn redact_line<'a>(&self, line: &'a str) -> Cow<'a, str> {
        redact_with(line, Some(self))
    }

    /// Redact a typed path or URL value. Unlike [`Self::redact_line`], this fails closed for
    /// relative and otherwise-unrecognized paths: every non-structural segment is tokenized.
    /// The caller supplies the value boundary, so this path never applies prose-boundary
    /// heuristics or returns a suffix unredacted.
    pub fn redact_path(&self, path: &str) -> String {
        redact_typed_path(path, Some(self))
    }

    /// Redact a typed file or folder name with this report's correlation key.
    pub fn redact_name(&self, name: &str, is_dir: bool) -> String {
        redact_leaf(name, !is_dir, Some(self))
    }

    /// Redact a volume's display name with the domain used for mount and share identities.
    pub fn redact_volume_name(&self, name: &str) -> String {
        identity_token("volume", TokenDomain::Volume, name, Some(self))
    }

    /// Redact a name-derived volume ID. The synthetic local-root ID carries no identity.
    pub fn redact_volume_id(&self, id: &str) -> String {
        if id == "root" {
            id.to_string()
        } else {
            identity_token("volume-id", TokenDomain::VolumeId, id, Some(self))
        }
    }
}

/// One left-to-right pass, resuming at whatever the rewriter actually consumed.
///
/// ❗ **Not `replace_all`, and the difference is load-bearing.** The path branches
/// deliberately over-match and hand a tail back (`split_trailing_noise`), but `replace_all`
/// resumes after the WHOLE match, so every handed-back byte was skipped by the scanner and
/// could never match another pattern. `/Volumes/d/f.txt and smb://host/share/x.txt` ate the
/// `smb:` into the volume match, gave it back as text, and left `//host/share/x.txt` with no
/// pattern willing to claim it: the share and the filename shipped verbatim. Resuming at
/// `match.start() + consumed` puts the tail back in front of the scanner, where it belongs.
fn redact_with<'a>(line: &'a str, context: Option<&RedactionContext>) -> Cow<'a, str> {
    let re = redactor_regex();
    let mut out: Option<String> = None;
    let mut pos = 0usize;

    while pos <= line.len() {
        // `captures_at` keeps the whole line as context, so `^` in `bare_lead` still means
        // "start of line" rather than "start of the remaining slice".
        let Some(caps) = re.captures_at(line, pos) else { break };
        let Some(whole) = caps.get(0) else { break };
        let (replacement, consumed) = dispatch(&caps, context);

        let buf = out.get_or_insert_with(|| String::with_capacity(line.len()));
        buf.push_str(&line[pos..whole.start()]);
        buf.push_str(&replacement);

        // A rewriter that consumed nothing would spin forever on the same offset; fall
        // back to the full match, then to one byte, so the scan always advances.
        let next = whole.start() + consumed;
        pos = if next > pos {
            next
        } else {
            whole.end().max(next_char_boundary(line, pos))
        };
    }

    match out {
        Some(mut buf) => {
            buf.push_str(&line[pos.min(line.len())..]);
            Cow::Owned(buf)
        }
        None => Cow::Borrowed(line),
    }
}

/// The next char boundary strictly after `pos`, so the no-progress fallback can't split a
/// multi-byte character.
fn next_char_boundary(line: &str, pos: usize) -> usize {
    let mut i = pos + 1;
    while i < line.len() && !line.is_char_boundary(i) {
        i += 1;
    }
    i.min(line.len())
}

/// Redact a multi-line text blob. Splits on `\n` and redacts each line independently
/// so regex anchors behave predictably.
pub fn redact_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        out.push_str(&redact_line(line));
    }
    out
}

/// Redact a panic message. Routes through [`redact_text`] so multi-line payloads (the
/// panic body + chained `caused by:` errors) get every line scrubbed independently.
pub fn redact_panic_message(message: &str) -> String {
    redact_text(message)
}

// --- Internals ---

fn redactor_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Path tail: consecutive path chars, optionally interrupted by single spaces (for
        // labels like "My Backup Drive" and filenames like "Invoice for Acme Corp.pdf").
        // Stops at whitespace-runs, quotes, brackets, and sentence-ending punctuation
        // that's clearly not path content.
        //
        // A continued word after a space may be ANY shape, lowercase included. Match
        // greedily here and give the boundary back in `split_trailing_noise`, which is the
        // only place that can tell a filename's own words from the prose after it.
        // ❗ The two have to stay in step. Anchoring continuation words to `[A-Z0-9]` here
        // instead looks tidier and silently ships PII: it stopped
        // `Screenshot 2026-09-04 at 01.13.03 PM-2.jpeg` dead at ` at`, and the rest of the
        // name rode out in an uploaded bundle verbatim. Every multi-word filename leaked
        // its tail that way.
        //
        // Tail chars: anything that isn't whitespace, quotes, backticks, angle brackets,
        // or the pipe character. Single spaces between tail chunks are allowed.
        //
        // The `(?x)` verbose flag lets us write this readably.
        Regex::new(
            r#"(?x)
            (?P<win_home>         [A-Za-z] : \\ Users \\ [^\\/\s"'<>|`]+
                                  (?: \\ [^\\\s"'<>|`]+ (?: \x20 [^\\\s"'<>|`]+ )* )*
            )
            | (?P<unix_home>      / (?: Users | home ) / [^/\s"'<>|`]+
                                  (?: / [^/\s"'<>|`]+ (?: \x20 [^/\s"'<>|`]+ )* )*
            )
            | (?P<unix_system>    / (?: tmp | var | private | opt ) /
                                  [^/\s"'<>|`]+
                                  (?: / [^/\s"'<>|`]+ (?: \x20 [^/\s"'<>|`]+ )* )*
            )
            | (?P<volumes>        / Volumes / [^/\s"'<>|`]+ (?: \x20 [^/\s"'<>|`]+ )*
                                  (?: / [^/\s"'<>|`]+ (?: \x20 [^/\s"'<>|`]+ )* )*
            )
            | (?P<media>          / media / [^/\s"'<>|`]+ (?: \x20 [^/\s"'<>|`]+ )*
                                  (?: / [^/\s"'<>|`]+ (?: \x20 [^/\s"'<>|`]+ )* )*
            )
            | (?P<remote_url>     (?i: sftp | ssh | webdav | http | https | smb ) ://
                                  [^\s"'<>|`]+ (?: \x20 [^\s"'<>|`]+ )*
            )
            | (?P<unc>            \\\\ [^\\\s"'<>|`]+ (?: \\ [^\\\s"'<>|`]+ (?: \x20 [^\\\s"'<>|`]+ )* )* )
            | (?P<url_userinfo>   (?P<scheme>[a-zA-Z][a-zA-Z0-9+.-]*) ://
                                  (?P<userinfo>[^\s@/:"'<>|`]+ (?: : [^\s@/"'<>|`]* )? )
                                  @
                                  (?P<host_rest>[^\s"'<>|`]*)
            )
            # Scheme-less userinfo URL: `//user:pass@host[:port][/...]`. The macOS `smbutil`
            # and Linux `smbclient` fallbacks build exactly this shape and a misbehaving
            # server can reflect it in stderr. The regex crate has no lookbehind, so we
            # capture the leading delimiter (start-of-text or a single whitespace char) and
            # re-emit it in the rewriter; this stops us from matching the `//user@host` tail
            # inside a scheme'd `http://user@host` (which `url_userinfo` already handles).
            | (?P<bare_lead>^|\s) (?P<bare_userinfo> //
                                  [^\s@/:"'<>|`]+ (?: : [^\s@/"'<>|`]* )?
                                  @
                                  (?P<bare_host_rest>[^\s"'<>|`]*)
            )
            # A path in a `key=value` log field, which may be RELATIVE: SMB logs name a file by
            # its share-relative path (`smb_path="docs/a b.pdf"`, and `smb2`'s own unquoted
            # `from=docs\a b.pdf to=…`), and no branch above can recognize a path with no mount
            # prefix in front of it. The key is what says "this is a path".
            #
            # Quoted values are `{:?}` output, escapes included. An unquoted value over-matches
            # to the end of the line and `end_of_bare_value` finds where it really stops. An
            # absolute value goes back to the branches above (see `redact_path_field`).
            | (?P<path_field>
                \b (?P<pf_key>
                    smb_path | path | input | from | to | file | directory | dir | parent
                  | src | dest | dst | destination | selectName | new_name | old_name
                )
                =
                (?P<pf_value>
                    " (?: [^"\\\n] | \\ . )* "
                  | [^\s"'<>|`\[\](){},;] [^"|`\n]*
                )
            )
            # User-controlled identities in producer-owned structured fields. These exact
            # keys are deliberately narrow: generic `name=` / `id=` occur throughout logs
            # with non-private meanings and must not become broad word matching.
            | (?P<identity_field>
                \b (?P<if_key>
                    host | server | share | volumeId | serverId | deviceId
                )
                =
                (?P<if_value>
                    Some\( " (?: [^"\\\n] | \\ . )* " \)
                  | " (?: [^"\\\n] | \\ . )* "
                )
            )
            # Current IDs from `cmdr-fs::volume::ids`: a known scheme and an exact
            # lowercase 16-hex digest. Bound the optional slug to the funnel's 24
            # characters rather than guessing at arbitrary hyphenated prose. MTP may
            # append its numeric storage ID.
            | (?P<derived_id>
                \b (?:
                    (?: smb | sftp | webdav | adb | vol | path ) -
                    (?: [\p{L}\p{N}] (?: [\p{L}\p{N}-]{0,22} [\p{L}\p{N}] )? - )?
                    [0-9a-f]{16}
                  | mtp -
                    (?: [\p{L}\p{N}] (?: [\p{L}\p{N}-]{0,22} [\p{L}\p{N}] )? - )?
                    [0-9a-f]{16}
                    (?: : [0-9]{1,10} )?
                )
                \b
            )
            # The manual SMB store predates the digest funnel. Its exact generated shape is
            # `manual-{address-with-dot/colon-as-dash}-{port}`.
            | (?P<manual_server_id>
                \b manual- [\p{L}\p{N}_-]+ - [0-9]{1,5} \b
            )
            | (?P<email>         [A-Za-z0-9][A-Za-z0-9._%+-]* @ [A-Za-z0-9][A-Za-z0-9.-]*\.[A-Za-z]{2,} )
            # Account name in a `key=value` / `key: value` log field. Our SMB paths log the
            # account someone signs in to a share with (`user=david`, `user=Some("david")`,
            # `username: "david"` in a debug struct); nothing else redacts it, and an account
            # name is as identifying as the email pattern above.
            #
            # The key must be exactly `user` / `username`, so `max_users=12` and `user_count=7`
            # don't match (`\b` can't split `parent_user` either: `_` is a word char). The `:`
            # form requires a real space after the colon, which is what keeps a module path
            # (`foo::user::bar`) out.
            | (?P<account>
                \b (?P<account_key> [Uu]ser (?: [Nn]ame )? )
                (?P<account_sep> = | : \x20+ )
                (?P<account_value>
                    Some\( " (?: [^"\\\n] | \\ . )* " \)
                  | " (?: [^"\\\n] | \\ . )* "
                  | [^\s,;"'()}]+
                )
            )
            | (?P<mdns>           [\p{L}\p{N}][\p{L}\p{N}-]{0,62} \. local\b )
            | (?P<ipv6>
                (?:
                  # Full 8-group form: a:b:c:d:e:f:g:h (h is required)
                  \b (?: [0-9A-Fa-f]{1,4} : ){7} [0-9A-Fa-f]{1,4} \b
                  # Compact forms: must have `::` with at least one hex group on at least one side.
                  # `a::b`, `a::`, `::b`, `a:b::c`, `::` alone (not matched, too ambiguous).
                  | \b [0-9A-Fa-f]{1,4} (?: : [0-9A-Fa-f]{1,4} ){0,6} :: (?: [0-9A-Fa-f]{1,4} (?: : [0-9A-Fa-f]{1,4} ){0,6} )? \b
                  | :: [0-9A-Fa-f]{1,4} (?: : [0-9A-Fa-f]{1,4} ){0,6} \b
                  | \b [0-9A-Fa-f]{1,4} (?: : [0-9A-Fa-f]{1,4} ){0,6} ::
                  # Loopback shorthand
                  | :: 1 \b
                )
            )
            | (?P<ipv4>           \b
                                  (?: (?: 25[0-5] | 2[0-4][0-9] | 1[0-9]{2} | [1-9]?[0-9] ) \. ){3}
                                      (?: 25[0-5] | 2[0-4][0-9] | 1[0-9]{2} | [1-9]?[0-9] )
                                \b
            )
            # MTP device name with a possessive owner prefix.
            #   "<Owner>'s Pixel 8 Pro"  → "<mtp-owner>'s Pixel 8 Pro"
            #   "<Owner>'s iPhone"        → "<mtp-owner>'s iPhone"
            # We ONLY match when the owner name is capitalized (so English contractions
            # like "It's a Pixel" don't match: `It` would be the owner candidate, but
            # the model word must follow the apostrophe-`s`-space pattern, and we
            # require the model to be one of a known set).
            # Bare model names without an owner ("Pixel 8 Pro") are intentionally NOT
            # matched; model strings alone aren't identifying and they're useful diag.
            | (?P<mtp_owner>
                \b [A-Z][a-zA-Z]+ ' s
                \x20+
                (?:
                    iPhone | iPad | iPod | Pixel | Galaxy | Samsung | OnePlus
                  | Note | Tablet | Phone | Camera
                )
                (?: \x20+ (?: Pro | Plus | Ultra | Max | Mini | SE | XL ) )?
                (?: \x20+ \d{1,3} )?
                (?: \x20+ (?: Pro | Plus | Ultra | Max | Mini | SE | XL ) )?
                \b
            )
            "#,
        )
        .expect("valid redactor regex")
    })
}

/// Rewrite one match into (replacement, bytes consumed).
///
/// A path branch consumes only the path, NOT the trailing noise it split off: the noise goes
/// back to the scanner in [`redact_with`], which is what lets a pattern that begins inside
/// the over-match still be recognized. Every other branch consumes its whole match.
fn dispatch(caps: &Captures<'_>, context: Option<&RedactionContext>) -> (String, usize) {
    if let Some(m) = caps.name("win_home") {
        let (path, _) = split_trailing_noise(m.as_str());
        return (redact_windows_home(path, context), path.len());
    }
    if let Some(m) = caps.name("unix_home") {
        let (path, _) = split_trailing_noise(m.as_str());
        return (redact_unix_home(path, context), path.len());
    }
    if let Some(m) = caps.name("unix_system") {
        let (path, _) = split_trailing_noise(m.as_str());
        return (redact_unix_system(path, context), path.len());
    }
    if let Some(m) = caps.name("volumes") {
        let (path, _) = split_trailing_noise(m.as_str());
        return (redact_volumes(path, context), path.len());
    }
    if let Some(m) = caps.name("media") {
        let (path, _) = split_trailing_noise(m.as_str());
        return (redact_media(path, context), path.len());
    }
    if let Some(m) = caps.name("remote_url") {
        if context.is_none() && !m.as_str().starts_with("smb://") {
            if remote_authority_has_userinfo(m.as_str()) {
                return (redact_legacy_url_userinfo(m.as_str()), whole_len(caps));
            }
            return rescan_inside(m.as_str());
        }
        let (path, _) = split_trailing_noise(m.as_str());
        let reference = trim_reference_end(path);
        if context.is_none() {
            return (redact_legacy_smb_url(reference), reference.len());
        }
        return (redact_remote_url(reference, context), reference.len());
    }
    if let Some(m) = caps.name("unc") {
        let (path, _) = split_trailing_noise(m.as_str());
        if context.is_none() {
            let host = path
                .strip_prefix(r"\\")
                .unwrap_or(path)
                .split('\\')
                .next()
                .unwrap_or_default();
            if !host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
            {
                return rescan_inside(m.as_str());
            }
            return (redact_legacy_unc(path), path.len());
        }
        return (redact_remote_unc(path, context), path.len());
    }
    if caps.name("url_userinfo").is_some() {
        if context.is_none() {
            let scheme = caps.name("scheme").map_or("", |m| m.as_str());
            let host_rest = caps.name("host_rest").map_or("", |m| m.as_str());
            return (format!("{scheme}://<userinfo>@{host_rest}"), whole_len(caps));
        }
        let reference = caps.get(0).map_or("", |m| m.as_str());
        return (redact_remote_url(reference, context), whole_len(caps));
    }
    if let Some(m) = caps.name("bare_userinfo") {
        let lead = caps.name("bare_lead").map(|m| m.as_str()).unwrap_or("");
        if context.is_none() {
            let host_rest = caps.name("bare_host_rest").map_or("", |m| m.as_str());
            return (format!("{lead}//<userinfo>@{host_rest}"), whole_len(caps));
        }
        return (
            format!("{lead}{}", redact_scheme_less(m.as_str(), context)),
            whole_len(caps),
        );
    }
    if caps.name("path_field").is_some() {
        return redact_path_field(caps, context);
    }
    if let Some(m) = caps.name("identity_field") {
        if context.is_none() {
            return rescan_inside(m.as_str());
        }
        return redact_identity_field(caps, context);
    }
    if let Some(m) = caps.name("derived_id") {
        if context.is_none() {
            return rescan_inside(m.as_str());
        }
        return (redact_derived_id(m.as_str(), context), whole_len(caps));
    }
    if let Some(m) = caps.name("manual_server_id") {
        if context.is_none() {
            return rescan_inside(m.as_str());
        }
        return (redact_manual_server_id(m.as_str(), context), whole_len(caps));
    }
    if caps.name("email").is_some() {
        return ("<email>".to_string(), whole_len(caps));
    }
    if let Some(m) = caps.name("account") {
        return (
            redact_account(
                caps.name("account_key").map(|k| k.as_str()).unwrap_or("user"),
                caps.name("account_sep").map(|s| s.as_str()).unwrap_or("="),
                caps.name("account_value").map(|v| v.as_str()).unwrap_or(""),
                m.as_str(),
                context,
            ),
            whole_len(caps),
        );
    }
    if let Some(m) = caps.name("mdns") {
        if context.is_none() && !m.as_str().is_ascii() {
            return rescan_inside(m.as_str());
        }
        return (redact_mdns_host(m.as_str(), context), whole_len(caps));
    }
    if let Some(m) = caps.name("ipv6") {
        return (
            identity_token("ipv6", TokenDomain::Host, m.as_str(), context),
            whole_len(caps),
        );
    }
    if let Some(m) = caps.name("ipv4") {
        return (
            identity_token("ipv4", TokenDomain::Host, m.as_str(), context),
            whole_len(caps),
        );
    }
    if let Some(m) = caps.name("mtp_owner") {
        return (redact_mtp_owner(m.as_str(), context), whole_len(caps));
    }
    // Shouldn't happen: regex matched but no named group. Return verbatim to be safe.
    (
        caps.get(0).map(|m| m.as_str().to_string()).unwrap_or_default(),
        whole_len(caps),
    )
}

/// Byte length of the whole match, for the branches that consume all of it.
fn whole_len(caps: &Captures<'_>) -> usize {
    caps.get(0).map_or(0, |m| m.len())
}

/// Leave a report-only outer match in front of the compatibility scanner one character at
/// a time. This preserves legacy nested matches instead of either applying the new policy or
/// returning the whole reference raw (for example, `.local`, IP, and email inside WebDAV).
fn rescan_inside(value: &str) -> (String, usize) {
    value
        .chars()
        .next()
        .map_or_else(|| (String::new(), 0), |first| (first.to_string(), first.len_utf8()))
}

fn redact_legacy_url_userinfo(reference: &str) -> String {
    let Some((scheme, remainder)) = reference.split_once("://") else {
        return reference.to_string();
    };
    let Some((_, host_rest)) = remainder.split_once('@') else {
        return reference.to_string();
    };
    format!("{scheme}://<userinfo>@{host_rest}")
}

fn remote_authority_has_userinfo(reference: &str) -> bool {
    reference
        .split_once("://")
        .map(|(_, remainder)| {
            remainder
                .split(['/', '?', '#'])
                .next()
                .unwrap_or_default()
                .contains('@')
        })
        .unwrap_or(false)
}

fn identity_token(kind: &str, domain: TokenDomain, value: &str, context: Option<&RedactionContext>) -> String {
    context.map_or_else(
        || format!("<{kind}>"),
        |context| format!("<{kind}:{}>", context.token(domain, value)),
    )
}

/// Replace an account name with `<user>`, keeping the field's shape so the line still reads.
///
/// `Some(...)` and the quotes stay because they carry the answer to the question a triager
/// asks of these lines ("did we have a username at all, and did it come from the mount info
/// or the Keychain?"). `None` is not a name and passes through verbatim, which is the whole
/// reason this can't be a blanket `user=\S+` → `<user>` rewrite.
fn redact_account(key: &str, separator: &str, value: &str, whole: &str, context: Option<&RedactionContext>) -> String {
    if value == "None" {
        return whole.to_string();
    }
    if value.starts_with("Some(\"") && value.ends_with("\")") {
        let account = unescape_debug(&value[6..value.len() - 2]);
        return format!(
            "{key}{separator}Some(\"{}\")",
            identity_token("user", TokenDomain::Userinfo, &account, context)
        );
    }
    if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        let account = unescape_debug(&value[1..value.len() - 1]);
        return format!(
            "{key}{separator}\"{}\"",
            identity_token("user", TokenDomain::Userinfo, &account, context)
        );
    }
    format!(
        "{key}{separator}{}",
        identity_token("user", TokenDomain::Userinfo, value, context)
    )
}

/// Replace the possessive owner prefix with `<mtp-owner>`, keep the model words intact.
/// Input is guaranteed to start with `<Owner>'s ` (capital letter, then letters, then
/// `'s`, then one or more spaces) by the regex.
fn redact_mtp_owner(s: &str, context: Option<&RedactionContext>) -> String {
    // Find the `'s` boundary; everything from there onward is the model phrase.
    // Splitting on `'s` is safe because the regex anchors the apostrophe-s.
    match s.find("'s") {
        Some(i) => {
            // s[i..] starts with "'s", which we want to keep so the redacted output
            // reads naturally ("<mtp-owner>'s Pixel 8 Pro").
            format!(
                "{}{}",
                identity_token("mtp-owner", TokenDomain::Device, &s[..i], context),
                &s[i..]
            )
        }
        None => s.to_string(),
    }
}

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
fn split_trailing_noise(s: &str) -> (&str, &str) {
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
    if seam != Some(end) {
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
