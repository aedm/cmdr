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
            | (?P<email>          [A-Za-z0-9][A-Za-z0-9._%+-]* @ [A-Za-z0-9][A-Za-z0-9.-]*\.[A-Za-z]{2,} )
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
///   last `/`, which is what keeps `/Volumes/naspi and then it failed` down to `naspi`.
fn split_trailing_noise(s: &str) -> (&str, &str) {
    let bytes = s.as_bytes();
    let mut end = bytes.len();

    // The `{path}: {message}` seam. Everything from it belongs to the message.
    if let Some(seam) = s.find(": ") {
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
    while end > 0 {
        let b = bytes[end - 1];
        if matches!(b, b',' | b';' | b'!' | b'?' | b')' | b']' | b'}') {
            end -= 1;
        } else {
            break;
        }
    }

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
    // The floor is the first word after the last `/`: that word is a real path segment
    // however lowercase it looks, so `/Volumes/naspi and then it failed` keeps `naspi`.
    // An extension ends the run on the spot, because a word carrying one is part of the
    // filename, not prose — that is what holds `my secret notes.txt` together.
    {
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
            if !starts_lower || has_extension_like_suffix(word) {
                break;
            }
            end = i - 1;
            while end > floor && bytes[end - 1] == b' ' {
                end -= 1;
            }
        }
    }

    // Finally, strip a trailing `.` or `,` that was exposed by the above steps.
    while end > 0 {
        let b = bytes[end - 1];
        if matches!(b, b',' | b';' | b'!' | b'?' | b')' | b']' | b'}') {
            end -= 1;
        } else {
            break;
        }
    }

    // SAFETY: we only advance `end` on ASCII byte boundaries.
    (&s[..end], &s[end..])
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
fn short_hash(salt: &[u8], segment: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(segment.as_bytes());
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
    )
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
