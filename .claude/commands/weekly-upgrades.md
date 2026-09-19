Do this week's dependency pass, the one Renovate structurally can't: the `desktop non-major updates` and
`all major updates` lanes. Read @docs/guides/update-dependencies.md first; it's the canonical procedure and the
rationale. This file is only the wrapper around it.

Run unattended, start to finish, without asking David anything. He's reading the result, not supervising the run.

1. **Worktree.** `~/.claude/scripts/new-worktree.sh weekly-upgrades`, then `EnterWorktree` with the path it prints. Wait
   for `.warming-worktree` to disappear before the first build.
2. **Do the pass** per the guide: dashboard for the list, npm (`ncu -u`, `pnpm install`, `pnpm dedupe`), Cargo
   (`cargo outdated -w`, then per-crate `Cargo.toml` edit + `cargo update -p <crate> --precise <version>`, 3-day window
   enforced by hand against crates.io), fix the breakage, `pnpm check third-party-notices`, `pnpm check --include-slow`.
3. **Commit in logical pieces**, not one blob: npm bumps, Cargo bumps, and each non-trivial API fix separately. Lead
   with impact. A crate you had to hold back gets a line in the body saying which and why.
4. **Merge and tear down.** `ExitWorktree` with `keep`, FF-merge the branch into local `main`, verify `main` moved, then
   `~/.claude/scripts/remove-worktree.sh weekly-upgrades`. Don't push; David pushes on his own schedule.
5. **Report** in about ten lines: what moved (counts per ecosystem, and name anything interesting), what broke and how
   you fixed it, what you held back and why, and whether the slow lane went green.

**When it's a quiet week**, say so in two lines and stop. An empty dashboard is a fine outcome; don't manufacture work.

**When something needs David**, finish everything else first, commit it, and put the question at the end of the report.
A single crate needing an architectural call doesn't block the other forty.
