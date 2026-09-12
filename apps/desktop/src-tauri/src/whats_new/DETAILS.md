# What's new parser details

Depth and rationale. `CLAUDE.md` holds the must-knows; this is the exact parse contract.

## What the parser captures

- Recognizes `## [x.y.z] - YYYY-MM-DD` release headings top-down. Skips the top `## [Unreleased]` block (no date, not a
  release) and ends the current release on any other H2.
- Captures the **lead** (the block between the heading and the first `###`) and the Added / Changed / Fixed / Security
  sections, in changelog order. Drops `Non-app` and any unknown section name.
- Omits a release that has no lead AND no displayable section.

## Rendering: CommonMark, backend-side

The IPC model carries HTML (`lead_html`, `entries_html`), rendered by `pulldown-cmark` with strikethrough and tables on,
the extensions the website's marked (GFM mode) also honors. The changelog renders as CommonMark in all three places it
shows up: the app popup, getcmdr.com, and GitHub releases.

- **The lead is block HTML, rendered from its raw source lines.** `build_lead` joins the lines between the heading and
  the first `###` with `\n` and renders them, nothing else. Paragraphs become `<p>`, a numbered list of highlights an
  `<ol>`, and bullets indented under a highlight a nested `<ul>` inside its `<li>`. A wrapped continuation line joins its
  item by CommonMark's own rules.
- **An entry is inline HTML.** `render_inline_html` drops the paragraph events, because the dialog wraps each entry in
  its own `<li>`.
- **Why the backend renders, and why never snarkdown.** 0.44.0's lead (two paragraphs, then a numbered list nesting
  three bullets) shipped as one `<ul>` whose first bullet was empty. Two layers broke it: a pre-pass here trimmed every
  line (dropping the indentation that nests the bullets), and the frontend's snarkdown can't nest lists, turns blank
  lines into `<br />`, and merges consecutive list lines into ONE list typed by the LAST marker. The website looked fine
  (marked over the raw file), so nothing flagged it. Rendering here keeps business logic in Rust and puts the output
  under Rust tests.
- **What guards it.** `real_changelog_renders_as_commonmark_blocks` walks the latest five real releases: each lead must
  equal `render_block_html` over its raw source slice (found independently of the parser's walk, so any pre-pass fails
  it), be block HTML, carry no `<br`, and have no `<li>` opening straight into a nested list; no entry may render as a
  block.

## Per-entry post-processing

- Joins wrapped continuation lines.
- Strips the trailing `(hash, hash, …)` commit group. One entry may carry several comma-separated hashes wrapped
  across source lines, so the stripper matches the whole variable-length trailing parenthetical structurally, only
  when every comma-separated item inside is a bare hash (6-40 lowercase hex chars). A real trailing aside like
  `(~40x speed-up!)` survives. The 6-40 range is the shared contract with the `changelog-commit-links` check and the
  website's linkifier (`apps/website/src/lib/changelog.ts`); `scripts/check/checks/DETAILS.md` § "CHANGELOG commit
  refs" owns the rule and the reasoning behind its floor.
- Flattens markdown links to their label. Bold / italic / `code` stay, and render to inline HTML with the rest of the
  entry (special characters escaped).

## Version comparison

Uses the `semver` crate, so `0.9.0 < 0.10.0` (not string order).

## No `### Development history` cutoff

There's deliberately no special case for the `Development history` block. The slice never collects more than `max`
(≤ 5) releases, so the walk stops well before that block. Don't add a cutoff.
