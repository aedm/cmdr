# Redact: details

Depth and rationale. `CLAUDE.md` holds the must-knows and the pattern table.

## Pattern table

| Group | Matches | Rewrites to |
| --- | --- | --- |
| `unix_home` | `/Users/<user>/...`, `/home/<user>/...` | `$HOME/<allowlisted-parent-or-dir>/<file>.<ext>` |
| `win_home` | `C:\Users\<user>\...` | `$HOME\<allowlisted-parent-or-dir>\<file>.<ext>` |
| `unix_system` | `/tmp/`, `/var/`, `/private/`, `/opt/` | prefix kept; tail walked with same shape rules |
| `volumes` | `/Volumes/<label>/...` (spaces allowed) | `/Volumes/<volume>/<allowlisted-or-dir>/<file>.<ext>` |
| `media` | `/media/<label>/...` (spaces allowed) | `/media/<volume>/<allowlisted-or-dir>/<file>.<ext>` |
| `smb_uri` | `smb://host/share/...` | `smb://<host>/<share>/<redacted tail>` |
| `unc` | `\\host\share\...` | `\\<host>\<share>\<redacted tail>` |
| `url_userinfo` | `scheme://user[:pass]@host/...` | `scheme://<userinfo>@host/...` (host kept) |
| `bare_userinfo` | `//user[:pass]@host/...` (no scheme) | `//<userinfo>@host/...` (host kept) |
| `email` | `local@domain.tld` | `<email>` |
| `account` | `user=`/`username:` fields | `user=<user>`, `None` untouched |
| `mdns` | `<label>.local` | `<host>.local` |
| `ipv4` | dotted-quad, valid octet ranges | `<ipv4>` |
| `ipv6` | full + compact forms (`::1`, `fe80::1`) | `<ipv6>` |
| `mtp_owner` | `<Owner>'s <Model>` device names | `<mtp-owner>'s <Model>` (model kept) |

`mtp_owner` needs a known model word (`iPhone | iPad | Pixel | Galaxy | OnePlus | …`) right after the `'s `, which is
what keeps contractions and module paths out of it. `SAFE_PARENT_DIR_NAMES` is the parent-dir allowlist: `Documents`,
`Downloads`, `Desktop`, `Library`, `src`, `Pictures`, `Movies`, `Music`, `Public`, `AppData`, `Application Support`.

## Decision: path-shape preservation + allowlist

