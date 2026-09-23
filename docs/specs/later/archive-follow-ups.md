# Archive follow-ups

Browsing and editing archives shipped: `crates/cmdr-archive/` reads zip, the tar family, and 7z (encrypted ones
included), and `apps/desktop/src-tauri/src/file_system/write_operations/archive_edit/` mutates a zip through a staged
temp+rename, local or remote-hosted. Those two `CLAUDE.md` / `DETAILS.md` pairs are the canonical account. The three
items below are independent, re-verified against the code on 2026-09-23.

## 1. Add a file to a big zip without rewriting the whole archive

- **Problem**: every zip edit is an O(archive) temp+rename rewrite. Adding a 1 MB file to a 2 GB zip rewrites 2 GB, and
  for a NAS-hosted zip it round-trips the whole archive over the wire twice.
- **Impact**: low today (temp+rename is correct, only slow); grows with big archives and NAS-hosted zips.
- **Solution**: "clone + tail-rewrite", settled in `docs/notes/m-append-spike.md` (which carries the measurements and
  the reader-compatibility matrix): clone the archive to a sibling temp, truncate at the old central-directory offset,
  append the new entries plus a fresh contiguous CD and EOCD, then atomic rename. Only tail-ADDs get the fast path;
  delete, rename, and mid-archive edits stay on today's mutator, which physically removes bytes. ❌ Not append-past-EOF:
  `ditto -x -k` forward-scans local headers and silently drops every appended entry, and CD-rewrite deletes leave
  deleted bytes recoverable. For an SMB-hosted zip, the server-side half is a copychunk of the retained prefix plus an
  upload of only the new tail: `smb2` already exposes both (`server_side_copy_file_range` and `create_file_writer_at`,
  since 0.13.0; Cmdr pins 0.24.2), so it's no longer blocked on the crate. Old Samba and NAS firmware may lack copychunk
  (`ErrorKind::Unsupported`), so probe and fall back to today's pull round trip. Before shipping: a manual Quick Look
  and Spotlight check on a machine with a foreground, and property tests for the zip64 and data-descriptor paths.
- **Size**: M for local (two to three days), plus M for the SMB half. Trigger: real archives feeling slow, or the NAS
  case starting to matter.

## 2. Open a file inside an archive in an external app

- **Problem**: Enter on a file inside an archive opens the built-in viewer (temp-extract, byte-capped, per-instance
  reaper); "Open with <external app>" isn't offered. A detached launched app holds the file for an unknown lifetime and
  has no close event to hook, so it can't reuse the viewer's session-scoped extract (`crates/cmdr-archive/DETAILS.md` §
  "Left for later").
- **Impact**: medium. Opening a PDF or image from inside a zip in its real app is a common expectation.
- **Solution**: clone the viewer's persist-extract module (`apps/desktop/src-tauri/src/file_viewer/materialize.rs`,
  described in `file_viewer/DETAILS.md` § "Per-instance extract dir + startup reaper") into a sibling open-with module
  with a startup-ONLY reaper, its own per-instance dir under the app data dir with its own prefix (so neither reaper can
  touch the other's live temps), and a uuid subdir per open. The launch path swaps each archive-inner path for its fresh
  temp in `menu/menu_handlers.rs`'s `open-with:` and `OPEN_WITH_OTHER_ID` branches before `open_paths_with`. The app
  data dir isn't TCC-protected, so the reaper needs no Full Disk Access guard. Rejected: a session or refcount temp (no
  close event), a dedup cache keyed by inner path (costs archive-edit invalidation for nothing), a TTL reaper (the
  process boundary already marks prior-run temps abandoned), a shared extract dir (a second instance's reap would delete
  the first's live temps), and write-back when the external app saves (inner files are read-only preview). One unknown:
  candidate apps are listed at menu-build time (`file_system/open_with.rs::compute_open_with_choices`) against the inner
  path, which isn't a real file; LaunchServices probably maps extension to UTType without a stat, and if not, list
  against a path with the same extension.
- **Size**: S (about a day). Clear win.

## 3. Edit a zip on an MTP device without pulling and pushing the whole archive

- **Problem**: a remote zip edit is pull-edit-upload-swap (`write_operations/archive_edit/DETAILS.md`), which for MTP is
  O(archive) over USB in both directions.
- **Impact**: low. MTP zip editing is rare, and the slow path is correct.
- **Solution**: in-place editing through `mtp-rs` (David's crate, so the API can be added), for example a partial-object
  write for the tail-rewrite shape in item 1, where the device supports it.
- **Size**: L. Stretch item; trigger is MTP zip editing seeing real use.
