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
//! # Salted mode (per-bundle correlation)
//!
//! [`redact_line`] emits bare `<dir>` / `<file>` tokens, useful but indistinguishable
//! when a log line mentions the same directory twenty times. The error reporter calls
//! [`redact_line_salted`] instead, threading a 16-byte random salt minted at bundle
//! build time. Salted mode emits `<dir:HHHHHH>` / `<file:HHHHHH>` where the 6 hex chars
//! are the first 3 bytes of `blake3(salt || segment)`. Same path → same hash within a
//! bundle, so a triager (or agent) can spot "same dir, mentioned 12 times." Different
//! salt across bundles → no cross-bundle correlation, no rainbow tables.
//!
//! # Coverage
//!
//! See `CLAUDE.md` in this directory for the pattern table and the runbook for adding
//! a new pattern.

use regex::{Captures, Regex};
use std::borrow::Cow;
use std::sync::OnceLock;

#[cfg(test)]
mod tests;

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

/// Redact one log line. Hot path: called per line by the error reporter.
///
/// Returns a [`Cow::Borrowed`] when no redaction was needed so we don't allocate
/// on lines like `"Reconciler: switched to live mode"` that have no PII at all.
///
/// Bare `<dir>` / `<file>` tokens. For salted mode (correlatable hashes within a
/// bundle), use [`redact_line_salted`].
pub fn redact_line(line: &str) -> Cow<'_, str> {
    redact_with(line, None)
}

/// Salted variant of [`redact_line`]. Path segments that would collapse to `<dir>`
/// or `<file>` instead emit `<dir:HHHHHH>` / `<file:HHHHHH>` where the 6 hex chars are
/// `blake3(salt || segment)[..3]`. Same input → same output within a single salt;
/// no cross-bundle correlation between different salts.
///
/// The salt is expected to be ≥ 16 bytes of cryptographic random per bundle. Anything
/// shorter is accepted (the hash still correlates) but cross-bundle resistance suffers
/// proportionally.
pub fn redact_line_salted<'a>(line: &'a str, salt: &[u8]) -> Cow<'a, str> {
    redact_with(line, Some(salt))
}

