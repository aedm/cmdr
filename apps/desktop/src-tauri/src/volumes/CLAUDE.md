# Volumes

macOS volume and location discovery, plus live mount/unmount watching via `NSWorkspace`. Distinct from
`file_system/volume/` (the `Volume` trait + `VolumeManager`). Linux twin: `volumes_linux/`.

## Module map

`mod.rs` holds the model types and orchestrators and re-exports everything (`crate::volumes::X` stays stable):
`ids.rs` (ID derivation), `fs_type.rs` (non-blocking `statfs`), `nsurl.rs` (blocking NSURL enrichment), `mounts.rs`
(`getfsstat` enumeration), `smb.rs`, `cloud.rs`, `disk_image.rs`, `watcher.rs` (the `NSWorkspace` observer behind
`volume-mounted` / `volume-unmounted`), `disk_units.rs` (which volumes sit on which whole disk), and
`unmount_approver/` (the DiskArbitration approval session).

## Must-knows

- **❌ Never derive or parse a volume ID yourself; call `ids::volume_id_for`** (or `volume_id_for_mount` given only a
  path). An ID keys the index DB, `lastUsedPaths`, tabs, and routing, so a lossy one sends reads and deletes to the
  wrong disk. Only the scheme prefix means anything: ❌ never match the slug or rebuild one from parts.
- **What this module publishes is the SWITCHER's list, ❌ never the registry's**: `mount_roots` hands the whole mount
  table to `file_system::volume::mount_registration`, which registers all of it, because resolution can mint an ID for
  any of it. A row being filtered out of discovery must never mean a path can't be opened.
- **One volume ID publishes ONE location at ONE canonical root**: mounts sharing an ID collapse to the shortest path
  via `cmdr_fs::volume::canonical_root::collapse_by_volume_id` (shared with `volumes_linux/`: ❌ never re-copy it),
  and `list_locations` dedupes on ID, ❌ never on path alone.
- **The unmount path can't use `volume_id_for_mount`**: nothing identifies a gone mount, so it falls back to the wrong
  id. Use `VolumeManager::remove_root(volume_path)` (`handle_volume_unmounted`).
- **Check cloud-drive prefixes BEFORE `statfs` in `resolve_path_volume_fast()`**: a cloud drive is a folder on the data
  volume, so `statfs` answers `/` and mis-highlights "Macintosh HD".
- **A mount earns a switcher row by `MNT_DONTBROWSE` (the OS's own sidebar rule), or by sitting inside `$HOME`**
  (`is_user_facing_mount`), ❌ never by a `/Volumes/` prefix: that hid every drive mounted elsewhere, which is how a
  cloud client's `~/pCloud Drive` became unreachable. The home clause is for clients that mark their mount unbrowsable
  and add their own Finder shortcut; `$HOME` itself is excluded.
- **A cloud provider's own MOUNT is a `CloudDrive` row with `is_cloud_mount: true`** (`is_cloud_provider_mount`, which
  asks `cmdr_fs::…::friendly_error::Provider::is_cloud_storage`), which groups it under CLOUD and keeps it out of the
  index affordances. ❗ Set it in BOTH `get_attached_volumes` and `resolve_path_volume_fast`, or the switcher's
  checkmark and the pane disagree. ❌ Never ask this about a non-mount path: every `~/Library/CloudStorage` folder names
  a provider too, and those are ordinary directories the index reads at local speed.
- **Discovery must never block on a hung mount** (a wedged NAS once froze launch): enumerate with
  `getfsstat(MNT_NOWAIT)`, ❌ never NSFileManager; run blocking NSURL / NSWorkspace / DiskArbitration enrichment for
  LOCAL mounts only; never on the main thread.
- **Launch-time icon, LaunchServices, and TCC-protected `read_dir` calls need the FDA gate**
  (`crate::fda_gate::is_fda_pending_runtime()`), or onboarding stacks 5-10 native TCC popups.
- **Detect SMB with `is_smb_fs_type()`**, ❌ never raw `"smbfs"` / `"cifs"`: one place covers both platforms.
- **An SMB share is ONE path segment; everything below it is a directory INSIDE the share** (`SmbMountInfo::subpath`,
  from `parse_smb_mount_source`), and such a mount is the SAME volume as its share. ❌ Never split a mount source on the
  first `/` (a DFS sub-mount records `//user@domain/SYSVOL/domain`: wrong share, second volume ID, ERR-48RZX). `DETAILS.md` § "A mount can sit inside its share".
- **`mount_is_read_only` and `is_disk_image` are set in BOTH `get_attached_volumes` and `resolve_path_volume_fast`**, or
  they drift. ❌ Read-only is no disk-image proxy: a writable `.dmg` is read-write.
- **Only `enrich_from_volume_registry` copies registry state onto a `LocationInfo`** (`capabilities` +
  `connection_state`); a new field goes there once, in BOTH twins. ❌ Never a discovery constructor.
- **`volume_listing::complete` assembles the published volume list**, owning the order (device providers,
  servers arm, enrichment) and the only `append_device_volumes` call.
- **Wrap every objc-touching `spawn_blocking` body in `objc2::rc::autoreleasepool`**, or the objects leak. Keep
  `watcher.rs`’s observer block cheap: main thread, so no blocking I/O.
- **Every DA-mediated unmount waits while Cmdr lets go of the drive** (`unmount_approver/`): each ask stops every
  indexed volume of the whole BSD unit, or dissents. ❌ On the ask path, no filesystem, no SQLite, no unwind into
  DiskArbitration, no wait but the gate’s. ❌ Never install the `WillUnmount` observer beside it (the fallback; two hooks stop one index twice). A mount that ended with NO ask is a vanished drive (`causes.rs`): stopped as
  `Vanish`, ❌ never resumed, and `watcher.rs`'s stop stands down. `DETAILS.md` § "The unmount approver".
- **❗ A mount table that wouldn't answer is its OWN answer, ❌ never an empty one**: `mount_sources` says `None`, an ask
  `unit_unreadable`, an eject `DiskMounts::Unreadable`. Folding them unmounts a disk past a sibling nobody stopped.

Decisions, edge cases, the servers arm, and the `Retained::cast_unchecked` contract: `DETAILS.md`. Read it before any
non-trivial work here: editing, planning, reorganizing, or advising.
