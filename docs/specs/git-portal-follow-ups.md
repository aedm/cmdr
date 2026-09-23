# Git portal follow-ups

The virtual `.git` portal is a read-only routed volume in `crates/cmdr-git`, plus a pane-only listing overlay for the
`.git/` root, and `LocalPosixVolume` names git nowhere. How it's built and why: `crates/cmdr-git/DETAILS.md` § "The
portal is a routed volume" and `apps/desktop/src-tauri/src/file_system/git/DETAILS.md` § "Two seams, no hooks". What's
left is one by-hand pass in the real app.

## 1. Manual QA of the routed git portal

- **Problem**: the portal moved from ten `if` sites inside `LocalPosixVolume` to a routed volume plus a listing overlay.
  The bytes and the folder shape of a copy-out are automated (`apps/desktop/test/e2e-playwright/git-portal.spec.ts`,
  plus a crate cell for the executable bit), but nobody has walked the rest in a running app.
- **Impact**: the risky paths are the ones touching real data: editing and deleting real files under `.git/`, and
  deleting a whole repo on a non-boot volume, which before the routing left `.git/` behind half-deleted.
- **Solution**: David, in a running app: (1) browse each of the six categories (`branches/`, `tags/`, `commits/`,
  `stash/`, `worktrees/`, `submodules/`) in a repo's `.git/`; (2) copy a file out of a branch tree to another volume (a
  real cross-device copy) and check the executable bit survives; (3) edit `.git/config` in place, then rename and delete
  a real file under `.git/`; (4) delete a whole repo folder, on the boot disk and on an external one; (5) toggle the
  portal off and on with a `.git/` pane open, and with a pane standing inside `.git/branches/`; (6) open a linked
  worktree's `.git`: the categories below it answer, and the landing listing doesn't (by design,
  `crates/cmdr-git/DETAILS.md` § "Linked worktrees"). File a bug per failure.
- **Size**: S, about half an hour of David's time. Blocked on David.
