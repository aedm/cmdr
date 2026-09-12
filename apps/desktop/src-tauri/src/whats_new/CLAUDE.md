# What's new parser

Parses the repo-root `CHANGELOG.md` into a typed, user-facing model for the post-update "What's new" popup, with each
lead and entry rendered to HTML by `pulldown-cmark` (CommonMark). Exposes at most the five newest in-range releases;
older notes live on the website.

## Module map

- `mod.rs`: types, the top-down parser, entry post-processing, CommonMark rendering,
  `releases_between(since, current, max)` slicing, and a `OnceLock` cache over the embedded changelog.
- `tests.rs`: fixture tests plus two tests over the real embedded file: `smoke_real_changelog_parses` (the canary if the
  format drifts) and `real_changelog_renders_as_commonmark_blocks` (what the popup shows).
- IPC: `../commands/whats_new.rs` (thin `get_whats_new` + `whats_new_dev_override`).

## Guardrails

- **The changelog is the single source of truth; fix bad formatting THERE, never grow fix-up logic here.** Whatever
  lands in a release's lead and its Added / Changed / Fixed / Security sections renders verbatim. The parser only strips
  machinery the user shouldn't see (the trailing commit-hash group, `Non-app` and unknown sections) and flattens
  markdown links to their text. Teaching it to "clean up" garbled entries would make it a second source of
  truth that rots the moment the two disagree.
- **The lead reaches the renderer untouched, and the frontend adds no markdown pass.** No trimming or line re-joining:
  indentation is what nests a list. A trim pass plus the frontend's snarkdown once shipped 0.44.0's lead as one broken
  `<ul>`. `real_changelog_renders_as_commonmark_blocks` compares each real lead against its raw source.
- **Resilience over strictness.** Malformed input must never panic or block startup: skip what doesn't parse, log at
  debug (`target: "whats_new"`), show what does.

## Gotchas

- **`include_str!` runs up five levels** (`../../../../../CHANGELOG.md`). Moving files breaks it at compile time, which
  is the good failure mode.
- **Dev-mode staleness.** Editing `CHANGELOG.md` does NOT trigger a `pnpm dev` rebuild (the root `.taurignore` excludes `*.md`,
  and the changelog is embedded, not read at runtime). Cargo tracks the `include_str!` input, so the next real
  `cargo build` picks it up. Don't add the file to the watcher: it would restart the app on every changelog edit.
- **No runtime I/O.** The changelog is embedded, so the commands can't hang and intentionally skip `blocking_with_timeout`.

Full details (the exact parse contract: heading recognition, lead capture, the variable-length commit-hash stripper,
semver comparison, the no-`Development history`-cutoff reasoning): `DETAILS.md`.
