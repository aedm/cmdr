//! The path-shape rewriters: each path branch keeps its mount/home prefix as a fixed token, and the tail goes
//! through the same leaf and allowlist rules (`redact_path_tail`).

use super::SAFE_PARENT_DIR_NAMES;
use super::names::{ends_with_unicode_escape, split_cmdr_suffix, unescape_debug};

pub(super) fn redact_unix_home(path: &str, salt: Option<&[u8]>) -> String {
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

pub(super) fn redact_windows_home(path: &str, salt: Option<&[u8]>) -> String {
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

pub(super) fn redact_unix_system(path: &str, salt: Option<&[u8]>) -> String {
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

pub(super) fn redact_volumes(path: &str, salt: Option<&[u8]>) -> String {
    // `/Volumes/<label>/<rest>` → `/Volumes/<volume>/<redacted rest>`
    redact_labeled_mount(path, "/Volumes/", "/Volumes/<volume>", salt)
}

pub(super) fn redact_media(path: &str, salt: Option<&[u8]>) -> String {
    // `/media/<label>/<rest>` → `/media/<volume>/<redacted rest>`
    redact_labeled_mount(path, "/media/", "/media/<volume>", salt)
}

pub(super) fn redact_labeled_mount(path: &str, prefix: &str, prefix_out: &str, salt: Option<&[u8]>) -> String {
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

pub(super) fn redact_smb_uri(uri: &str, salt: Option<&[u8]>) -> String {
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

pub(super) fn redact_unc(unc: &str, salt: Option<&[u8]>) -> String {
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
pub(super) fn redact_path_tail(tail: &str, salt: Option<&[u8]>) -> String {
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

pub(super) fn redact_leaf(seg: &str, is_file: bool, salt: Option<&[u8]>) -> String {
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

pub(super) fn dir_token(seg: &str, salt: Option<&[u8]>) -> String {
    match salt {
        Some(s) => format!("<dir:{}>", short_hash(s, seg)),
        None => "<dir>".to_string(),
    }
}

pub(super) fn file_token(seg: &str, salt: Option<&[u8]>) -> String {
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
pub(super) fn short_hash(salt: &[u8], segment: &str) -> String {
    use sha2::{Digest, Sha256};
    use unicode_normalization::UnicodeNormalization;
    let name: String = unescape_debug(segment).nfc().collect();
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(name.as_bytes());
    let bytes = hasher.finalize();
    format!("{:02x}{:02x}{:02x}", bytes[0], bytes[1], bytes[2])
}

pub(super) fn is_safe_parent_dir(name: &str) -> bool {
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
pub(super) fn ends_filename(token: &str) -> bool {
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
pub(super) fn ends_sentence(token: &str) -> bool {
    matches!(
        token.as_bytes().last(),
        Some(b')' | b';' | b',' | b'.' | b'!' | b'?' | b']' | b'}')
    ) && !ends_with_unicode_escape(token)
}

pub(super) fn has_extension_like_suffix(seg: &str) -> bool {
    if let Some(dot) = seg.rfind('.') {
        let ext = &seg[dot + 1..];
        // dot not at position 0 (no `.ssh`) and ext is alnum, <= 8 chars.
        dot > 0 && !ext.is_empty() && ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric())
    } else {
        false
    }
}
