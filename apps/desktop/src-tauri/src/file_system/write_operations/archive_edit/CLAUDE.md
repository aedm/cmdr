# Archive edits

The driver for writes inside a `.zip`: mkdir, mkfile, rename, delete, transfer, and compress. Existing-archive writes
are O(archive) temp+rename rewrites. Up: `../CLAUDE.md`; mutation: `crates/cmdr-archive/src/mutation/DETAILS.md`.

## Module map

- `routing.rs`: the shared primitives every route builds on — inner-path helpers, `ensure_zip_writable` (the one
  write-side chokepoint refusing tar/7z), `archive_inner_exists` (the duplicate pre-check), the instant-op sink builder.
- `driver.rs`: `archive_edit_start` (the managed op's whole lifecycle) plus `route_archive_delete`. `engine.rs`:
  `run_managed_edit`, the local-vs-remote dispatcher, and `remote.rs` its remote leg (pull, apply, upload, swap).
  `edit_error.rs`: `EditError`, the leaf both of them return. `conflicts.rs`: resolution against the archive index.
- Per-shape routes: `copy_into.rs` (`route_archive_copy_into`, plus the remote-source pull), `move_out.rs`,
  `compress.rs` (current routed fallback), and `fresh_zip.rs` (seedless bounded producer). The create and rename routes live with their instant ops in
  `../create.rs` and `../rename.rs`, and call in here.

## Must-knows

- **An archive edit is MANAGED, never instant**: it goes through `spawn_managed`, takes the PARENT drive's lane, and
  marks that drive busy. A `create` / `rename` returns an operation id, ❌ not a path.
- **Every apply site runs through `engine::run_managed_edit`**, ❌ never a bare `spawn_blocking(mutator::apply(...))` —
  that dispatcher is what makes one closure work for both a local and a remote parent.
- **❌ No in-place remote edit.** A remote parent (direct SMB / MTP) goes pull → apply locally → upload to a temp name →
  swap, and the remote ORIGINAL keeps its bytes until that final swap. Keep the four steps in that order and keep the
  cleanup on every early exit; the swap's shape depends on whether the backend allows same-name siblings. DETAILS §
  "Remote edit: the data-safety contract".
- **Routing detection must be PARENT-AWARE**: the seams call the async `VolumeManager::path_is_inside_archive` /
  `path_crosses_archive_boundary`, ❌ never the sync `std::fs`-only predicates, which answer FALSE for an `smb://` /
  `mtp://` path and drop the write onto the parent volume.
- **The empty-zip seed is LOAD-BEARING for the current compress fallback**: `ZipArchive::new` rejects a 0-byte file,
  so a brand-new target gets a valid 22-byte archive before the managed rewrite. This means M1 does NOT make creation
  safe before registration; removing the seed belongs to the dedicated fresh-create path. DETAILS § Compress.
- **`fresh_zip` has explicit terminal status**: EOF is not success. Drain, await `finish`, and join every worker.
- **Compress progress has two different byte axes**: `Compressing` is uncompressed source bytes; remote
  `Transferring` is completed-ZIP bytes. Both finishing phases clear BOTH totals and ETA. Ordinary archive mutation
  stays `ArchiveEdit` + `Copying`.
- **Move OUT deletes only what durably landed**: extract first, then ONE batch `{ delete }` rewrite over the sources
  that extracted with ZERO deep skips (a hard error deletes the durable prefix; cancel and rollback delete nothing).
  The copy engine's deep `skipped_file_count` fold is what makes that count honest.
- **Unrepresentable entries (symlinks, fifos, devices, broken links) are SKIPPED, never lost**, and any skip suppresses
  a move's source deletion. Every skip increments `skipped_count` and surfaces as `files_skipped`.
- **Conflicts are planned INSIDE the op**, against the working copy `run_managed_edit` hands the closure — planning up
  front would break a remote edit. Stop-mode prompts per FILE (dirs merge silently), storing the sender BEFORE the emit.
- **The terminal `files_processed` is `MutationProgress::entries_changed`**, ❌ not `entries_total`: deleting one file
  from a 3-entry zip reports 1.
- **Compression level rides on the `Changeset`** (from the `behavior.archiveCompressionLevel` setting) and applies to
  newly ADDED entries only; `None` means the crate default.

Routing detail, the remote-edit contract and its stale-temp reap, the per-op changesets, compress, move-out, conflicts,
and the mutation-test coverage: `DETAILS.md`. Read it before any non-trivial work here: editing, planning,
reorganizing, or advising.
