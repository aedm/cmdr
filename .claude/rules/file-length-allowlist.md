# Allowlist consent

Warn-only scanners keep JSON allowlists of current sizes (`file-length`, `claude-md-length`, `module-cycles`,
`jscpd-rust` / `jscpd-frontend`, the coverage allowlist), plus the error-level `docs-reachable` and
`desktop-i18n-doc-citations`. They shrink-wrap themselves on local runs, so ❌ don't hand-edit the `files` /
`subsystems` / `pairs` / `tangles` sections: run the check and commit the rewrite.

✅ **Tightening never needs asking**: remove an entry a check now says is unneeded, lower a number, drop an `exempt`.
Refreshing a bundle-size baseline (`desktop-bundle-size`, `website-bundle-size`) is free too: delete the file, re-run,
and report the growth.

❌ **Loosening always needs David's explicit consent**: a new entry, a raised number, a new `exempt`. Bumping one as a
side effect hides growth that trimming or splitting should fix (a `CLAUDE.md` moves depth into its `DETAILS.md`; a jscpd
pair extracts the shared code). A warn is safe to leave, so surface it rather than silence it. `docs-reachable` and
`desktop-i18n-doc-citations` are errors: connect the orphan, or repoint the citation.

Two carve-outs: `invariant-density` is mothballed (no lane runs it, and its allowlist is hand-bumpable), and an
`index-crate-isolation` ceiling rises when the wider surface is genuinely better, said in its `handle/DETAILS.md` and
the commit, ❌ never merely to pass. Mechanics: `scripts/check/checks/DETAILS.md`.
