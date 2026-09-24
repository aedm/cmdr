# Redact

Path-shape-preserving redactor shared by the crash reporter and the error reporter.

The hot path is `redact_line`, called once per log line: one composed regex with named capture groups, single pass, a
dispatch calling the matched group's rewriter. `Cow::Borrowed` for no-match lines (zero alloc).
`redact_line_salted(line, &salt)` is the same pipeline with a per-bundle salt: `<dir>` / `<file>` become
`<dir:HHHHHH>` / `<file:HHHHHH>` (`sha256(salt || segment)[..3]`), so equal segments correlate within one bundle only.
The builder mints a fresh 16-byte random salt per build; the salt never ships.

Sixteen named groups, one per pattern class: four path shapes (`unix_home`, `win_home`, `unix_system`, `volumes` /
`media`), three share shapes (`smb_uri`, `unc`, `url_userinfo` / `bare_userinfo`), the keyed `path_field`, and the scalar
ones (`email`, `account`, `mdns`, `ipv4`, `ipv6`, `mtp_owner`). What each matches and rewrites to: `DETAILS.md` §
Pattern table. `redact_name` is the one entry point for a bare name with no line around it (the error reporter's
state snapshot).

## Must-knows

- **Path-shape preservation keeps three things, collapses the rest.** The mount/home prefix as a fixed token (`$HOME`,
  `/Volumes/<volume>`, `/tmp/`, …); the immediate parent dir name IF allowlisted (`SAFE_PARENT_DIR_NAMES`); and the
  extension if ≤ 8 ASCII alnum chars. So `/Users/john/Documents/budget.pdf` → `$HOME/Documents/<file>.pdf`, but
  `/Users/john/SecretProject/budget.pdf` → `$HOME/<dir>/<file>.pdf`.
- **Leaf `<dir>` vs `<file>` is decided by `has_extension_like_suffix`** (`.X`, 1-8 alnum, dot not at position 0). An
  extensionless file (`id_rsa`, `README`) is mislabeled `<dir>`; that trade-off is deliberate, see `DETAILS.md`.
- **Account names go, the field shape stays.** The key must be exactly `user` / `username` (`max_users=`,
  `parent_user=` don't match); the `Some("…")` / quote wrapper is re-emitted and `None` passes through. Share names and
  bare NetBIOS hosts stay readable by decision: `DETAILS.md`.
- **MTP owner redacted, model kept.** `mtp_owner` needs a capitalized possessive AND a known model word right after
  `'s `, leaving contractions (`it's a Pixel`) and module paths alone. A bare model name isn't identifying and stays.
- **The path branches over-match on purpose; `split_trailing_noise` finds the real end.** Spaces are legal in labels
  AND filenames, so the boundary is recovered after the match. ❌ Never anchor continuation words to `[A-Z0-9]` (that
  shipped ` at 01.13.03 PM-2.jpeg` verbatim), ❌ never use the looser `has_extension_like_suffix` for its forward scan
  (`.03` in a timestamp halves the name). `DETAILS.md` § "Finding the end of a path".
- **A relative path is only found by its key** (`path_field`: `path=`, `smb_path=`, `from=`, …). ❗ Log a file path or
  name as `key={:?}` with a key from that list, never as bare prose, which is invisible to the redactor.
- **Hashes key on the name, not its bytes** (escapes undone, NFC, Cmdr temp suffix split off): one file, one token.
- **`redact_with` resumes at `match.start() + consumed`, ❌ never `replace_all`.** A handed-back tail must face the
  scanner again or nothing else can claim it: one match ate `smb:` and shipped the share and filename. A new branch
  that hands text back owes `dispatch` a consumed length.

## Gotchas

- **A filename repeated in PROSE is still not redacted.** macOS names the file again inside its own error text
  (`the Trash refused it: “Screenshot ….jpeg”`), with no path around it, and no pattern claims a bare name. Known gap,
  pinned by `trash_refusal_line_redacts_its_path`.
- **Dispatch order mirrors the regex alternation order**, and the userinfo branches depend on it: `DETAILS.md` §
  "Pattern overlaps".

## Files

`mod.rs` (public API, composed regex, dispatch, `split_trailing_noise`), `paths.rs` (path-shape rewriters),
`fields.rs` (keyed fields), `names.rs` (temp suffixes, `{:?}` escapes), `tests.rs` (per-pattern, idempotency, golden corpus, histogram),
`fixtures/log-corpus.txt` + `.redacted.txt` (golden snapshot).

Full details (decision rationale, how to add a pattern, regex verbose-mode notes): `DETAILS.md`.