/// Redact a bare file or folder NAME, one with no path around it for a pattern to find:
/// `budget.pdf` → `<file>.pdf`, `Wedding` → `<dir>`, `Downloads` → `Downloads` (allowlisted).
/// Same leaf rules as a path's last segment, unsalted.
///
/// For structured output that names things on its own (the error reporter's state snapshot),
/// where a line-level pass can't tell a name from any other word.
pub fn redact_name(name: &str, is_dir: bool) -> String {
    redact_leaf(name, !is_dir, None)
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
fn redact_with<'a>(line: &'a str, salt: Option<&[u8]>) -> Cow<'a, str> {
    let re = redactor_regex();
    let mut out: Option<String> = None;
    let mut pos = 0usize;

    while pos <= line.len() {
        // `captures_at` keeps the whole line as context, so `^` in `bare_lead` still means
        // "start of line" rather than "start of the remaining slice".
        let Some(caps) = re.captures_at(line, pos) else { break };
        let Some(whole) = caps.get(0) else { break };
        let (replacement, consumed) = dispatch(&caps, salt);

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
            | (?P<smb_uri>        smb:// [^\s"'<>|`]+ )
            | (?P<unc>            \\\\ [A-Za-z0-9_.-]+ (?: \\ [^\\\s"'<>|`]+ (?: \x20 [^\\\s"'<>|`]+ )* )* )
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
                    Some\( " [^"]* " \)
                  | " [^"]* "
                  | [^\s,;"'()}]+
                )
            )
            | (?P<mdns>           [A-Za-z0-9][A-Za-z0-9-]{0,62} \. local\b )
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
fn dispatch(caps: &Captures<'_>, salt: Option<&[u8]>) -> (String, usize) {
    if let Some(m) = caps.name("win_home") {
        let (path, _) = split_trailing_noise(m.as_str());
        return (redact_windows_home(path, salt), path.len());
    }
    if let Some(m) = caps.name("unix_home") {
        let (path, _) = split_trailing_noise(m.as_str());
        return (redact_unix_home(path, salt), path.len());
    }
    if let Some(m) = caps.name("unix_system") {
        let (path, _) = split_trailing_noise(m.as_str());
        return (redact_unix_system(path, salt), path.len());
    }
    if let Some(m) = caps.name("volumes") {
        let (path, _) = split_trailing_noise(m.as_str());
        return (redact_volumes(path, salt), path.len());
    }
    if let Some(m) = caps.name("media") {
        let (path, _) = split_trailing_noise(m.as_str());
        return (redact_media(path, salt), path.len());
    }
    if let Some(m) = caps.name("smb_uri") {
        let (path, _) = split_trailing_noise(m.as_str());
        return (redact_smb_uri(path, salt), path.len());
    }
    if let Some(m) = caps.name("unc") {
        let (path, _) = split_trailing_noise(m.as_str());
        return (redact_unc(path, salt), path.len());
    }
    if caps.name("url_userinfo").is_some() {
        // Preserve scheme and everything after the `@`, redact the userinfo.
        let scheme = caps.name("scheme").map(|m| m.as_str()).unwrap_or("");
        let host_rest = caps.name("host_rest").map(|m| m.as_str()).unwrap_or("");
        return (format!("{scheme}://<userinfo>@{host_rest}"), whole_len(caps));
    }
    if caps.name("bare_userinfo").is_some() {
        // Scheme-less `//user:pass@host`: drop the userinfo, keep the leading delimiter
        // and everything after the `@`.
        let lead = caps.name("bare_lead").map(|m| m.as_str()).unwrap_or("");
        let host_rest = caps.name("bare_host_rest").map(|m| m.as_str()).unwrap_or("");
        return (format!("{lead}//<userinfo>@{host_rest}"), whole_len(caps));
    }
    if caps.name("path_field").is_some() {
        return redact_path_field(caps, salt);
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
            ),
            whole_len(caps),
        );
    }
    if caps.name("mdns").is_some() {
        return ("<host>.local".to_string(), whole_len(caps));
    }
    if caps.name("ipv6").is_some() {
        return ("<ipv6>".to_string(), whole_len(caps));
    }
    if caps.name("ipv4").is_some() {
        return ("<ipv4>".to_string(), whole_len(caps));
    }
    if let Some(m) = caps.name("mtp_owner") {
        return (redact_mtp_owner(m.as_str()), whole_len(caps));
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

/// Replace an account name with `<user>`, keeping the field's shape so the line still reads.
///
/// `Some(...)` and the quotes stay because they carry the answer to the question a triager
/// asks of these lines ("did we have a username at all, and did it come from the mount info
/// or the Keychain?"). `None` is not a name and passes through verbatim, which is the whole
/// reason this can't be a blanket `user=\S+` → `<user>` rewrite.
fn redact_account(key: &str, separator: &str, value: &str, whole: &str) -> String {
    if value == "None" {
        return whole.to_string();
    }
    if value.starts_with("Some(\"") && value.ends_with("\")") {
        return format!("{key}{separator}Some(\"<user>\")");
    }
    if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        return format!("{key}{separator}\"<user>\"");
    }
    format!("{key}{separator}<user>")
}

/// Replace the possessive owner prefix with `<mtp-owner>`, keep the model words intact.
/// Input is guaranteed to start with `<Owner>'s ` (capital letter, then letters, then
/// `'s`, then one or more spaces) by the regex.
fn redact_mtp_owner(s: &str) -> String {
    // Find the `'s` boundary; everything from there onward is the model phrase.
    // Splitting on `'s` is safe because the regex anchors the apostrophe-s.
    match s.find("'s") {
        Some(i) => {
            // s[i..] starts with "'s", which we want to keep so the redacted output
            // reads naturally ("<mtp-owner>'s Pixel 8 Pro").
            format!("<mtp-owner>{}", &s[i..])
        }
        None => s.to_string(),
    }
}

/// Path-branch groups: a value one of these claims from its first byte is an absolute path
/// they already know how to redact.
const PATH_BRANCHES: &[&str] = &[
    "win_home",
    "unix_home",
    "unix_system",
    "volumes",
    "media",
    "smb_uri",
    "unc",
    "url_userinfo",
];

/// Top-level directory names that say where a path lives without saying anything about
/// who owns it. Kept as the first segment of an absolute field value (`path=/private`).
const SYSTEM_ROOTS: &[&str] = &[
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
/// just `key=` (plus the opening quote), and [`redact_with`] resumes at the value, where that
/// branch claims it with its own prefix rules (`$HOME`, `/Volumes/<volume>`, …). Everything
/// else is walked here segment by segment, which is what reaches a share-relative path.
fn redact_path_field(caps: &Captures<'_>, salt: Option<&[u8]>) -> (String, usize) {
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
    let redacted = redact_relative_path(&unescaped, salt);
    let consumed = head.len() + value.len() + quote.len();
    (format!("{head}{redacted}{quote}"), consumed)
}

/// Where an unquoted field value really ends. The regex takes the rest of the line; the value
/// stops at the first of: the `{path}: {message}` seam, a `, ` (the next field), or a
/// ` key=` (the next field in `smb2`'s space-separated `from=… to=…`). A `)` left over from
/// `fn(share=…, path=…)` goes too, unless the name opened it (`photo (1).jpg`).
///
/// ⚠️ A comma-space inside an unquoted name ends it early and the rest is handed back
/// unredacted. Only `{}`-printed values can hit that, which is why path fields in our own
/// code are printed with `{:?}`.
fn end_of_bare_value(raw: &str) -> &str {
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
fn starts_with_field_key(s: &str) -> bool {
    let ident_len = s
        .char_indices()
        .take_while(|&(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
        .count();
    ident_len > 0 && s[ident_len..].starts_with('=')
}

/// Whether a path branch matches `value` from its very first byte.
fn claimed_by_path_branch(value: &str) -> bool {
    redactor_regex().captures(value).is_some_and(|caps| {
        caps.get(0).is_some_and(|m| m.start() == 0) && PATH_BRANCHES.iter().any(|g| caps.name(g).is_some())
    })
}

/// Redact a path no branch has a prefix rule for: share-relative (`docs/a b.pdf`,
/// `docs\a b.pdf`), volume-relative (`/docs/a b.pdf`), or a bare name. Same shape rules as
/// every other path: the leaf keeps its extension, an allowlisted parent keeps its name, the
/// rest collapse. Segments that are already tokens pass through, which keeps it idempotent.
fn redact_relative_path(value: &str, salt: Option<&[u8]>) -> String {
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
            out.push_str(&redact_leaf(seg, has_extension_like_suffix(seg), salt));
        } else if i + 1 == leaf_idx && is_safe_parent_dir(seg) {
            out.push_str(seg);
        } else {
            out.push_str(&dir_token(seg, salt));
        }
    }
    out
}

/// Whether a path segment is already redacted (`<dir>`, `<file:ab12cd>.pdf`, `$HOME`) or
/// carries nothing to redact (`.`, `..`).
fn is_redacted_segment(seg: &str) -> bool {
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
    let hash_ok = hash.is_empty() || (hash.len() == 6 && hash.chars().all(|c| c.is_ascii_hexdigit()));
    let tail_ok = tail.is_empty()
        || tail
            .strip_prefix('.')
            .is_some_and(|ext| !ext.is_empty() && ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric()));
    kind_ok && hash_ok && tail_ok
}

/// The staging suffixes Cmdr puts on names it's still writing. Their UUID tail carries no
/// PII, and keeping it lets a triager see which temp became which file.
const CMDR_TEMP_SUFFIXES: &[&str] = &[".cmdr-tmp-", ".cmdr-temp-", ".cmdr-staging-"];

/// Split `photo.jpg.cmdr-tmp-3f2a…` into (`photo.jpg`, `.cmdr-tmp-3f2a…`). The tail must be
/// hex and dashes, so a user's own `notes.cmdr-tmp-plan.txt` doesn't qualify.
fn split_cmdr_suffix(seg: &str) -> (&str, &str) {
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
fn unescape_debug(s: &str) -> Cow<'_, str> {
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
fn ends_with_unicode_escape(s: &str) -> bool {
    let Some(body) = s.strip_suffix('}') else { return false };
    body.rfind("\\u{").is_some_and(|at| {
        let hex = &body[at + 3..];
        (1..=6).contains(&hex.len()) && hex.chars().all(|c| c.is_ascii_hexdigit())
    })
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

// --- Path rewriters ---

fn redact_unix_home(path: &str, salt: Option<&[u8]>) -> String {
    // path like `/Users/<user>/...` or `/home/<user>/...`
    // Strip the `/Users/<user>` prefix and replace with `$HOME`.
    let rest = match path.split('/').nth(3) {
        Some(_) => {
            // find the 3rd `/` and take what follows
            let mut slashes = 0;
            let mut cut = None;
            for (i, ch) in path.char_indices() {
                if ch == '/' {
                    slashes += 1;
                    if slashes == 3 {
                        cut = Some(i);
                        break;
                    }
                }
            }
            cut.map(|i| &path[i..]).unwrap_or("")
        }
        None => "",
    };
    format!("$HOME{}", redact_path_tail(rest, salt))
}

fn redact_windows_home(path: &str, salt: Option<&[u8]>) -> String {
    // `C:\Users\<user>\...` → `$HOME\...` (using backslashes to preserve shape)
    // Skip the first 3 `\` separators: `C:` + `\Users` + `\<user>`.
    let mut backslashes = 0;
    let mut cut = None;
    for (i, ch) in path.char_indices() {
        if ch == '\\' {
            backslashes += 1;
            if backslashes == 3 {
                cut = Some(i);
                break;
            }
        }
    }
    let rest = cut.map(|i| &path[i..]).unwrap_or("");
    // Normalize to forward slashes for the tail walker, then convert back.
    let normalized: String = rest.chars().map(|c| if c == '\\' { '/' } else { c }).collect();
    let redacted_tail = redact_path_tail(&normalized, salt);
    format!("$HOME{}", redacted_tail.replace('/', "\\"))
}

fn redact_unix_system(path: &str, salt: Option<&[u8]>) -> String {
    // `/tmp/<rest>`, `/var/<rest>`, `/private/<rest>`, `/opt/<rest>`: keep prefix verbatim,
    // redact everything below it with shape preservation.
    // Find the second `/` (end of the prefix dir), keep `/tmp/` etc., walk the tail.
    let mut slashes = 0;
    let mut tail_start = path.len();
    for (i, ch) in path.char_indices() {
        if ch == '/' {
            slashes += 1;
            if slashes == 2 {
                tail_start = i + 1;
                break;
            }
        }
    }
    let prefix = &path[..tail_start]; // includes trailing `/`
    let tail = &path[tail_start..];
    if tail.is_empty() {
        return prefix.to_string();
    }
    // tail is one or more segments separated by `/`. Reuse redact_path_tail by prepending `/`.
    let redacted = redact_path_tail(&format!("/{tail}"), salt);
    // strip the leading `/` we added back since `prefix` already ends in `/`
    format!("{}{}", prefix, redacted.strip_prefix('/').unwrap_or(&redacted))
}

fn redact_volumes(path: &str, salt: Option<&[u8]>) -> String {
    // `/Volumes/<label>/<rest>` → `/Volumes/<volume>/<redacted rest>`
    redact_labeled_mount(path, "/Volumes/", "/Volumes/<volume>", salt)
}

fn redact_media(path: &str, salt: Option<&[u8]>) -> String {
    // `/media/<label>/<rest>` → `/media/<volume>/<redacted rest>`
    redact_labeled_mount(path, "/media/", "/media/<volume>", salt)
}

fn redact_labeled_mount(path: &str, prefix: &str, prefix_out: &str, salt: Option<&[u8]>) -> String {
    let after = path.strip_prefix(prefix).unwrap_or(path);
    // Label may contain spaces. Find the first `/` to end the label.
    match after.find('/') {
        Some(i) => {
            let rest = &after[i..]; // starts with `/`
            format!("{prefix_out}{}", redact_path_tail(rest, salt))
        }
        None => prefix_out.to_string(),
    }
}

fn redact_smb_uri(uri: &str, salt: Option<&[u8]>) -> String {
    // `smb://host/share/path/file.ext` → `smb://<host>/<share>/<redacted path>`
    let after = uri.strip_prefix("smb://").unwrap_or(uri);
    // split host
    let (_host, rest) = match after.split_once('/') {
        Some(parts) => parts,
        None => return "smb://<host>".to_string(),
    };
    // split share
    let (_share, tail) = match rest.split_once('/') {
        Some(parts) => (parts.0, format!("/{}", parts.1)),
        None => return "smb://<host>/<share>".to_string(),
    };
    format!("smb://<host>/<share>{}", redact_path_tail(&tail, salt))
}

fn redact_unc(unc: &str, salt: Option<&[u8]>) -> String {
    // `\\host\share\path\file.ext` → `\\<host>\<share>\<redacted path>`
    let after = unc.strip_prefix("\\\\").unwrap_or(unc);
    // normalize to forward slashes for reuse, then convert back
    let normalized: String = after.chars().map(|c| if c == '\\' { '/' } else { c }).collect();
    let parts: Vec<&str> = normalized.splitn(3, '/').collect();
    match parts.as_slice() {
        [_host] => r"\\<host>".to_string(),
        [_host, _share] => r"\\<host>\<share>".to_string(),
        [_host, _share, tail] => {
            let redacted = redact_path_tail(&format!("/{tail}"), salt);
            format!(r"\\<host>\<share>{}", redacted.replace('/', "\\"))
        }
        _ => r"\\<host>".to_string(),
    }
}

/// Redact the tail of a path (everything after the user/label prefix).
/// Input starts with `/` (or is empty). Output starts with `/` (or is empty).
///
/// Shape preservation: keep the filename's extension and the last directory name if it's
/// in [`SAFE_PARENT_DIR_NAMES`]. Otherwise collapse to `<dir>` / `<file>` (or salted
/// equivalents when `salt` is `Some`).
fn redact_path_tail(tail: &str, salt: Option<&[u8]>) -> String {
    if tail.is_empty() {
        return String::new();
    }
    // tail starts with `/`, strip it for splitting.
    let body = tail.strip_prefix('/').unwrap_or(tail);
    if body.is_empty() {
        return "/".to_string();
    }
    let segments: Vec<&str> = body.split('/').collect();
    if segments.len() == 1 {
        // Single segment under the prefix: could be a dir or a file. We guess based on
        // presence of an extension: segments with a `.X` suffix are files, otherwise dirs.
        let seg = segments[0];
        let is_file = has_extension_like_suffix(seg);
        return format!("/{}", redact_leaf(seg, is_file, salt));
    }
    // Walk segments: all but the last are dirs; the last is guessed via the
    // extension heuristic: leaves with `.ext` are files, leaves without are dirs.
    // Don't default leaves to `<file>` unconditionally: directory listings dominate
    // log lines, so they'd read as files in error reports.
    let mut out = String::new();
    let last_idx = segments.len() - 1;
    for (i, seg) in segments.iter().enumerate() {
        out.push('/');
        if i == last_idx {
            let is_file = has_extension_like_suffix(seg);
            out.push_str(&redact_leaf(seg, is_file, salt));
        } else if i == last_idx - 1 {
            // Immediate parent dir of the leaf; allowlist check.
            if is_safe_parent_dir(seg) {
                out.push_str(seg);
            } else {
                out.push_str(&dir_token(seg, salt));
            }
        } else {
            // Ancestor dirs: always collapse.
            out.push_str(&dir_token(seg, salt));
        }
    }
    out
}

fn redact_leaf(seg: &str, is_file: bool, salt: Option<&[u8]>) -> String {
    if seg.is_empty() {
        return String::new();
    }
    // `photo.jpg.cmdr-tmp-3f2a…` is `photo.jpg` on its way in: redact the name the temp will
    // become (so both hash alike) and keep Cmdr's own suffix.
    let (name, temp_suffix) = split_cmdr_suffix(seg);
    if !temp_suffix.is_empty() {
        return format!(
            "{}{temp_suffix}",
            redact_leaf(name, has_extension_like_suffix(name), salt)
        );
    }
    if !is_file {
        return if is_safe_parent_dir(seg) {
            seg.to_string()
        } else {
            dir_token(seg, salt)
        };
    }
    // File: try to keep the extension.
    if let Some(dot) = seg.rfind('.') {
        let ext = &seg[dot + 1..];
        // Only preserve "sane" extensions: <= 8 ASCII chars, alnum. Otherwise it's probably
        // a filename with a dot in the stem (e.g., `my.secret.project`), not an extension.
        if !ext.is_empty() && ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric()) && dot > 0 {
            return format!("{}.{ext}", file_token(seg, salt));
        }
    }
    file_token(seg, salt)
}

fn dir_token(seg: &str, salt: Option<&[u8]>) -> String {
    match salt {
        Some(s) => format!("<dir:{}>", short_hash(s, seg)),
        None => "<dir>".to_string(),
    }
}

fn file_token(seg: &str, salt: Option<&[u8]>) -> String {
    match salt {
        Some(s) => format!("<file:{}>", short_hash(s, seg)),
        None => "<file>".to_string(),
    }
}

/// Short, salted, lowercase-hex hash for path segments. 6 hex chars = 3 bytes ≈ 16 M
/// distinct values; collisions are possible but harmless: only correlation within a
/// single bundle's window matters here, and a bundle holds at most low-thousands of
/// distinct path segments. Cross-bundle correlation is prevented by varying the salt.
///
/// Uses SHA-256 (already in our dep tree for license device hashing) rather than
/// pulling in a second hash crate just for this. The hash is overkill for what we
/// need: we only consume the first 3 bytes, but the cost is one allocation per
/// distinct path segment per bundle build, negligible.
///
/// The segment is hashed as the NAME it spells, not the bytes that printed it: `{:?}` escapes
/// are undone and the result NFC-normalized, so `me\u{301}retek` (Debug), `méretek` (NFD,
/// Display), and `méretek` (NFC, what a NAS lists) are one token. Without that, one file
/// showed up as three unrelated tokens in a single bundle.
fn short_hash(salt: &[u8], segment: &str) -> String {
    use sha2::{Digest, Sha256};
    use unicode_normalization::UnicodeNormalization;
    let name: String = unescape_debug(segment).nfc().collect();
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(name.as_bytes());
    let bytes = hasher.finalize();
    format!("{:02x}{:02x}{:02x}", bytes[0], bytes[1], bytes[2])
}

fn is_safe_parent_dir(name: &str) -> bool {
    SAFE_PARENT_DIR_NAMES.contains(&name)
}

/// True if `seg` looks like a filename with an extension (e.g., `foo.pdf`).
/// False for `Documents`, `.ssh`, `config`, `v0.13.0` (leading digits in ext is fine but
/// we require the dot to be in a reasonable position).
/// Whether this space-separated token looks like the END of a filename: an extension whose
/// first character is a LETTER.
///
/// Stricter than [`has_extension_like_suffix`] on purpose, and the extra letter is doing
/// real work: `01.13.03` in a screenshot timestamp has an "extension" of `03`, so the looser
/// test would cut the name in half and ship ` PM-2.jpeg` verbatim. The cost is that a
/// digit-led extension (`.7z`, `.3gp`) doesn't end the scan, which only means the path may
/// keep a word or two of prose — an over-redaction, never a leak.
fn ends_filename(token: &str) -> bool {
    let Some(dot) = token.rfind('.') else { return false };
    let ext = &token[dot + 1..];
    dot > 0
        && !ext.is_empty()
        && ext.len() <= 8
        && ext.chars().all(|c| c.is_ascii_alphanumeric())
        && ext.starts_with(|c: char| c.is_ascii_alphabetic())
}

/// Whether this token ends a sentence, and with it the path. `/Volumes/naspi-1) claim …`
/// is the shape: the label ends at the `)`, and everything after is prose.
fn ends_sentence(token: &str) -> bool {
    matches!(
        token.as_bytes().last(),
        Some(b')' | b';' | b',' | b'.' | b'!' | b'?' | b']' | b'}')
    ) && !ends_with_unicode_escape(token)
}

fn has_extension_like_suffix(seg: &str) -> bool {
    if let Some(dot) = seg.rfind('.') {
        let ext = &seg[dot + 1..];
        // dot not at position 0 (no `.ssh`) and ext is alnum, <= 8 chars.
        dot > 0 && !ext.is_empty() && ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric())
    } else {
        false
    }
}
