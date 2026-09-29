# Redact

Path-shape-preserving redactor with separate compatibility and report-delivery policies.

The hot path is one composed regex with named capture groups and one dispatch. Unsalted `redact_line` is the stable
compatibility sanitizer for ordinary MCP resources. `RedactionContext::redact_line` is the stricter report boundary:
tokens correlate only within one report and process. Its key derives from an ephemeral process secret plus the report
ID; neither ships. Tests use `RedactionContext::for_test`. Both APIs borrow no-match lines.

The pattern table and overlap rules are in `DETAILS.md`. Typed diagnostic fields use structured `RedactionContext`
methods, which consume complete values, fail closed, and retain report correlation.

## Must-knows

- **Path shape keeps only a fixed mount/home prefix, an allowlisted immediate parent, and a conservative extension.**
  Everything custom collapses.
- **Only a home-prefix path proves the Downloads role in report mode.** `$HOME/Downloads` survives at any depth below
  it; another local or remote segment spelled `Downloads` gets a token. Unsalted callers retain the legacy allowlist.
- **Extensionless leaves become `<dir>`.** This deliberate directory-first heuristic is in `DETAILS.md`.
- **Account wrappers and MTP model names stay; identities go.** Exact match rules are in `DETAILS.md`.
- **A recognized remote reference is redacted as one unit.** SFTP/SSH/WebDAV/HTTP(S)/SMB URLs, scheme-less SMB
  userinfo, and UNC keep scheme, hierarchy, address class, port, and conservative extension; every identity-bearing
  component gets its own token **in report mode**. Unsalted callers retain the legacy SMB/UNC transforms and generic
  userinfo rewrite; newly recognized outer references are rescanned so legacy nested email/IP/mDNS matches still run.
- **Only exact derived-ID shapes are recognized.** Funnel IDs require a known scheme and 16-hex digest;
  `manual-…-<port>` follows its legacy constructor. Never guess from arbitrary hyphens.
- **The path branches over-match on purpose; `split_trailing_noise` finds the real end.** Spaces are legal in labels
  AND filenames, so the boundary is recovered after the match. ❌ Never anchor continuation words to `[A-Z0-9]` (that
  shipped ` at 01.13.03 PM-2.jpeg` verbatim), ❌ never use the looser `has_extension_like_suffix` for its forward scan
  (`.03` in a timestamp halves the name). `DETAILS.md` § "Finding the end of a path".
- **A relative path is only found by its key** (`path_field`: `path=`, `smb_path=`, `from=`, …). ❗ Log a file path or
  name as `key={:?}` with a key from that list, never as bare prose, which is invisible to the redactor.
- **Debug-format producer-owned identities** (`host={host:?}`); literal wrappers let values escape the field.
- **Tokens key on a domain plus the name, not its printed bytes** (escapes undone, NFC, Cmdr temp suffix split off).
  Credentials, query/fragment values, and diagnostic IDs have separate domains from the entities they describe.
- **Typed inputs never trust token-looking syntax.**
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
