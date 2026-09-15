# Volume abstraction

The `Volume` trait's app-side wiring: backends, the `VolumeManager` registry, eject. Every file system operation goes
through a `Volume`, **relative to the volume root**.

## Module map

- `mod.rs` re-exports all of `cmdr_fs::volume`; the trait itself is `crates/cmdr-fs/src/volume/mod.rs`.
- `manager.rs` (+ `manager/`: `routing.rs` and its two routes, the mount-root set): the registry behind
  `get_volume_manager()`.
- `backends/` (its own `CLAUDE.md`), `eject/` (macOS+Linux teardown by kind), `drive_release/` (the index stop/start
  gate), `friendly_error/` (in `cmdr-fs`).

## Must-knows
- **A site passing a path calls `VolumeManager::resolve(volume_id, path).await`, ❌ never `get(volume_id)`.** It routes
  `.zip`-crossing and `.git/<category>/` paths to their backends, path UNCHANGED, and answers `is_routed()`; match a `RoutedKind` only where the answer is about that one backend.
  `resolve_local_only` is for the ONE caller that can't `.await`. `DETAILS.md` § "Resolving a path: the two routes".
- **Watcher-pre-registered volumes use `register_if_absent`** (else FSEvents overwrites an `SmbVolume`). `register`
  replaces only at the SAME root; an edited root uses `replace_root_in_place`. `DETAILS.md` § "Key decisions".
- **A volume the registry REMOVES is retired** (`Volume::retirement`), so a backend's watcher and reconnect loop stand
  down; a replace doesn't. `DETAILS.md` § "Leaving the registry".
- **Work that must WAIT for a volume subscribes with `on_volume_arrival`, ❌ never polls the registry.** A listener gets
  the ID only and runs INSIDE the registration: return at once, hand work to a task. `DETAILS.md`
  § "Telling someone a volume arrived".
- **A registry entry owns a SET of mount roots, one active.** An unmount drops one via `VolumeManager::remove_root`,
  which promotes a survivor and unregisters ONLY on the last; ❌ never `unregister` because one mount went away, or a
  share mounted twice disappears on the first eject. `find_by_root` matches ANY known root, so compare `volume.root()`.
  `DETAILS.md` § "A volume ID owns a set of mount roots".
- **❌ Never probe a mount root for liveness.** Promotion runs on evidence that arrives on its own: an unmount event,
  or `volume::note_root_failure` seeing a mount-is-gone errno. A probe on a wedged mount blocks 30–120 s and once froze
  the app at launch (`volumes/DETAILS.md` § "Hung mounts").
- **Cross-volume copy flows only through `open_read_stream` / `write_from_stream`, chunk by chunk.** ❌ Never drain a
  `VolumeReadStream` or collect a remote file into a `Vec<u8>`.
- **Rows a PANE sees that no volume holds come from a `ListingOverlay`** (`src/listing_overlays.rs`). ❌ Never in a
  `Volume` impl or the manager: scans, delete walkers, and the indexer list through `Volume`, and a row with no inode
  once left a half-deleted repo. `DETAILS.md` § "Architecture".
- **Every mutation must call `notify_mutation`, `write_from_stream` included.** Its default is a no-op and SMB/MTP
  watcher events are lossy, so skipping it leaves a stale pane.
- **Capability flags default to the conservative answer** (`Err(NotSupported)` / `false`), so a backend opts in.
  Several break silently when answered wrong (`is_writable` is button state): read `DETAILS.md` § "Trait capability
  model" first. `capabilities()` is a pure fold; ❌ never override it.
- **A path from the UI is anchored by its CALLER (`cmdr_fs::volume::root_anchored`), ❌ never guessed at by the
  backend.** Panes send absolute paths, the transfer dialog volume-relative ones, and a leading `/` can't tell them
  apart. Idempotent, so anchor without checking. `DETAILS.md` § "Path handling gotchas".
- **Every app-side index stop of a removable drive and every non-root index start goes through `drive_release`**, ❌
  never a bare `Index::start_volume` / `rescan_volume` / `cover` / `stop_removable_volume`. `eject/` stops through it
  BEFORE `diskutil` runs (FSKit wedge: kernel-panic risk), and **every teardown goes through `run_teardown`**, the ONE
  place a refusal is logged. `DETAILS.md` § "Eject", § "One release, one start".

Architecture, flows, and decision detail: `DETAILS.md`. Read it before any non-trivial work here: editing, planning,
reorganizing, or advising.
