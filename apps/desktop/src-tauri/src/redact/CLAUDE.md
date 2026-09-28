# Redact

Path-shape-preserving redactor shared by the crash reporter and the error reporter.

The hot path is `redact_line`, called once per log line: one composed regex with named capture groups, single pass, a
dispatch calling the matched group's rewriter. `Cow::Borrowed` for no-match lines (zero alloc).
`RedactionContext::redact_line` is the report mode: tokens carry 12 lowercase hex characters and correlate only for
one report in one process. Its key derives from an ephemeral process secret plus the report ID; neither key nor secret
ships. Tests use `RedactionContext::for_test`, never process-global overrides.

The pattern table and overlap rules are in `DETAILS.md`. `redact_name` handles a bare name with no line around it.

## Must-knows

- **Path shape keeps only a fixed mount/home prefix, an allowlisted immediate parent, and a conservative extension.**
  Everything custom collapses.
- **Only a home-prefix path proves the Downloads role in report mode.** `$HOME/Downloads` survives at any depth below
  it; another local or remote segment spelled `Downloads` gets a token. Unsalted callers retain the legacy allowlist.
- **Extensionless leaves become `<dir>`.** This deliberate directory-first heuristic is in `DETAILS.md`.
- **Account wrappers and MTP model names stay; identities go.** Exact match rules are in `DETAILS.md`.
- **A recognized remote reference is redacted as one unit.** SFTP/SSH/WebDAV/HTTP(S)/SMB URLs, scheme-less SMB
  userinfo, and UNC keep scheme, hierarchy, address class, port, and conservative extension; every identity-bearing
  component gets its own token. Valid URLs use `url`; recognizable malformed forms use the same bounded splitter.
- **Only exact current derived-ID shapes are recognized.** `smb`/`sftp`/`webdav`/`adb`/`mtp`/`vol`/`path` require the
  ID funnel's 16-hex digest; `manual-…-<port>` follows its legacy constructor. Never guess from arbitrary hyphens.
- **The path branches over-match on purpose; `split_trailing_noise` finds the real end.** Spaces are legal in labels
  AND filenames, so the boundary is recovered after the match. ❌ Never anchor continuation words to `[A-Z0-9]` (that
  shipped ` at 01.13.03 PM-2.jpeg` verbatim), ❌ never use the looser `has_extension_like_suffix` for its forward scan
  (`.03` in a timestamp halves the name). `DETAILS.md` § "Finding the end of a path".
- **A relative path is only found by its key** (`path_field`: `path=`, `smb_path=`, `from=`, …). ❗ Log a file path or
  name as `key={:?}` with a key from that list, never as bare prose, which is invisible to the redactor.
- **Tokens key on a domain plus the name, not its printed bytes** (escapes undone, NFC, Cmdr temp suffix split off).
  Credentials, query/fragment values, and diagnostic IDs have separate domains from the entities they describe.
- **`redact_with` resumes at `match.start() + consumed`, ❌ never `replace_all`.** A handed-back tail must face the
  scanner again or nothing else can claim it: one match ate `smb:` and shipped the share and filename. A new branch
  that hands text back owes `dispatch` a consumed length.

## Gotchas

- **A filename repeated in prose is still not redacted.** macOS names the file again inside its own error text
  (`the Trash refused it: “Screenshot ….jpeg”`), with no path around it, and no pattern claims a bare name. This gap is
  pinned by `trash_refusal_line_redacts_its_path`.
- **Dispatch order mirrors regex alternation order.** Complete recognized URLs must run before legacy generic-userinfo
  fallback, and derived IDs before scalar IP matching. See `DETAILS.md`.

## Files

`mod.rs` (public API, composed regex, dispatch), `context.rs` (report key + token domains), `paths.rs` (path rewriters),
`references.rs` (remote references + diagnostic IDs), `fields.rs` (keyed fields), `names.rs` (printing normalization),
`tests.rs` + `reference_tests.rs` (matrix, idempotency, golden corpus, histogram),
`fixtures/log-corpus.txt` + `.redacted.txt` (golden snapshot).

Full details (decision rationale, how to add a pattern, regex verbose-mode notes): `DETAILS.md`.