The tradeoff is debuggability ("I can see this is a Documents path") against PII safety ("but I don't want to leak
project codenames"). The allowlist captures the dirs that are near-universal across users; anything custom collapses.
Net result: triagers can usually guess the failure context without seeing the user's secrets.

## Decision: an extensionless leaf reads as `<dir>`

`has_extension_like_suffix` decides whether a path's last segment becomes `<file>` or `<dir>`, so an extensionless file
(`id_rsa`, `README`, `Makefile`) is mislabeled `<dir>`. Accepted: Cmdr's logs are dominated by directory listings, so
guessing `<dir>` is right far more often on real triage data than the reverse.

## Decision: MTP owner names redacted, model names kept

`mtp_owner` catches the common `<Owner>'s <Model>` shape (`John's Pixel 8 Pro`). The owner becomes `<mtp-owner>`; the
model phrase (`Pixel 8 Pro`, `iPhone 15 Pro`) is kept because model strings alone aren't identifying and are useful
diagnostic context. The pattern requires both a capitalized possessive AND a model word from a known set
(`iPhone | iPad | Pixel | Galaxy | OnePlus | Note | Tablet | Phone | Camera | ...`) right after the `'s `, which keeps
English contractions (`it's a Pixel`) and module paths (`cmdr_lib::mtp::device`) untouched. `That's Pixel 8 Pro` does
match, accepted as an over-redaction (rare phrasing without an article between `'s` and the model word).

## Decision: account names redacted, share names kept

An SMB login has three parts in our logs, and they don't carry the same weight. The **account name** is a real
identifier, as personal as the email pattern, and it appeared verbatim in three places (`commands/network.rs` twice,
`crates/cmdr-smb/src/connection.rs` once), so `account` collapses it to `<user>`. The **share name** appears in ~40 debug lines
and is what makes a bundle readable ("which share was this?"); a share is usually a generic label (`media`, `public`,
`backups`), so it ships as-is. **Hosts** were already handled for the shapes that identify a person or a network
(`mdns`, `ipv4`, `ipv6`, `smb_uri`); a bare NetBIOS name (`server=NASPOLYA`) still ships, which is the known remaining
gap here.

`None` passes through because it isn't a name, and the difference between "no username in the mount info" and "a
username we then looked up in the Keychain" is exactly what a triager reads these lines for. That's why this can't be a
blanket `user=\S+` rewrite.

## Finding the end of a path

A space can be inside a path (`/Volumes/My Backup Drive`), inside a filename (`Invoice for Acme Corp.pdf`), or the gap
between a path and the prose after it. Nothing local to the character tells the three apart, so the regex takes the
greedy option and `split_trailing_noise` hands back what wasn't path. Both directions of getting that wrong have shipped:

- **Too cautious** and the leftover is a filename fragment that goes out in a bundle. Anchoring continuation words to
  `[A-Z0-9]` stopped `Screenshot 2026-09-04 at 01.13.03 PM-2.jpeg` at ` at`, so every multi-word filename leaked its
  tail. `Invoice for Acme Corp.pdf` is the same shape with something to lose.
- **Too greedy** and the line loses its message. A label group that swallowed following lowercase words truncated 98
  uploaded reports at `/Volumes/<volume>`, taking the volume ID and the resolution with it.

The boundary rules, in order, each earning its place against one of those:

1. **The `": "` seam.** Nearly every caller formats `{path}: {message}`. macOS forbids `:` in a filename, so cutting
   there can't shorten a real name; on Linux a `: ` inside one is vanishingly rare.
2. **Forward scan to the first token that ends the path**: one carrying a letter-led extension (`report.pdf`), or one
   ending a sentence (`naspi-1)`, `state.`). Forward, not backward, is the load-bearing part:
   `report.pdf for alice@example.com` has to cut after `report.pdf`, and from the right the email's `.com` is
   indistinguishable from a real extension.
3. **`ends_filename` demands a LETTER-led extension**, unlike the looser `has_extension_like_suffix` used for the
   `<dir>`/`<file>` decision. `01.13.03` in a timestamp otherwise reads as an extension and halves the name. A digit-led
   real extension (`.7z`) just doesn't end the scan, which costs a word of prose, never a leak.
4. **Backward trim of a lowercase extension-less run**, floored at the first segment after the last `/`. That floor is
   what leaves `/Volumes/naspi and then it failed` as `naspi` rather than nothing.

### Why the scan resumes mid-match

`redact_with` walks the line itself rather than calling `replace_all`, resuming at `match.start() + consumed` where
`consumed` is the path length without the noise. `replace_all` resumes after the WHOLE match, so every handed-back byte
was skipped by the scanner: `/Volumes/d/f.txt and smb://host/share/x.txt` pulled `smb:` into the volume match, gave it
back as literal text, and left `//host/share/x.txt` with no pattern willing to claim it. The share name and filename
shipped. Any future branch that hands text back needs the same treatment, which is why `dispatch` returns a length.

### Known gap: a filename repeated in prose

macOS puts the filename in its own error text as well as in the path (`the Trash refused it: “Screenshot ….jpeg”`).
That copy has no path around it and no pattern claims a bare name, so it still ships. Closing it means a second pass
that redacts verbatim repeats of a segment already recognized on the same line, keyed on the segment being long enough
and filename-shaped to be worth matching. Pinned by `trash_refusal_line_redacts_its_path`.

## How to add a new pattern

1. Add a new alternative inside `redactor_regex()` with a unique `(?P<group_name>...)` and write a corresponding
   rewriter (or extend `dispatch`) to map matches to redacted output.
2. Add a dedicated test in `tests.rs` with at least six input→expected tuples covering edge cases (start of line, middle
   of line, embedded in punctuation, multiple per line).
3. Append two or three lines to `fixtures/log-corpus.txt` exercising the new pattern, and update
   `fixtures/log-corpus.redacted.txt` to match. The `replacement_count_histogram` test flags a corpus missing your
   pattern.

## Regex and line-splitting notes

- `redact_text` splits on `\n` and redacts each line independently. This keeps regex `\b` anchors predictable and lets
  us return `Cow::Borrowed` per line.
- Verbose regex mode (`(?x)`) ignores whitespace OUTSIDE character classes. Inside `[...]` whitespace is literal, so
  `[A-Za-z]` is fine but `[ A-Za-z ]` would match a space.
- Paths with embedded spaces (`/Volumes/My Backup Drive/...`) match by allowing single spaces between path components.
  Multi-space gaps stop the match. Where the match then actually ends: § "Finding the end of a path".
