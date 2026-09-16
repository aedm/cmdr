# Volumes

macOS volume and location discovery, plus live mount/unmount watching via `NSWorkspace`. Distinct from
`file_system/volume/` (the `Volume` trait + `VolumeManager`). Linux twin: `volumes_linux/`.

## Module map

`mod.rs` holds the model types and orchestrators and re-exports everything (`crate::volumes::X` stays stable). Around
it: `ids.rs`, `fs_type.rs` (non-blocking `statfs`), `nsurl.rs` (blocking enrichment), `mounts.rs` (`getfsstat`),
`smb.rs`, `cloud.rs`, `disk_image.rs`, `watcher.rs` (the `NSWorkspace` observer), `disk_units.rs` (volume→whole disk),
`unmount_approver/` (the DiskArbitration session).

## Must-knows

- **❌ Never derive or parse a volume ID yourself; call `ids::volume_id_for`** (or `volume_id_for_mount` given only a
  path): an ID keys the index DB, `lastUsedPaths`, tabs, and routing, so a lossy one sends deletes to the wrong disk.
  Only the scheme prefix means anything: ❌ never match the slug or rebuild one from parts.
- **One volume ID publishes ONE location at ONE canonical root**: mounts sharing an ID collapse to the shortest path via
  `canonical_root::collapse_by_volume_id` (shared with `volumes_linux/`: ❌ never re-copy it), and `list_locations`
  dedupes on ID, ❌ never path alone.
- **What this module publishes is the SWITCHER's list, ❌ never the registry's**: `mount_roots` hands the whole mount
  table to `file_system::volume::mount_registration`. A row filtered out here must never mean a path can't be opened.
- **A mount earns a row by NOT carrying `MNT_DONTBROWSE` (Finder's own rule), or by sitting inside `$HOME`**
  (`is_user_facing_mount`), ❌ never by a `/Volumes/` prefix, which hid every drive mounted elsewhere.
- **A cloud provider's own MOUNT is a `CloudDrive` row with `is_cloud_mount: true`** (`is_cloud_provider_mount`), which
  groups it under CLOUD and drops the index affordances. ❌ Never ask it about a non-mount path: a
  `~/Library/CloudStorage` folder names a provider too, and the index reads those at local speed.
- **Discovery must never block on a hung mount** (a wedged NAS once froze launch): `getfsstat(MNT_NOWAIT)`, ❌ never
  NSFileManager; blocking NSURL / NSWorkspace / DiskArbitration enrichment for LOCAL mounts only, never on the main
  thread. **❗ A mount table that wouldn't answer is its OWN answer, ❌ never an empty one**, or an unmount takes a
  sibling down under its live watcher.
- **Detect SMB with `is_smb_fs_type()`**, ❌ never raw `"smbfs"` / `"cifs"`: one place covers both platforms.
- **An SMB share is ONE path segment; everything below it is a directory INSIDE the share** (`SmbMountInfo::subpath`),
  the SAME volume as its share. ❌ Never split a mount source on the first `/`: a DFS sub-mount records
  `//user@domain/SYSVOL/domain` (wrong share, second ID, ERR-48RZX).
- **Four fields drift unless BOTH `get_attached_volumes` and `resolve_path_volume_fast` set them**:
  `mount_is_read_only`, `is_disk_image`, `is_cloud_mount`, and the category. ❌ Read-only is no disk-image proxy: a
  writable `.dmg` is read-write.
- **Only `enrich_from_volume_registry` copies registry state onto a `LocationInfo`** (`capabilities` +
  `connection_state`), once, in BOTH twins, ❌ never a discovery constructor. `volume_listing::complete` assembles the
  published list, owning its order and the only `append_device_volumes` call.
- **Wrap every objc-touching `spawn_blocking` body in `objc2::rc::autoreleasepool`**, or the objects leak. Keep
  `watcher.rs`’s observer block cheap: main thread, no blocking I/O.
- **Five more rules each own a `DETAILS.md` section; read it before touching that area**: which mounts get a row,
  cloud-drive prefixes ahead of `statfs` in `resolve_path_volume_fast` (else a cloud folder highlights "Macintosh HD"),
  the unmount path's `remove_root` (nothing identifies a gone mount, so deriving an ID lands on the wrong one), the FDA
  gate in front of launch-time icon / LaunchServices / TCC-protected reads (or onboarding stacks 5-10 popups), and the
  DiskArbitration unmount approver (each ask stops the whole BSD unit's indexed volumes or dissents, with ❌ no
  filesystem, no SQLite, and no second pre-unmount hook on the ask path).

Decisions, edge cases, the servers arm, and the `Retained::cast_unchecked` contract: `DETAILS.md`. Read it before any
non-trivial work here.
