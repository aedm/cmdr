# Allowlist consent

Scanners keep JSON allowlists of current sizes and shrink-wrap themselves on local runs, so ❌ never hand-edit a number:
run the check, commit the rewrite. The `reason` beside it is yours to write.

✅ **Tightening never needs asking**: drop an entry a check calls unneeded, lower a number, drop an `exempt`. A
bundle-size baseline (`desktop-bundle-size`, `website-bundle-size`) refreshes free too: delete it, re-run, report the
growth.

✅ **`file-length`, `claude-md-length`, and `jscpd-rust` / `jscpd-frontend` FAIL, and the call is yours**: always split
in case of a genuine architectural win, bump otherwise. Every entry carries a `reason` for that call, or
`TODO: <why it should be split, and how>` when you bumped one that shouldn't stay; say which in the commit.

❌ **Every other allowlist needs David's explicit consent to loosen**: a new entry, a raised number, a new `exempt`.
That covers `module-cycles`, the coverage allowlist, and the error-level `docs-reachable` /
`desktop-i18n-doc-citations` (connect the orphan, or repoint the citation).

Two carve-outs: `invariant-density` is mothballed (no lane runs it, its allowlist is hand-bumpable), and an
`index-crate-isolation` ceiling rises when the wider surface is genuinely better, said in `handle/DETAILS.md` and
the commit, ❌ never merely to pass. Mechanics: `scripts/check/checks/DETAILS.md`.
