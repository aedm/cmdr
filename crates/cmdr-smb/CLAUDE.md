# `cmdr-smb`

Everything Cmdr says to an SMB server: the `SmbVolume` backend and the protocol layer under it. No `tauri`, no app.
Discovery, the keychain, mounts, upgrades, and every human-facing word stay in the app's `network/`.

## Module map

- `src/volume/`: the backend — `mod.rs` (structs), `volume_impl.rs` (the whole `impl Volume`), and one module per
  concern (`paths`, `query`, `mutation`, `session`/`reconnect`/`state`, `scan`/`scan_pool`, `streams`, `mapping`,
  `spelling`, `foreground_yield`, `liveness`, `watcher/`, `testing` for the Docker fixtures).
- `src/{types,errors,connection}.rs`: share-listing vocabulary, `smb2::Error` classification, the address builder.
  Re-exported at the root (`cmdr_smb::`).

## Backend must-knows

- **Everything the backend asks the app goes through the `VolumeHost`** taken in `connect_smb_volume`, kept on
  `SmbVolumeInner` (seams: `crates/cmdr-fs/src/volume/host/CLAUDE.md`). Background work spawns onto `host.runtime()`.
- **The watcher runs on a DEDICATED session** (stacked CHANGE_NOTIFY long-polls wedge Samba) **and never reconnects
  itself**: on death it kicks the ONE reconnect path (`spawn_watcher_death_reconnect`), which respawns it and resumes
  the index. ❌ No second loop, no cancel on pane close.
- **`SmbVolume` is a per-mount-root instance over a shared `Arc<SmbVolumeInner>`**; share-scoped background work reads
  `SmbVolumeInner::self_handle()`, ❌ never the volume id (the SUCCESSOR's after a swap).
- **A replaced volume is SUPERSEDED, never unmounted**: `on_superseded` retires the id-scoped parts and leaves `state` /
  `tree` / `client` alone for transfers still holding an `Arc` (tearing it down once killed a live NAS copy). ❌ A
  promotion calls neither hook on the instance it replaces: both act on the SHARED session.
- **`paths_are_os_visible()` tracks the MOUNT, not the backend kind** (latched off by `note_root_mount_gone`). ❌ Never
  hardcode `true`: smb2 browses on past a dead mount, so the drag it breaks fails silently.
- **`write_from_stream` drives an OWNED `FileWriter` on a cloned `Connection`**, ❌ never one borrowed under the client
  mutex (the QNAP deadlock); on error, `abort()` then delete the partial. Progress is `bytes_written()`
  (server-confirmed), ❌ never bytes handed to the pipeline.
- **A hinted read compounds up to `conn.quick_read_limit()`** (`fits_one_compound_read`), sized via
  `read_file_compound_sized` (unsized charges a whole `max_read` in credits). ❌ Don't tune `max_concurrent_ops`'s
  credit clamp (inert).
- **A streamed read ends at its last byte** (the CLOSE is already out): ❌ don't wait for `None` to drop `chunk_tx`.
- **`scan_recursive` asks its `ScanBoundary` per entry, `dir()` BEFORE the listing** (`DETAILS.md` § "Scanning").
- **Bulk work draws on the refcounted pool of extra sessions** (`scan_pool.rs`); a dead member retries on a sibling, ❌
  never moving the MAIN volume's connection state.
- **smb2 bounds every wait itself**: ❌ no timeout layer of ours, never a missed keepalive read as death.
- **Path conversion matches whole COMPONENTS both ways** (`to_smb_path` joins the instance's `share_root`,
  `to_display_path` strips it). ❌ Build a volume through `MountAnchor`, never a bare mount path: an anchored mount that
  loses its anchor addresses the share's top (ERR-48RZX).
- **SMB names are opaque bytes** (NFC and NFD mix, ERR-VETBX): ❌ never normalize a share path, wire or watcher key (a
  `\` in a watcher filename is NAME). A foreign path gets exact only via `find_stored_spelling` (`spelling.rs`), ❌
  never a fold inside an operation: a delete could hit a look-alike. New names compose in the app's write layer
  (`composes_new_names`), ❌ never here.

## Crate must-knows

- **Verify with `cargo check -p cmdr-smb --all-targets`.** ❌ Nothing here may name the app, and the public surface is
  capped (`index-crate-isolation`).
- ❌ **No user-facing prose here**: an error `message` is a log diagnostic; the host renders what humans read.
- ❌ Never gate behavior on `cfg(test)`; use `any(test, feature = "testing")`, or it flips silently when a consumer
  compiles this crate.

Lifecycles, anchored mounts, `rerooted`, credits, Unicode names, the `specta` pin, tests, and decisions: `DETAILS.md`.
Read it first.
