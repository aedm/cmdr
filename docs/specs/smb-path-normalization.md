# SMB paths carry the server's own bytes

**Problem**: Cmdr composed every SMB path to NFC before it reached the wire, so a file whose name the server stores in
NFD was visible in the pane, correctly sized, and impossible to open, copy, rename, or delete (`ERR-VETBX`, app 0.46.1,
macOS 27.0, 2026-09-22; GitHub `vdavid/cmdr-reports` issue 23). On David's NAS that was 712 photos across six albums.

**Decision**: a path that came from the server keeps the server's exact bytes, end to end. A path from anywhere else
resolves against a real listing once, where it enters. Names Cmdr creates on a share go out composed. Ambiguity is
refused, never guessed.

Status: shipped, all six milestones (M0–M5), 2026-09-23. The approved plan is `ce12d6097` on `main`. This file holds
only what David still has to decide; everything durable lives beside the code.

## Where it lives

- The rule, the evidence, the resolve, and the rejected retry ladder: `crates/cmdr-smb/DETAILS.md` § "SMB names are
  opaque bytes" and § "Resolving a foreign path".
- The seams where a foreign path enters (pane open, backend swap, Finder drop and paste, the cursor):
  `apps/desktop/src-tauri/src/file_system/listing/DETAILS.md` § "A pane path the volume stores another way".
- Look-alike writes and the new-name spelling: `apps/desktop/src-tauri/src/file_system/write_operations/DETAILS.md` §
  "Look-alike names" and `apps/desktop/src-tauri/src/file_system/write_operations/transfer/volume/DETAILS.md` §
  "Look-alike names and new-name spelling".

## Open for David

1. **Confirm the new-name policy.** New names Cmdr creates on SMB go out composed (a copy's free name, a new folder or
   file, a rename target, a ` (N)` pick); every other backend keeps names as given. Revert is deleting
   `SmbVolume::composes_new_names`; extending it is one override per backend. The look-alike guard stays either way.
2. **Review the ambiguity copy** (two stored names match a typed path and neither exactly):
   `errors.listing.ambiguousName.*` (the listing error panel) and `errors.volume.ambiguousName` (the inline line), plus
   their 10 agent translations. The Hungarian inline one reads `a(z) „{path}”`, which is clunky.
3. **Whether the conflict dialog should say when a clash is a look-alike.** Today it's an ordinary conflict whose
   destination is the stored entry. Draft if yes: "These names look the same, but the server spells them differently.
   Overwrite replaces the one that's there." (needs a `lookAlike` flag on `WriteConflictEvent`).
4. **Case-only twins stay unguarded, on purpose.** `Report.docx` beside `report.docx` on a case-sensitive share still
   lands as a second entry, as it always has: case is a difference a person can see. Say if that should change.
5. **Favorites keep the user's spelling.** A restored tab or history entry adopts the stored spelling after its first
   listing; a favorite doesn't, so a favorite in another form pays one resolve per click (remembered per share until the
   folder changes).
6. **Not guarded yet:** bulk rename and a compress's archive name take names as given with exact checks only.
