# Eject through DiskArbitration, naming what holds the drive

**Problem.** When an eject is refused, Cmdr says "Something is still using this drive", and only the log knows which
process, because the name lives in `diskutil`'s stderr, which we may never parse. Finder names the app. Three
data-safety gaps sit under that:

- `diskutil eject` of one partition that unmounts while a sibling refuses counts as done (`Settled::AlreadyGone`), so
  the disk stays powered and the person is told it ejected.
- Only THIS volume's index stops before the teardown, and only this volume's writes are checked, while a whole-disk
  eject unmounts every sibling too. An indexed exFAT sibling is exactly the FSKit wedge that kernel-panicked a Mac on
  2026-07-15.
- After a refusal, nothing restarts the indexes the eject stopped, so a drive that stays mounted silently stops being
  indexed.

**Outcome.**

- Every sibling is checked for writes, and its index stops first; a refused eject resumes what it stopped.
- Disk volumes (USB, SD, DMG) eject through DiskArbitration (DA) with typed statuses and a whole-disk answer.
- Every refusal carries typed holders, and the toast names them: one app, several apps, a disk image still open, Cmdr
  itself, or "macOS is still working with this drive".
- SMB keeps `diskutil unmount`; Linux keeps `umount`.

**Status.** Planned 2026-09-12, not started, review rounds 1 and 2 folded in.

- **Landed prerequisites**: the refusal retry (`unmount_tool::settle_with_retries`, `REFUSAL_RETRY_BACKOFF` 0.5, 1, and
  1.5 s), the `NotEjectable` preflight (`is_already_unmounted`), and the eject deadlines (`2aa20b547`).
- **Still pending**: the index-stop wait fix (B1 below). M2 can't start until it lands.

## Loud rules

- ❌ **Never classify by `diskutil` stderr, a DA status string, or a process name.** Every decision is a typed status,
  an errno, a PID, an activation policy, a code-signing attribute, a BSD unit number, or a mount-table entry
  (`error-string-match`, `cmdr/no-error-string-match`).
- ❌ **Never eject until a fresh mount table shows every volume of the physical disk gone.**
  `DADiskUnmount(physical, Whole)` does NOT reach APFS volumes in its synthesized container. Whole-unmount every
  synthesized container first, then the physical whole disk, then re-read the table.
- ❌ **An eject-stage dissent is never plain success.** It's a refusal, unless the table confirms every volume
  unmounted, which makes it the typed `UnmountedNotPoweredDown`.
- ❌ **Never eject the synthesized APFS container.** `DADiskEject` on it answers success and does nothing.
- ❌ **Check every registered sibling for writes, and stop every sibling's index, before any teardown**, not only this
  volume's. A whole-disk unmount takes the siblings down too.
- ❌ **Nothing unmounts after a pre-unmount step fails or stalls.** Resolution and the index stop each answer
  `NotResponding` and stop there, per the deadlines commit's truth test.
- ❌ **Holder facts never run a code-signing query against a process whose executable lives on the target volume**, run
  only after the PIDs are captured, and release every reference before returning. A Security query against an executable
  on the volume makes Cmdr the holder.
- ❌ **Every DA and resolve call runs under a deadline.** `DADiskCreateFromVolumePath` runs `statfs` then `realpath`,
  which hangs on a DMG whose backing file lives on a hung SMB share.
- ❌ **Never run DA, or scan holders, against a network volume or a path that already left the mount table.**
  `PATH_IS_VOLUME` on a plain directory scans the BOOT volume (737 holders in the review probe).
- ❌ **Never link the private `DADissenterGetProcessID` or `responsibility_get_pid_responsible_for_pid`.** Look each up
  once with `dlsym`; a missing symbol means fewer names, never a link failure.
- ❌ **DA bridge**:
  - One `DASession` per eject, scheduled on ONE serial dispatch queue of its own, for resolve, unmount, and eject;
    callbacks arrive on the disk's own session.
  - Never touch a callback's box after the call returns.
  - The callback never panics.
- ❌ **Never resume an index after `TimedOut`.** That unmount may still land, and a fresh watcher on a volume
  mid-unmount is the FSKit wedge. Resume only after a definite refusal, and only indexes that were running before the
  stop.
- ❌ **Real-disk tests and spikes use synthetic APFS images only**: never FAT or exFAT (FSKit `msdos`), never a physical
  card.
  - Take BSD names from `hdiutil attach -plist`.
  - Detach only when `hdiutil info -plist` still maps that node to OUR image: DA reuses unit numbers at once, so a
    stored `/dev/diskN` can become someone's Time Machine drive.
  - Detach nested images inner first, and never `-force` the outer.
- **Keep the retry working**: the disk path retries a typed busy refusal. Scan holders ONCE, after the last attempt.
- **Every user-facing word comes from `errors.eject.*`**, in all 13 catalogs. The English copy below is a draft for
  David.
- **Docs ship with each milestone** (`AGENTS.md` "Keep in sync"); M6 is only the close-out sweep.
  `apps/desktop/src-tauri/src/file_system/volume/CLAUDE.md` is at 599 of 600 words, so a milestone that touches it (M2,
  M5) rewrites its eject bullet no longer and moves depth into `DETAILS.md` § "Eject".
- **Checks run through `pnpm check`, foreground only**, with ONE named exception: the `#[ignore]`d real-image tests are
  a hand-run `cargo nextest` (no lane runs them), and each milestone that touches them pastes that run's result into its
  commit body.

## Evidence the design rests on

Unless marked otherwise: verified 2026-09-12 on macOS 26.6.2 (25G83), unsandboxed uid 501, with throwaway probes against
50–60 MB APFS DMGs attached `-nobrowse` (rounds 1 and 2 of the systems and safety reviews). Apple sources:
DiskArbitration-535.0.10, xnu-12377.1.9.

### DiskArbitration statuses and flow

- **Busy is `unix_err(EBUSY)`, not `kDAReturnBusy`.** A held volume answered `0x0000C010` (system 0, sub 3, code 16).
  The daemon rewrites any `unmount(2)` failure to EBUSY (`diskarbitrationd/DARequest.c:1525`). `kDAReturnBusy`
  (`0xF8DA0002`) covers `/` and the Data volume (`DARequest.c:1355-1366`) or an approval dissenter's own choice.
- **A failure turns into success when the volume already left the mount table** (`DARequest.c:1507-1512`), so a repeat
  unmount of an unmounted volume answers success, and a retry is idempotent.
- **Whole links subrequests by BSD unit** (`DAQueue.c:974`), so container `disk6` isn't part of `disk5`'s Whole request.
  Probe: `DADiskUnmount(disk5, Whole)` answered success in 0 ms with `disk6s1` still mounted, then `DADiskEject(disk5)`
  answered `0x0000C010`, PID 0, after 4.6 s, still mounted.
- **A container Whole unmount reaches every volume in the container, and partial unmounts are real.** Two-volume APFS
  image, file held on volume R2B: R2A unmounted, R2B stayed mounted, EBUSY; the retry answered the same PID.
- **DA refusals are slow.** A refused container Whole unmount took 12.2 s, its retry 3.9 s; four attempts on today's
  backoff come to about 27 s. A first attempt under load can pass the 15 s `TOOL_TIMEOUT`.
- **The dissenter PID propagates through a Whole link** from the first sub-dissenter (`DARequest.c:134-155`), so a Whole
  request never answers `kDAReturnNotMounted`.
- **`kDAReturnNotMounted` (`0xF8DA0007`) is typed** for a non-Whole request on a disk DA knows is unmounted (0 ms,
  `DARequest.c:1348`). `DADiskCreateFromVolumePath` on a path that's no longer a mount returns NULL.
- **`DADiskCreateFromVolumePath` runs `statfs` then `realpath`** (`DiskArbitration/DADisk.c:413-421`).
- **The dissenter PID is private but exported.**
  - Declared only in `DiskArbitrationPrivate.h:327`, and listed in the SDK `DiskArbitration.tbd`.
  - The daemon fills it with `proc_listpidspath` run as ROOT, keeping ONE PID (`DARequest.c:1707-1738`).
  - Approval dissenters carry the dissenting app's `getpid()` (`DADissenter.c:39`).
- **Bridge facts**:
  - Callbacks arrive on the disk's own session (`DiskArbitration.c:257,1273`).
  - A queueing failure runs the callback synchronously on the calling thread (`DiskArbitration.c:1283-1300`).
  - `DASessionSetDispatchQueue` retains the session until cancelled (`DASession.c:720,737`).
  - A daemon restart drops pending requests with no callback (`DASession.c:744-754`).
  - `_DASessionCallback` makes a synchronous MIG call on its queue (`DASession.c:242`).
- **DA doesn't track SMB**: `diskutil info /Volumes/naspi` prints "Could not find disk".
- **Ejecting the physical store detaches an image**: once everything was unmounted, `DADiskUnmount(disk5, Whole)` then
  `DADiskEject(disk5)` detached it in 16 ms.
- **IOKit ancestry of an APFS volume** (`ioreg -p IOService -t`): `AppleAPFSVolume` → `AppleAPFSContainer` (the
  synthesized whole disk) → `AppleAPFSContainerScheme` → `IOMedia` (the physical store partition) → the physical whole.

### The index stop (B1, confirmed in code by the lead)

`stop_the_volume` (`crates/cmdr-index/src/indexing/lifecycle/state/teardown.rs:104`) returns WITHOUT waiting in three
cases, so today's eject can unmount under a live FSEvents stream or open SQLite handles:

- a scan start holds the manager (`StopTarget::Claimed`, drained at handback);
- the phase is `Initializing` (cancel plus remove; the half-built manager shuts down later);
- a drain is already running (the `other =>` arm puts the state back).

The lead is fixing it on this branch: `Index::stop_removable_volume` will wait for the drain under the existing 15 s
`INDEX_STOP_DEADLINE`. This plan depends on that fix and doesn't redesign it.

### Holders

- **`proc_listpidspath` is public in the SDK (`libproc.h:85`) but sees same-uid processes only.**
  - Scanning `/` found 1,323 holders, all uid 501 (`CHECK_SAME_USER`, `xnu/bsd/kern/proc_info.c:2196-2211`).
  - It catches open files, cwd, and per-thread cwd. It misses memory-mapped-only holders; DA gives PID 0 for those too,
    and `lsof` finds them.
  - It `stat`s its path first (`proc_listpidspath.c:83`).
  - Timing with `PATH_IS_VOLUME`: 142 ms to 9.4 s under load; a healthy SMB share took 67 ms.
- **A Security query can make the prober the holder.** With a binary running FROM the volume:
  - `SecCodeCopyGuestWithAttributes` put the probe itself in `proc_listpidspath`.
  - `SecCodeCopySigningInformation` took 4.5 s.
  - A Whole unmount in that window got EBUSY with DA's dissenter PID = the probe's own PID.
- **Typed classification signals** (`NSRunningApplication` + `SecCodeCopyGuestWithAttributes` +
  `SecCodeCopySigningInformation`, ppid from `PROC_PIDT_SHORTBSDINFO`):
  - `/usr/libexec/lsd` (uid 0 and uid 501): parent launchd, Apple platform binary (platform identifier 26), no
    `NSRunningApplication`.
  - `mds_stores` (uid 308): same shape. The Security calls work across uids.
  - Finder: platform binary AND a regular activation policy.
  - `/bin/zsh` in a terminal: platform binary, no app; the parent chain reaches Warp (regular).
  - `com.apple.WebKit.WebContent` serving Google Drive: platform binary, parent launchd, an accessory-policy
    `NSRunningApplication` named "Google Drive Web Content".
  - `/bin/sleep` and Calculator are platform binaries too, so "platform binary" alone never means "macOS system".
- **ERR-TT2FH** (the #4 probe): the dissenter was `/usr/libexec/lsd`, registering an unseen `.app` after a pane fetched
  its icon, for about 0.3–0.9 s.
- **System processes that "wait a minute" doesn't fit**: `diskimagesiod` holds a volume while an image stored on it
  stays attached; `backupd` holds it for a whole Time Machine backup.

## Current code (re-read 2026-09-12, after `2aa20b547`)

- `apps/desktop/src-tauri/src/file_system/volume/eject/mod.rs`:
  - `EjectAction::{DiskutilEject, DiskutilUnmount, DeviceDisconnect}` from the pure `decide_eject_action`.
  - `EjectError` IS the wire type (`specta::Type`, tagged camelCase). Its variants:
    - `UnmountRefused { detail }`;
    - `TimedOut`: something started and may still land;
    - `NotResponding { step: EjectStep }`, with `EjectStep::{EjectabilityCheck, IndexStop}`: a pre-unmount step stalled,
      so nothing was unmounted.
  - `eject` → `in_flight::join_or_start(volume_id)` → `eject_now`:
    1. the busy gate (`busy_volume_ids`, this volume only);
    2. the device provider;
    3. the `is_already_unmounted` preflight;
    4. `resolve_is_ejectable(mount_path)` under `deadlines::within_deadline(EjectabilityCheck, 5 s)`;
    5. `stop_index_then_unmount(volume_id, stop_index_blocking(volume_id), || run_teardown(..))`.
  - `stop_index_then_unmount(volume_id, stop_index: impl Future, unmount)` runs the stop under `INDEX_STOP_DEADLINE` (15
    s) and never runs `unmount` when it stalls.
  - `run_teardown(volume_id, Teardown::{Device(provider), Tool { verb, mount_path }})`:
    - the Device arm runs under `DEVICE_EJECT_DEADLINE` (15 s) and answers `TimedOut`;
    - the Tool arm calls `unmount_tool::settle_with_retries(Target, run, is_still_mounted)`.
  - `disconnect_smb` runs `Teardown::Tool { Unmount }` on macOS.
- `eject/deadlines.rs`: `EJECTABILITY_CHECK_DEADLINE`, `INDEX_STOP_DEADLINE`, `DEVICE_EJECT_DEADLINE`, and
  `within_deadline(step, volume_id, deadline, work)` over `crate::deadline::timeout_detached_typed` (expiry detaches the
  work and answers `NotResponding { step }`).
- `eject/unmount_tool.rs`:
  - `TOOL_TIMEOUT` (15 s), `UnmountVerb`, `ToolOutcome`, `run`, and the pure `settle`.
  - `is_still_mounted` (`volumes::is_mount_point`, `getfsstat(MNT_NOWAIT)`).
  - `REFUSAL_RETRY_BACKOFF`, `Target`, and `settle_with_retries`, typed on `ToolOutcome`, with retry tests.
- `eject/in_flight.rs`: one flight per VOLUME ID; its tests construct `UnmountRefused { detail }`.
- Index:
  - `Index::stop_removable_volume(id) -> bool` (`crates/cmdr-index/src/indexing/handle/mod.rs:351`).
  - `Index::start_volume(id)` (`handle/mod.rs:207`) routes a `LocalExternal` drive to
    `start_indexing_for_local_external`, and records the per-drive "the user turned this on" marker.
  - `index_host::index()` needs `install(&AppHandle)` (`apps/desktop/src-tauri/src/index_host.rs:28`), so tests can't
    run real indexes.
- Registry: `VolumeManager::list_volumes_with_handles() -> Vec<(String, Arc<dyn Volume>)>` (`manager.rs:332`) and
  `find_by_root(&Path)` (`manager.rs:284`; it matches ANY known root, so compare `volume.root()`).
- `apps/desktop/src-tauri/src/volumes/disk_image.rs`: the only DA code, raw `extern "C"` over `core_foundation`, with an
  `#[ignore]` real-DMG test that runs `hdiutil` unguarded.
- `apps/desktop/src-tauri/src/mcp/executor/eject.rs`:
  `Err(ToolError::internal(format!("Couldn't eject {volume_id}: {e}")))`. `ToolError` has a typed `data` member
  (`mcp/executor/mod.rs:62`).
- Frontend:
  - `apps/desktop/src/lib/file-explorer/navigation/eject-error-messages.ts`: `EJECT_MESSAGE` is a record over
    `EjectError['type']`, including `notResponding`, plus `wordEjectRefusal` and `ejectTechnicalDetail`.
  - The words sit inside `fileExplorer.pane.ejectFailedToast` ("Couldn''t eject {volumeName}: {message}") or
    `disconnectFailedToast`.
  - Call sites: `VolumeBreadcrumb.svelte:639`, `pane/volume-context-action.ts:55`, `pane/smb-view-state.svelte.ts:225`.
  - Error toasts carry "Send error report…" (`lib/ui/toast/toast-store.svelte.ts:91`).
- Catalog: `errors.eject.*` (now including `notResponding`) is a RAW family: `getMessage`, single apostrophes, a literal
  `{token}` replaced with `replaceAll`, so no plurals (`docs/guides/i18n.md:501`). 13 catalogs: `en`, ten full
  translations, and the `en-GB` / `en-AU` overlays.
- Tests:
  - `crates/cmdr-index/src/indexing/tests/external_drive_fixture.rs`: the guarded `hdiutil` runner, FAT/exFAT,
    `#[ignore]`, the `disk-image` nextest group (`.config/nextest.toml:65,76-85`), hand-run per
    `crates/cmdr-index/src/indexing/tests/DETAILS.md:94`.
  - The global per-test cap is 8 s (`.config/nextest.toml:447`).
  - `cmdr-fs` has a `testing` feature that widens test helpers to `pub`.
- Dependencies:
  - `libc` 0.2.189 lacks `proc_listpidspath`, its flags, and `PROC_ALL_PIDS`.
  - `objc2-disk-arbitration` is NOT in `Cargo.lock`.
  - `dispatch2` 0.3.1 and `objc2-io-kit` 0.3.2 are in it transitively.
  - `security-framework` 3.7 is direct, with `SecCode::copy_guest_with_attribues` but no signing-information binding.
  - No list formatter exists in `$lib/intl`. The old-WebKit boot guard requires Safari 15.4, which has
    `Intl.ListFormat`.

## Target design

### Order of an eject

1. **Per-volume preflight, unchanged**: in-flight join by volume ID, device provider, `is_already_unmounted`, and the
   ejectability check.
2. **Resolve the disk (M2)**. Under `DISK_RESOLVE_DEADLINE`, answering `NotResponding { step: DiskResolve }` on a stall
   OR a failure (S1): nothing has started, and the existing `notResponding` copy stays true.
3. **Join or start the DISK flight (M2)**, keyed by the physical whole disk's IOKit registry entry ID. See § "Flights
   per physical disk".
4. **Sibling busy gate (M2)**: `Busy` if a write op touches ANY registered volume on the disk.
5. **Stop every sibling's index (M2)**, all inside the one `INDEX_STOP_DEADLINE`, recording which were indexing.
6. **Teardown**: `diskutil eject` until M5, DA after, under its own budget.
7. **Holders (M3a, M3b)**, after a final `UnmountRefused` only.
8. **Resume (M2)** the indexes that were running, on volumes still mounted, after a final `UnmountRefused` only.

### Deadlines and budgets (S2; pending David as open question 8)

Re-derived from the DA timings above:

- **Ejectability check**: 5 s, unchanged.
- **`DISK_RESOLVE_DEADLINE`**: 5 s, the same tier. Resolution is local DA and IOKit calls, except the `statfs` +
  `realpath` inside `DADiskCreateFromVolumePath`.
- **`INDEX_STOP_DEADLINE`**: 15 s, unchanged, now covering ALL sibling stops, which run concurrently on the blocking
  pool.
- **`DA_REQUEST_DEADLINE`**: 30 s per DA request, about 2.5 times the worst measured refusal (12.2 s under load). With
  15 s, a loaded first attempt answers `TimedOut` and names nobody.
- **`DISK_REFUSAL_RETRY_BACKOFF`**: `[1 s]`, one retry. A DA attempt already lasts at least 3.9 s (the daemon's own
  unmount, busy-vnode retry, and root holder scan), which outlasts the 0.3–0.9 s `lsd` hold that motivated retrying.
  More attempts cost 4–12 s each for holds that aren't transient. The `diskutil` path keeps `REFUSAL_RETRY_BACKOFF`
  until M5 removes it.
- **`DISK_TEARDOWN_BUDGET`**: 45 s across attempts; no new attempt starts past 30 s elapsed.
- **`HOLDER_BUDGET`**: 1.5 s for scan plus facts, injected as a parameter so tests never race a real scan.

What the person waits:

- **A clean eject**: under a second.
- **A typical refusal** (measured): about 12.2 + 1 + 3.9 + 1.5 ≈ 19 s after the index stop.
- **Worst case, every step stalling at its limit** (only a broken drive): 5 + 5 + 15 + 45 + 1.5 ≈ 72 s.
- **Today's `diskutil` worst case, for comparison**: 5 + 15 + (four 15 s attempts + 3 s of backoff) ≈ 83 s.

The ejecting spinner shows throughout either way.

### `TimedOut` or `NotResponding`

The deadlines commit's truth test: `NotResponding` when nothing was unmounted and nothing may still land; `TimedOut`
when a teardown request started and may still land.

- Resolve stall or failure → `NotResponding { step: EjectStep::DiskResolve }` (a new step; the existing copy).
- Any sibling's index stop past `INDEX_STOP_DEADLINE` → `NotResponding { step: IndexStop }`, and no resume (it may still
  be draining).
- A DA unmount request past `DA_REQUEST_DEADLINE` → `AlreadyGone` if the fresh table shows the disk's volumes gone, else
  `TimedOut`.
- A DA eject request past its deadline → `UnmountedNotPoweredDown` if the table shows everything gone, else `TimedOut`.
- The holder phase past `HOLDER_BUDGET` → never an error; unfinished PIDs become `Unclassified`.

### Flights per physical disk (decided)

Keep `in_flight` keyed by volume ID for the preflight. After resolution, join or start a DISK flight in the same module,
keyed by `DiskKey(u64)`, the physical whole disk's IOKit registry entry ID. That ID isn't reused the way BSD unit
numbers are.

- **Why join**: ejecting sibling B while A's whole-disk teardown runs has the same goal. A second teardown would double
  the index stops, race DA requests on the same disk, scan holders twice, and could hand B a success for A's work, or A
  a refusal caused by B's stop. Joining gives both callers one honest answer.
- **The ejecting set** marks every registered sibling's volume ID for the flight's life, so every control shows
  progress.
- **Resolution runs before the join**, so two callers can each spend up to `DISK_RESOLVE_DEADLINE` resolving. Resolution
  is read-only, so that's accepted.

### Module shape

- `eject/mod.rs`:
  - `EjectAction::DiskutilEject` becomes `EjectAction::DiskEject` in M5 (DA on macOS, `umount` on Linux);
    `DiskutilUnmount` stays for SMB.
  - M2 adds `disk: Option<&'a DiskTarget>` to `Teardown::Tool` (`None` for SMB and Linux), so `run_teardown` can hand
    the resolved disk to the scan.
  - M5 replaces the eject use of `Tool` with `Teardown::Disk { target: &'a DiskTarget }`.
  - `EjectStep` gains `DiskResolve`.
- `eject/disk_target.rs` (macOS, M2): resolution to
  `DiskTarget { key: DiskKey, physical_whole_unit: u32, container_units: Vec<u32>, session: RetainedSession, queue: DispatchQueue }`,
  plus `mounted_volumes(&target) -> Vec<MountedVolume { bsd_unit, path }>`, rebuilt from a FRESH mount table on every
  call (S4).
- `eject/disk_flight.rs` (M2): the per-disk join, sibling selection, the sibling busy gate, the stop and resume.
- `eject/disk_arbitration/` (macOS, M5): `mod.rs` (the bridge, `eject_disk(&target) -> DiskOutcome`), `status.rs`
  (`DaStatus`, `settle_disk`), `private_symbols.rs` (the two `dlsym` lookups).
- `eject/holders/`: `mod.rs` (`VolumeHolder`, `HolderKind`, `merge`, `classify`), `scan.rs` (`proc_listpidspath`; Linux
  answers empty), `facts.rs`, `nested_images.rs`.

### Resolution (`disk_target.rs`, M2)

All on the blocking pool under `DISK_RESOLVE_DEADLINE`, on ONE `DASession` that the DA teardown reuses in M5:

1. `DADiskCreateFromVolumePath(mount_path)`. NULL → a resolve failure (`NotResponding { DiskResolve }`). The hung-share
   DMG blocks here; the deadline bounds the wait, the blocking thread stays stuck until its syscall returns (accepted),
   and `in_flight` stops a repeat click stacking another.
2. `DADiskCopyWholeDisk`. If its IOMedia (`DADiskCopyIOMedia`) conforms to `AppleAPFSContainer` (`IOObjectConformsTo`, a
   class identifier), walk `kIOServicePlane` parents to the first plain `IOMedia` (the physical store partition),
   `DADiskCreateFromIOMedia` it, and take its whole disk. Otherwise the whole disk is physical.
3. `key`: `IORegistryEntryGetRegistryEntryID` of the physical whole.
4. `container_units`: walk the physical whole's descendants for `AppleAPFSContainer`, reading each one's
   `kDADiskDescriptionMediaBSDUnitKey` (a typed CFNumber).
5. `mounted_volumes` (per call): read the non-blocking mount table, map each local `/dev/diskNsM` through
   `DADiskCreateFromBSDName` (touches no filesystem) and its `MediaBSDUnit`, and keep units equal to
   `physical_whole_unit` or in `container_units`.

### Siblings (`disk_flight.rs`, M2)

- **Selection**: from `VolumeManager::list_volumes_with_handles()`, the volumes whose `volume.root()` is one of
  `mounted_volumes`' paths. Don't use `find_by_root` alone: it matches any known root.
- **Busy gate**: `Busy` if any selected ID is in `busy_volume_ids()`.
- **Stop**: capture whether each sibling's index is running (the handle's status query M2 picks), then
  `stop_removable_volume` for all of them concurrently. The whole set is the `stop_index` future of
  `stop_index_then_unmount`, so it sits inside the one `INDEX_STOP_DEADLINE`.
- **Resume (S3)**: after the teardown's final `UnmountRefused`, re-read `mounted_volumes`, and call
  `Index::start_volume` for each sibling still mounted whose index was running before the stop. `start_volume` records
  the per-drive enable marker, so it must never run for a drive that wasn't indexing.
  - Never after `TimedOut`, `NotResponding`, or success.
  - A resume failure logs `warn` and changes nothing about the eject's answer.
- **Dependency**: B1's fix, so a returned stop means a drained index.

### The DA teardown (M5)

1. **Session and queue**: the resolve session, already scheduled on its own serial queue, so every `DADiskRef` belongs
   to it and all callbacks arrive there.
2. **Unmount each synthesized container**: `DADiskUnmount(container, Whole)`. A dissent stops the attempt.
3. **Unmount the physical whole**: `DADiskUnmount(physical_whole, Whole)`. A dissent stops the attempt.
4. **Gate**: rebuild `mounted_volumes` from a fresh table. Any entry left → `DiskOutcome::StillMounted` (never eject).
5. **Eject**: `DADiskEject(physical_whole)`.
6. **Bridge**:
   - Each request's context is a `Box` holding a `oneshot::Sender`, reclaimed with `Box::from_raw` exactly once by the
     callback.
   - The call site never touches it after the call returns, because a queueing failure runs the callback synchronously
     first.
   - The callback body is infallible, with `catch_unwind` as a backstop.
   - Each request is awaited under `DA_REQUEST_DEADLINE`.
   - The session is unscheduled (`DASessionSetDispatchQueue(session, NULL)`) once the flight ends.
   - Accepted and documented: a daemon restart drops a pending request with no callback, leaking one box per lost
     request, and that attempt answers `TimedOut`.
7. **Retry**: `settle_with_retries` becomes generic over the outcome type and its settle function, still retrying only
   `Refused(UnmountRefused)`, on `DISK_REFUSAL_RETRY_BACKOFF` within `DISK_TEARDOWN_BUDGET`. Each attempt re-runs steps
   2–5.

### Statuses (pure, `status.rs`, M5)

- `DaStatus::from_raw(u32)`:
  - `Busy`: `err_get_system == 0 && err_get_sub == 3 && err_get_code == EBUSY` (`0x0000C010`), or `0xF8DA0002`.
  - `NotMounted`: `0xF8DA0007`.
  - `NotFound`: `0xF8DA0006`.
  - `NotPermitted`: `0xF8DA0008`.
  - `NotPrivileged`: `0xF8DA0009`.
  - `Other(raw)`: anything else.
- `DiskOutcome`:
  - `Succeeded`.
  - `Dissented { stage: ContainerUnmount | WholeUnmount | Eject, status, raw, dissenter_pid: Option<u32> }`.
  - `StillMounted`, `SessionUnavailable`, `TimedOut { stage }`, `TaskFailed { detail }`.
- `settle_disk(outcome, all_gone) -> Settled`, where `all_gone` rebuilds `mounted_volumes` and asks if it's empty:
  - `Succeeded` → `Done`.
  - `Dissented { ContainerUnmount | WholeUnmount, Busy }` → `Refused(UnmountRefused)`, even when THIS volume's root left
    the table. That closes the sibling gap.
  - `Dissented { ContainerUnmount | WholeUnmount, NotMounted | NotFound | Other }` → `AlreadyGone` if `all_gone`, else
    `Refused(Unexpected)`.
  - `StillMounted` → `Refused(UnmountRefused)`.
  - `Dissented { Eject, _ }` → `UnmountedNotPoweredDown` if `all_gone`, else `Refused(UnmountRefused)` for `Busy`,
    `Refused(Unexpected)` otherwise.
  - `TimedOut { ContainerUnmount | WholeUnmount }` → `AlreadyGone` if `all_gone`, else `Refused(TimedOut)`.
  - `TimedOut { Eject }` → `UnmountedNotPoweredDown` if `all_gone`, else `Refused(TimedOut)`.
  - `SessionUnavailable`, `TaskFailed` → `AlreadyGone` if `all_gone`, else `Refused(Unexpected)`.
- `Settled` gains `UnmountedNotPoweredDown`, whose wire mapping is open question 4. Until David answers, it maps to
  `Ok(())` with a `warn`, today's `diskutil` parity.
- `detail` for the log: `"DADiskUnmount disk6 (container): status 0x0000C010 (busy), dissenter pid 983"`.

### Holders (M3a scan and wire; M3b facts)

- **When**: once, in `run_teardown`, after the final `UnmountRefused` of a DISK teardown (`Tool` with `disk: Some`, then
  `Disk`). Never for SMB, whose holders stay empty (open question 3).
- **Capture first**: the DA dissenter PID from the last attempt (M5), then
  `proc_listpidspath(PROC_ALL_PIDS, 0, path, PATH_IS_VOLUME | EXCLUDE_EVTONLY)` over each path in a FRESHLY rebuilt
  `mounted_volumes`. Scan still-mounted paths only. Fact gathering starts after capture, and no unmount attempt follows
  it.
- **FFI (M3a)**: a two-line `extern "C"` for `proc_listpidspath`, beside `PROC_ALL_PIDS = 1`,
  `PROC_LISTPIDSPATH_PATH_IS_VOLUME = 1`, and `PROC_LISTPIDSPATH_EXCLUDE_EVTONLY = 2` (`libproc.h:52,61`,
  `sys/proc_info.h:51`). ❌ No `libproc` crate: it build-depends on `bindgen` with libclang.
- **`HOLDER_BUDGET`** is a parameter of the holder function; production passes 1.5 s, and tests inject their own.
- **Facts (M3b)**, cheapest first, stopping once the kind is decided. Walk ancestors up to eight levels, stopping at PID
  1, inside `objc2::rc::autoreleasepool`:
  1. `pid == own_pid`, or an ancestor is → `Cmdr`.
  2. `NSRunningApplication` for the process, then each ancestor. The first regular activation policy →
     `App { name, bundle_id }`.
  3. With OQ2 approved: `responsibility_get_pid_responsible_for_pid` (through `dlsym`). A responsible process with a
     regular policy → `App`.
  4. The executable's device (`lstat` of `proc_pidpath`) matches a `mounted_volumes` device → `Tool { name }`. ❌ No
     Security call.
  5. Otherwise the Apple platform-binary check, `SecCodeCopyGuestWithAttributes` + `SecCodeCopySigningInformation` →
     `kSecCodeInfoPlatformIdentifier`, references released at scope end. Platform binary → `System`; else `Tool`. ❌ Not
     `csops`: its header isn't in the SDK.
- **Nested images (M3b)**: attached disk images whose backing file lives on a `mounted_volumes` device (the signal is
  chosen in M0a) add one `DiskImage { name }` holder.
- **`merge`**: dedupes by PID in first-seen order and drops ESRCH.
- **Known tradeoffs** (recorded in `volume/DETAILS.md`):
  - `backupd` classifies as `System`; "wait a minute" understates a long Time Machine backup.
  - A helper with no responsible-PID answer and a launchd parent reads as `System`.
  - A path heuristic (`/System/`, `/usr/libexec/`) was rejected: `/bin/zsh` would read as macOS.
  - An orphaned platform CLI (`nohup sleep`) reads as `System`.

### Wire type (M3a)

```rust
UnmountRefused {
    /// Who held the drive when the last attempt was refused, deduped by PID. Empty when nothing could be named.
    holders: Vec<VolumeHolder>,
    /// The raw statuses and dissenter, for the log and the details line. ❌ Never the message.
    detail: String,
}

pub struct VolumeHolder {
    pub pid: u32,
    /// App display name, image volume name, or executable name (the last only for the log and MCP).
    pub name: String,
    pub bundle_id: Option<String>,
    pub kind: HolderKind,
}

pub enum HolderKind { App, Tool, DiskImage, System, Cmdr, Unclassified }
```

- M3a ships every holder as `Unclassified` with its executable name; M3b fills in the kinds.
- `Display` for the log and MCP: `unmount refused (held by Warp [app, pid 94646], lsd [system, pid 983]): <detail>`.
- MCP: keep the message, and set `ToolError.data` to `{ "outcome": "unmountRefused", "holders": [...] }`.
- `pnpm bindings:regen`, then update `wire_tests`, the `in_flight` tests, and the `unmount_tool` tests that destructure
  `UnmountRefused { detail }`.

### Frontend wording (M4)

- `wordUnmountRefusal(holders)`, pure, in `eject-error-messages.ts`. Precedence:
  1. Named holders (`App`, `Tool`), deduped by name: one → `errors.eject.unmountRefusedByApp`; two or more →
     `unmountRefusedByApps`. `{apps}` is up to three names, plus the count-free `errors.eject.otherApps` as the last
     item when there are more, joined by `formatConjunctionList`.
  2. `DiskImage` → `unmountRefusedByDiskImage`.
  3. `Cmdr` → `unmountRefusedByCmdr`; the backend also logs `warn`.
  4. `System` → `unmountRefusedBySystem`.
  5. Empty or only `Unclassified` → the existing `unmountRefused`.
- Draft English, each read after "Couldn't eject {volumeName}: " (raw family: single apostrophes, literal tokens, names
  without quotes):
  - `unmountRefusedByApp`: "{app} is still using this drive. Close anything it has open there, then eject again."
  - `unmountRefusedByApps`: "{apps} are still using this drive. Close anything they have open there, then eject again."
  - `otherApps`: "other apps"
  - `unmountRefusedByDiskImage`: "A disk image stored on this drive is still open. Eject that image first, then eject
    this drive."
  - `unmountRefusedBySystem`: "macOS is still working with this drive. Wait a minute, then eject again."
  - `unmountRefusedByCmdr`: "Cmdr itself is still using this drive. Wait a moment and eject again, or send a report if
    it keeps happening."
  - With OQ4 answered as "Ok plus a short toast" (the lead's lean), one more non-error key, for example
    `fileExplorer.pane.ejectedNotPoweredDownToast`: "{volumeName} is unmounted and safe to unplug, but it didn't power
    down."
- `formatConjunctionList(items)` in a new `$lib/intl/list-format.ts`: `Intl.ListFormat` with `getUiLocale()`, memoized
  per locale. It lives in `$lib/intl` because `cmdr/no-raw-locale-format` forbids a formatter in feature code. Fold it
  into the existing `number-format.ts` bullet of `apps/desktop/src/lib/intl/CLAUDE.md` (592 words).
- Each `@key` description names the toast surface, says the string follows "Couldn't eject {volumeName}: …", explains
  the tokens, marks Cmdr and macOS as names, and (for `otherApps`) says it's the last item of a list, so it must read
  naturally after the language's "and".

## Milestones

Order: **M0a ∥ M0b ∥ M1 → M2 → M3a → M3b → M4 → release checkpoint → M5 → M6.**

- **M0a, M0b, and M1 run in parallel.** Each touches a different spike image, and M1 lands the runner.
- **Why M0b comes before M2**: M2 already does DA and IOKit resolution.
- **The release checkpoint** is an FF-merge to David's local `main`. A tagged release happens only when David says so.

### M0a: holder-facts spike (throwaway)

- **Scope**: the holder unknowns, on synthetic APFS images in the scratchpad, recorded in § "Spike results" with
  evidence anchors.
- **Intentions**:
  - Reproduce the self-holder with a binary running from an image, and confirm facts step 4 skips the Security calls and
    the refusal no longer names the prober.
  - Time the Security calls for an executable off the volume.
  - Pick the nested-image signal (an IOKit property on the disk-image device, or `hdiutil info -plist`'s structured
    keys, never text), and confirm `diskimagesiod` holds the outer volume.
  - `dlsym` finds `responsibility_get_pid_responsible_for_pid` and attributes the Google Drive WebKit helper.
  - `lstat` device IDs match mount-table entries on APFS.
- **Landmines**:
  - BSD names from `attach -plist`; detach only after `info -plist` confirms the node is ours.
  - Inner images first, never `-force` the outer.
  - No FAT, exFAT, or physical media.
- **Test plan**: none; it's a spike.
- **DONE**: every intention answered; the plan is amended where one changes the design.
- **Docs**: § "Spike results" only.
- **Size**: half a day.

### M0b: DiskArbitration spike (throwaway)

- **Scope**: the resolution and bridge unknowns, same rules as M0a.
- **Intentions**:
  - The IOKit walk from an APFS volume reaches the physical store; descendants list every container.
  - `kDADiskDescriptionMediaBSDUnitKey` and `IORegistryEntryGetRegistryEntryID` read as documented, and the entry ID
    changes across detach and re-attach while the unit may repeat.
  - The container-then-physical Whole sequence unmounts a mounted APFS volume, and the eject then detaches.
  - One session on its own serial queue delivers callbacks for success, EBUSY, and a queueing failure (synchronously).
  - The dissenter PID arrives through the container's Whole request.
  - `dlsym(RTLD_DEFAULT, "DADissenterGetProcessID")` resolves in a Rust binary that links DA.
  - Re-time a refused container unmount and its retry, to confirm or correct the budgets.
  - A sibling-PARTITION image (two GPT partitions) can be built without FAT or exFAT; if not, that case stays manual QA.
- **Landmines**: as M0a.
- **Test plan**: none.
- **DONE**: every intention answered; § "Deadlines and budgets" amended if the timings move.
- **Docs**: § "Spike results" only.
- **Size**: half a day.

### M1: characterize today's eject, and share the guarded runner

- **Scope**:
  - The guarded runner moves into `cmdr-fs` behind its `testing` feature, and `external_drive_fixture` is repointed at
    it (one runner, no `jscpd` duplicate). The runner gains:
    - an APFS image fixture;
    - a guarded `diskutil apfs addVolume <container> APFS <name>` (the same SIGKILL deadline; nothing else from
      `diskutil`);
    - plist-based identity: `attach -plist` for the nodes, and `info -plist` checked before every `Drop` detach (S5).
  - `#[ignore]` real-image tests in a new `eject/real_image.rs`, declared
    `#[cfg(all(test, target_os = "macos"))] mod real_image;`, so the filter is
    `test(file_system::volume::eject::real_image::)`.
- **Intentions**:
  - The entry seam is `run_teardown(volume_id, Teardown::Tool { verb: UnmountVerb::Eject, mount_path })` directly, never
    `eject()`, which needs the registry, `index_host::install`, and NSURL.
  - Pins:
    - idle → `Ok` and detached;
    - a held file → `UnmountRefused`;
    - the two-volume container with a file held on the sibling, volume A ejected → whatever today's code answers,
      recorded as observed (the safety probe saw A unmount and B stay mounted), commented as the gap M5 flips.
  - The retry logic already has unit tests in `unmount_tool.rs`; don't add more.
- **Landmines**:
  - A child `sleep` spawned by the test descends from the test process, so M3b classifies it `Cmdr`; assert PID
    membership.
  - Attach once; the eject under test is the detach.
  - `nextest-filter-coverage`: add a `[[profile.default.overrides]]` in the `disk-image` group, with a 30 s cap and an
    `allowed-unmatched-nextest-filter` comment (macOS only).
  - `external_drive_fixture`'s FSKit discipline stays intact, and its tests must pass.
- **Test plan**:
  - `pnpm check rust`.
  - Hand runs (the named exception):
    `cd apps/desktop/src-tauri && cargo nextest run -p cmdr --run-ignored only -E 'test(file_system::volume::eject::real_image::)'`,
    plus the index fixture's command from its `DETAILS.md:94`.
- **DONE**: pins are green on current code, both hand-run results are in the commit body, and checks are green.
- **Docs**: `crates/cmdr-fs` testing docs, `crates/cmdr-index/src/indexing/tests/DETAILS.md` (the runner's new home),
  and `volume/DETAILS.md` § "Eject" (the real-image tests and how to run them).
- **Size**: about 350 lines.

### M2: disk resolution, per-disk flights, and sibling safety (today's `diskutil` path)

- **Prerequisite**: B1's fix has landed (`stop_removable_volume` waits for the drain).
- **Scope**:
  - `eject/disk_target.rs` and `eject/disk_flight.rs`.
  - `EjectStep::DiskResolve`, and `disk: Option<&DiskTarget>` on `Teardown::Tool`.
  - `objc2-disk-arbitration` (features `DADisk`, `DADissenter`, `DASession`, `dispatch2`) and `objc2-io-kit`, added per
    `docs/guides/add-rust-dependency.md`: crates.io latest and at least three days old (0.3.2, published 2025-10-04,
    re-check on the day), `madsmtm/objc2` not archived, `pnpm check desktop-rust-cargo-deny`.
- **Intentions**:
  - Resolution stall or failure → `NotResponding { DiskResolve }` with nothing stopped or unmounted (S1).
  - Two ejects on one disk join one disk flight.
  - The sibling busy gate.
  - All sibling stops inside the one `INDEX_STOP_DEADLINE`.
  - A resume only after a final `UnmountRefused`, only for running indexes on still-mounted volumes (S3).
  - `mounted_volumes` rebuilt fresh on every call (S4).
- **Landmines**:
  - `index_host::index()` can't be installed in a test: use real resolution plus a RECORDING `stop_index` future through
    `stop_index_then_unmount`'s parameter, and a recording resume seam.
  - Register test volumes in the global `VolumeManager` under unique IDs and remove them (`volume/DETAILS.md` § "Test
    isolation for the global `VolumeManager`").
  - `volumes/disk_image.rs` stays on raw FFI until M5, keeping this milestone narrow.
  - `volume/CLAUDE.md` is at 599 words: rewrite the eject bullet no longer.
- **Test plan**:
  - Pure tests: sibling selection over a fake `DiskTarget` and registry (`volume.root()` compared, not `find_by_root`),
    the busy gate, the join, the resume matrix (refused / timed out / not responding / success × indexing or not × still
    mounted or not), and resolution failure → `NotResponding`.
  - Hand-run real-image test: a two-volume container, eject one, assert the recording seam stopped both before the
    teardown, and that resolution keyed the physical whole, not the container.
  - `pnpm check`.
- **DONE**: siblings are gated and stopped first on today's path, refusals resume, checks are green, and the hand-run
  result is in the commit body.
- **Docs**:
  - `volume/DETAILS.md` § "Eject" (resolution, per-disk flights, siblings, `DiskResolve`, resume);
  - `volume/CLAUDE.md` (the rewritten bullet);
  - `crates/cmdr-index/src/indexing/DETAILS.md` § the unmount/eject lifecycle (siblings stop too);
  - `docs/guides/error-handling.md` if `EjectStep` is listed there.
- **Size**: about 450 lines.

### M3a: holder scan and wire type

- **Scope**:
  - `eject/holders/{mod.rs, scan.rs}`, `merge`, and `UnmountRefused { holders, detail }`.
  - `VolumeHolder`, with every kind `Unclassified` for now.
  - `Display`, the MCP `data`, and bindings.
  - A frontend stub: the `unmountRefused` arm keeps today's copy, so the frontend compiles.
- **Intentions**:
  - Scan once after the final refusal, over still-mounted paths from a fresh `mounted_volumes`.
  - Dedupe and the ESRCH drop.
  - An injected `HOLDER_BUDGET`.
  - SMB never scans; the Linux arm returns empty.
- **Landmines**:
  - Scanning inside the retry loop multiplies up to 9.4 s by the attempts.
  - A path that left the table scans the boot volume.
  - `// SAFETY:` on the FFI.
  - `specta` doc comments land in `bindings.ts`.
- **Test plan**:
  - Pure unit tests for `merge` and the budget (injected, paused clock).
  - An unignored macOS test: a child holds a temp FILE, and `proc_listpidspath` on that file path WITHOUT
    `PATH_IS_VOLUME` returns it, well inside the 8 s cap.
  - The M1 busy pin extended to assert the holder PID (hand run, result in the commit body).
  - `pnpm check`.
- **DONE**: MCP replies carry holders in `data`, bindings are regenerated, and checks are green.
- **Docs**: `volume/DETAILS.md` § "Eject" (holders, capture order, budget); `mcp/DETAILS.md` (the `eject` tool's
  `data`); `docs/guides/error-handling.md` (`detail` versus holders).
- **Size**: about 300 lines.

### M3b: holder facts and classification

- **Scope**: `eject/holders/{facts.rs, nested_images.rs}`, `classify`, the responsible-PID `dlsym` (if OQ2 is approved),
  and the Security extern.
- **Intentions**:
  - The `classify` table: `lsd`, `mds_stores`, Finder, zsh under Warp, the WebKit helper (with and without OQ2), an
    orphaned `sleep`, a third-party daemon, an executable on the target volume, self, and a descendant of self.
  - A nested image.
  - Facts past the injected budget stay `Unclassified`.
- **Landmines**:
  - A Security call against an executable on the volume makes Cmdr the holder.
  - `NSRunningApplication` without an autorelease pool leaks.
  - Release every `SecCode` and CF reference at scope end.
- **Test plan**:
  - Pure `classify` tests over recorded facts.
  - A hand-run real-image test: a binary copied onto the image and run from it holds the volume, and the refusal names
    it as `Tool`, not `Cmdr`.
  - `pnpm check`.
- **DONE**: kinds are filled in, checks are green, and the hand-run result is in the commit body.
- **Docs**: `volume/DETAILS.md` § "Eject" (classification and the known tradeoffs).
- **Size**: about 400 lines.

### M4: copy in every catalog, then the release checkpoint

- **Scope**:
  - `wordUnmountRefusal` and `formatConjunctionList`.
  - The six `errors.eject.*` keys, plus the OQ4 key if David picks "Ok plus a short toast".
  - `@key` descriptions and translations per `docs/guides/i18n-translation.md` § "New feature → add strings and
    translate to ALL languages": the ten full locales, plus `en-GB` / `en-AU` only where wording differs.
- **Intentions**: the precedence and drafts above; "other apps" starts at four named holders; names are never quoted.
- **Landmines**:
  - A raw family fails `desktop-i18n-icu` on a doubled apostrophe; tokens stay verbatim for `desktop-i18n-parity`.
  - A key with no call site fails `desktop-message-keys-unused`.
  - Run `pnpm intl:keys` and `node apps/desktop/scripts/sync-locale-keys.ts`.
  - `apps/desktop/src/lib/intl/CLAUDE.md` is at 592 words; fold, don't add.
- **Test plan**:
  - `eject-error-messages.test.ts`: one, two, three, four, and six apps; `DiskImage`; `Cmdr`; `System`; mixed cases;
    empty; only `Unclassified`.
  - A list-formatter test with a pinned locale.
  - `pnpm check svelte` plus `desktop-i18n-icu`, `desktop-i18n-parity`, `desktop-i18n-coverage`,
    `desktop-i18n-term-consistency`, `desktop-message-keys-fresh`, and `desktop-message-keys-unused`.
- **DONE**: every locale carries the keys and checks are green. Then the **release checkpoint**: an FF-merge to David's
  local `main`, carrying named holders, sibling safety, and resume. A tagged release only when David says.
- **Docs**: `apps/desktop/src/lib/file-explorer/navigation/DETAILS.md` § "A refusal speaks the catalog", its `CLAUDE.md`
  line on `wordEjectRefusal`, and `apps/desktop/src/lib/intl/CLAUDE.md` (the folded bullet).
- **Size**: about 150 lines of code plus six or seven keys across the catalogs.

### M5: DiskArbitration teardown for disk volumes

- **Scope**:
  - `eject/disk_arbitration/`, `EjectAction::DiskEject`, and `Teardown::Disk`.
  - `settle_with_retries` generic over the outcome type, and the new budget constants.
  - The DA dissenter PID merged ahead of the scan.
  - `volumes/disk_image.rs` migrated to `objc2-disk-arbitration` (one binding style), its real-DMG test repointed to the
    shared guarded runner under the `disk-image` nextest override. No raw `cargo test`.
- **Intentions**:
  - The sequence, statuses, and budgets above; the sibling refusal is a refusal.
  - DMG, USB, and SD go through the physical whole; SMB and Linux paths are byte-identical.
- **Landmines**:
  - A physical Whole request that skips containers.
  - The synthesized-container eject.
  - A double `Box::from_raw`.
  - A `DADiskRef` from another session never calls back.
  - A queue shared across sessions can stall on `_DASessionCallback`'s synchronous MIG call.
  - The hung-share DMG.
  - Hardened runtime: `dlsym` of a system symbol isn't library validation, but that's unverified, so smoke-test a signed
    release build.
  - `UnmountedNotPoweredDown` maps per OQ4.
  - `volume/CLAUDE.md` at 599 words.
- **Test plan**:
  - Pure tests for `DaStatus::from_raw` (`0x0000C010`, `0xF8DA0002`, `0xF8DA0006`, `0xF8DA0007`, `0xF8DA0008`, other
    errnos) and every `settle_disk` row, including `StillMounted` and both eject-stage branches.
  - The budget arithmetic on a paused clock.
  - M1's pins with the sibling pin flipped to `UnmountRefused`, plus a mounted APFS image ejecting through the
    container-then-physical sequence (hand run, result in the commit body).
  - `pnpm check`, then `pnpm check --include-slow`.
  - Manual QA for David:
    - an APFS and an exFAT USB stick;
    - an SD card;
    - a two-partition drive;
    - a DMG opened from Finder;
    - an app launched from its DMG;
    - Preview holding a file;
    - a terminal `cd`'d into the drive;
    - a fresh drive while Spotlight indexes it;
    - a DMG stored on another external drive;
    - a signed release build.
- **DONE**:
  - Disk ejects run through DA, and `rg 'UnmountVerb::Eject' apps/desktop/src-tauri/src/file_system/volume/eject/`
    returns nothing.
  - Checks and hand runs are green, and manual QA is listed for David.
- **Docs**:
  - `volume/DETAILS.md` § "Eject" (the DA sequence, statuses, budgets, and the closed sibling gap; delete the "Known
    gap" sentence);
  - `volume/CLAUDE.md` (bullet rewritten, no longer);
  - `eject/mod.rs` and `unmount_tool.rs` module docs;
  - `apps/desktop/src-tauri/src/commands/DETAILS.md` (`eject.rs`);
  - `apps/desktop/src-tauri/src/volumes/DETAILS.md` (the `disk_image.rs` binding).
- **Size**: about 600 lines.

### M6: close-out sweep

- **Scope**: a conformance review against the invariants, and a docs audit that reads every commit body and every
  touched `CLAUDE.md`.
- **Intentions**: anything a milestone missed; describe current state only.
- **Landmines**: `claude-md-length` on `volume/CLAUDE.md` and `intl/CLAUDE.md`.
- **Test plan**:
  `pnpm check docs-dead-links docs-reachable docs-link-text docs-section-refs claude-md-length resident-doc-budget oxfmt`.
- **DONE**:
  - `rg -l diskutil` over the docs (excluding `docs/specs`, `docs/notes`, `docs/i18n`) matches only SMB, Linux, and the
    FSKit incident notes.
  - The conformance review comes back clean.
  - This plan moves to "Shipped, kept for review" in `docs/specs/index.md`.
- **Size**: small.

## Rollback

- Each milestone lands as its own commits and reverts with `git revert`. Nothing persists: no settings, no databases, no
  migrations. The index resume uses the drive's existing enable marker.
- Reverting M5 returns disk ejects to `diskutil` and keeps M2's sibling safety and resume, M3's holders, and M4's copy.
- M2 reverts on its own, as long as M3a's scan falls back to this volume's root when `disk` is `None`.
- M3a, M3b, and M4 revert together (wire type and keys), after M5; regenerate bindings after.

## Invariants (the conformance register)

1. No classification by any string: statuses, errnos, PIDs, policies, signing attributes, BSD units, and mount-table
   entries only.
2. Every registered volume on the physical disk passes the busy gate, and has its index stopped inside one deadline,
   before any unmount.
3. Nothing unmounts after resolution or an index stop fails or stalls; those answer `NotResponding`.
4. One flight per volume, and one per physical disk, keyed by IOKit registry entry ID; a sibling's eject joins.
5. A refusal is logged once, in the teardown path, with the raw statuses and holders.
6. Synthesized containers are Whole-unmounted before the physical whole; the eject target is the physical whole.
7. An eject never runs while a freshly rebuilt `mounted_volumes` is non-empty; an eject-stage dissent is never plain
   success.
8. A whole-disk busy status is a refusal even when this volume's root left the table.
9. `TimedOut` only when a teardown request started; it never claims cancellation, and it never triggers a resume.
10. An index resumes only after a final `UnmountRefused`, only on a still-mounted volume, and only if it was running.
11. Holders are captured once per eject, after the last attempt, over still-mounted paths only, before any fact
    gathering, within the injected budget, never for network volumes.
12. No code-signing query runs against a process whose executable is on the target volume; every fact reference is
    released before returning.
13. Every DA and resolve call runs under a deadline, on one session per eject scheduled on its own serial queue; each
    callback reclaims its box exactly once and never panics.
14. Private symbols are looked up through `dlsym`; their absence means fewer names, never a failure.
15. Every user-facing word is catalog copy in all 13 catalogs; `detail` never reaches a toast.
16. Real-disk tests and spikes: synthetic APFS only, plist-verified identity before every detach, inner images first,
    hand-run with the result recorded.
17. SMB disconnect and Linux teardown behave exactly as before.

## Open questions for David

Recommendations come from the reviewers and the lead; all still wait for David.

1. **A shell `cd`'d into the drive**: name the terminal app, or the process ("zsh")? Recommended: the app, never "zsh".
2. **A third-party app's WebKit or XPC helper**: attribute it through the private
   `responsibility_get_pid_responsible_for_pid`, looked up through `dlsym`? Recommended: yes. It decides part of M3b.
3. **SMB refusals**: skip the holder scan, or scan, since a refusal proves the share answered? Recommended: skip.
4. **Every volume unmounted, but the power-down refused** (safe to unplug, may stay powered): (a) a silent `Ok`, today's
   behavior; (b) `Ok` plus a short toast; (c) a refusal? The lead leans (b), which adds one key to M4.
5. **The six English strings**, and names without quotes. Recommended: no quotes.
6. **Finder offers "Force Eject…" after a refusal.** Should Cmdr's refusal toast offer it, weighed against principle 1?
   A force unmount under an app with unsaved writes can lose them.
7. **The real-image tests run in no lane** (hand-run only, like the index fixture's). Add an opt-in macOS lane, for
   example under `--include-slow` on macOS only?
8. **The budgets** in § "Deadlines and budgets": 5 s ejectability, 5 s resolve, 15 s index stop for all siblings, 30 s
   per DA request, one retry after 1 s, 45 s teardown budget, and 1.5 s holders. A typical refusal answers in about 19
   s; the worst case is about 72 s (today's is about 83 s). They decide M2's and M5's constants.

## Spike results

M0a's holder spike adds its own subsection. The DiskArbitration approval-hook spike below covers the unmount-approval
questions, part of M0b, and the M0a facts it could check without new risk.

### DiskArbitration approval-hook spike

Verified on macOS 26.6.2 (25G83), unsandboxed uid 501, load about 2, 2026-09-14, with a throwaway Swift probe (not
committed): DA sessions on serial dispatch queues whose callbacks MATCH only the spike's own volume names, against fresh
APFS and HFS+ DMGs attached `-nobrowse`, with every mutating call gated on `hdiutil info -plist` plus
`diskutil info -plist` identity. Timings come from the probe's clock and `log stream` on `diskarbitrationd`. Apple
sources: DiskArbitration-535.0.10.

#### Design consequences

- **An unmount-approval session is the reliable pre-unmount hook for DA-mediated unmounts.** DA asks it before
  `diskutil unmount`, `unmountDisk`, and `eject`, `NSWorkspace unmountAndEjectDevice(at:)`, `hdiutil detach`, and
  `hdiutil detach -force`, once per mounted volume. It never sees a raw `unmount(2)`: `/sbin/umount` by the mount's
  owner needs no sudo and only shows up afterwards (`DidUnmount`, a `DAVolumePath` description change).
- **It holds an unmount for less than 10 s per ask, never longer.** Past that, DA logs the session as not responding and
  unmounts without the answer. An index stop inside the ask needs a budget well under 10 s, which `INDEX_STOP_DEADLINE`
  (15 s) isn't.
- **A timed-out session that registered ONLY approval callbacks is never asked again.** Register another callback kind
  (idle or description-changed) on the same session, so its next delivery clears the timeout, or recreate the session.
- **Asks run one after another on the session's queue.** A Whole request on a two-volume APFS container delivers both
  asks back to back, so a slow first ask spends the second's 10 s too: stop every sibling in the first ask.
- **Force can't be refused.** DA still asks, then ignores the dissent, so the hook does its stop work on every ask and
  never relies on dissenting.
- **An ask, or `WillUnmount`, doesn't mean the unmount happens.** Both fire for a request the kernel then refuses
  (EBUSY), and for every volume of a Whole request before any unmount. Whatever the hook stops must resume after a
  refusal; the trigger is DA's idle callback while the volume's `DAVolumePath` is still set.
  - ❗ Today's `handle_volume_will_unmount` (`apps/desktop/src-tauri/src/volumes/watcher.rs`) stops the index, and
    nothing resumes it when that unmount is refused.
- **`WillUnmount` is posted synchronously from AppKit's own DA approval callback, on the main thread.** Today's handler
  is racy only because it spawns a thread and returns. Blocking there would hold the unmount, but it freezes Cmdr's main
  thread and meets the same 10 s limit, so a dedicated DA session on its own queue is the better home.
- **Dissent works on non-force requests, and each caller reports it differently.** `diskutil` names the dissenting PID
  and its parent; the DA API answers `0xF8DA0002` with the dissenter's PID; NSWorkspace throws a bare `OSStatus` -47 and
  can leave a multi-partition disk partly unmounted. Dissenting to protect in-flight writes is a product decision with
  that cost.
- **A vanished disk is distinguishable from an eject**: a disappeared callback for the whole disk with NO eject approval
  before it. A description change alone can't tell. Measured only with the volume already unmounted.
- **`NSWorkspace unmountAndEjectDevice(at:)` on an APFS DMG ejects the synthesized container**: success, both volumes
  unmounted, image still attached. It confirms "never eject the synthesized container", and warns that API-driven ejects
  of APFS media may not power down.
- **Responsible-PID attribution works, but responsible apps are often accessory (menu bar) apps, and Google Drive's has
  no `NSRunningApplication`.** Facts steps 2 and 3 can't require a regular activation policy.
- **Index databases live on the Mac, never on the drive**, so a pulled drive can't corrupt them; the pre-unmount stop
  exists for the FSEvents stream and open handles. A copy interrupted by a pull leaves `<name>.cmdr-tmp-<uuid>` files
  that nothing cleans up.

#### 1. Approval coverage

- **`diskutil unmount <path>`**: asked, 7–12 ms after the solicitation. On macOS 26, `diskutil` requests go through
  `storagekitd`: DA's log names `storagekitd [47890]` as the requester.
- **`diskutil unmountDisk disk7`** (HFS+, two partitions): asked once per volume, one after the other (s2, then s1 190
  ms later).
- **`diskutil eject <path>`** (APFS DMG): asked. The sequence is `DADiskUnmount(disk6s1)` (not Whole), then
  `DADiskEject(disk6)` (the container), then `DADiskEject(disk5)` (physical, which detached); each eject asked the
  eject-approval callback.
- **`NSWorkspace unmountAndEjectDevice(at:)`**: asked; the requester is the calling process.
  - APFS: `DADiskUnmount(container, Whole)`, both volumes asked, then `DADiskEject(container)`. It answered OK in 122
    ms, and `hdiutil info` still listed the image.
  - HFS+ two partitions: `WillUnmount` for both.
- **`hdiutil detach disk8`** (APFS): asked. `DADiskUnmount(disk9 container, Whole)`, `DADiskUnmount(disk8, Whole)`, then
  `DADiskEject(disk8)`, in 132 ms.
- **`hdiutil detach -force`**: asked (options `0x00080001`, Force and Whole); it ignored a dissent and detached in 279
  ms.
- **`/sbin/umount /Volumes/X`, no sudo**: succeeded in 74 ms with no ask and no `WillUnmount`, only `DidUnmount`, a
  `DAVolumePath` change, and idle. sudo isn't available, and root wasn't needed for a DA mount the user owns.

#### 2. Per volume on a whole-disk eject

- **Yes, one ask per mounted volume**: HFS+ partitions (`unmountDisk`, `hdiutil detach`) and APFS container volumes
  (NSWorkspace eject, container Whole). The APFS pair arrived in the same millisecond.
- **Building a two-volume APFS image**: `diskutil apfs addVolume` answered -69493 on the SYNTHESIZED container of a 128
  MB image too, and succeeded on a 1.1 GB sparse image (`hdiutil create -size 1100m -type SPARSE -fs APFS`, then
  `addVolume <container> APFS <name> -nomount`). So the earlier -69493 came from the container's size (APFS scales its
  volume cap with container size), not the node. The exact formula is unverified.
- **A two-GPT-partition image without FAT**: `hdiutil create -layout GPTSPUD -fs HFS+`, attach, then
  `diskutil partitionDisk <disk> GPT JHFS+ A 60M JHFS+ B R`. `partitionDisk` remounts both browsable, so remount them
  with `diskutil mount -mountOptions nobrowse`. This answers M0b's sibling-partition intention: real-image tests can
  cover it.

#### 3. Blocking answer

- **2 s block**: `diskutil unmount` took 2.256 s (unblocked baseline 0.279 s). **8 s**: 8.368 s.
- **12 s**: DA logged `daprobe [<pid>]:<id> not responding.` 10.66 s after the solicitation (`__kDAResponseTimerLimit`
  10 plus the 1 s grace, `diskarbitrationd/DAQueue.c:45-46,174-199`), unmounted without the answer (`diskutil` 10.87 s),
  and ignored the late approval. Reproduced twice.
- **Recovery**: the timeout flag clears only when the client copies its callback queue (`DAServer.c:2147`), and approval
  dispatch skips a flagged session (`DAQueue.c:609`).
  - A session that also registered appeared, disappeared, description-changed, and idle callbacks was asked again on the
    next request (and timed out again).
  - An approval-only session was NOT asked on the next request (0.212 s, no ask): it stays silent for its lifetime.

#### 4. Dissent

`DADissenterCreate(kDAReturnBusy, "spike dissent")` from the approval callback:

- **`diskutil unmount`**: exit 1 in 0.135 s; stderr named the dissent string, the approver's PID, and its parent's PID
  and path.
- **DA API `DADiskUnmount`** (not Whole): 9 ms, status `0xF8DA0002`, and `DADissenterGetProcessID` (through `dlsym`)
  answered the approver's PID.
- **`hdiutil detach`** (Whole, two partitions): both asked, exit 2, "Resource busy", nothing unmounted.
- **NSWorkspace eject** (two HFS+ partitions, dissent on H1 only): threw `NSOSStatusErrorDomain` -47 with an empty
  `userInfo` and no process named, in 82 ms, and left H2 unmounted with H1 still mounted.
- **Force** (`diskutil unmount force`, `hdiutil detach -force`): asked, dissent ignored, unmounted (`DARequest.c:1610`).

#### 5. Disappearance without a request

- **Simulated** by SIGKILLing the image's own `diskimages-helper`: a legacy DiskImages image whose `hdid-pid` served
  only that image, with its volume already unmounted. DA logged `removed disk` for all four nodes, and the watcher's
  disappeared callbacks for `disk5`, `disk6`, and `disk6s1` came 12 ms later, with no eject solicitation, eject
  approval, or unmount approval.
- **Every DA-mediated eject or detach** in this spike delivered the whole disk's eject approval 14–18 ms before its
  disappeared callbacks.
- **`hdiutil detach -force` is not a simulation**: it asks (§ 1).
- **Raw `umount` is the unmount-without-request shape**: `DidUnmount` with no `WillUnmount` and no ask, and the media
  stays.
- **Unverified: a MOUNTED volume vanishing** (a real pulled cable). From source: DA marks the disk a zombie and issues
  its own `DADiskUnmount(disk, Force)` (`DAServer.c:1425-1434,1514`), a zombie request skips approval
  (`DARequest.c:1388`), and it may show the device-removal dialog (`DAServer.c:1495,1537`). Not run: killing the helper
  under a mounted filesystem is a new kernel-level risk.

#### 6. Refused-unmount settle signal

- **A holder process with the H1 volume root open**, so the kernel answers EBUSY:
  - `diskutil unmount`: the idle callback came 53 ms after DA logged the failure, and `diskutil` exited 81 ms after
    that; its stderr named the holder PID.
  - DA API unmount: status `0x0000C010` with the holder as dissenter PID, in 132 ms; idle 1 ms after the requester's
    callback.
  - `unmountDisk` with H1 held: H2 unmounted, H1 refused (partial), idle after each.
- **A refusal brings no description change and no `DidUnmount`**, but `WillUnmount` was posted for it.
- **Idle means "DA's queue is quiet"** and also fires after successes, so pair it with the volume's `DAVolumePath`
  (still set means refused). It's private (`DiskArbitrationPrivate.h:331`, `DARegisterIdleCallback` through `dlsym`); no
  public callback marks a refusal to an observer.
- **Refusals took 0.13–0.2 s at load about 2**, far below the 12.2 s under load in § "Evidence the design rests on", so
  the budgets stand.

#### 7. `NSWorkspaceWillUnmountNotification`

- **Posted from AppKit's own DA approval callback, synchronously, on the main thread.** A watcher with NO approval
  callback of its own, sleeping 8 s in its observer: `diskutil unmount` took 8.290 s, and DA logged "unmounted disk,
  ongoing" in the same millisecond the observer returned.
- **Sleeping 12 s**: DA logged the WATCHER process as not responding at 10.66 s (AppKit's is the only DA session in that
  process) and unmounted; `DidUnmount` arrived after the observer returned. AppKit's session recovered: the next unmount
  posted `WillUnmount` again.
- **With an approval session present**, AppKit's `WillUnmount` and the probe's ask arrived 1–6 ms apart: DA asks every
  session at once and waits for all of them.
- **Unverified**: whether AppKit's session filters by disk, so whether a slow observer also delays OTHER disks' unmounts
  (needs two concurrent disks).

#### 8. M0a holder facts checked here

- **`dlsym(RTLD_DEFAULT, "responsibility_get_pid_responsible_for_pid")`** resolves in an unsandboxed, non-root, unsigned
  Swift binary, at 2–8 µs per call:
  - `com.apple.WebKit.WebContent` (parent launchd) → BetterDisplay (accessory), Google Drive (`NSRunningApplication` nil
    for the responsible PID), CleanShot X (accessory), and Cmdr.app (regular).
  - `com.apple.WebKit.GPU` → iStat Menus Helper (accessory); a Chrome renderer → Google Chrome (regular).
  - Standalone agents (iStat Menus Helper, `ViewBridgeAuxiliary`, `DiskUnmountWatcher`) → themselves.
- **`DADissenterGetProcessID` through `dlsym`** resolves in a binary linking DA (also an M0b intention).
- **`lstat` devices**: on the APFS image, `st_dev`, the mount point's `f_fsid.val[0]`, and its `getfsstat` entry all
  read 16777241. On the boot volume, `/`'s `st_dev` (16777234) differs from its `f_fsid.val[0]` (16777235, the sealed
  system snapshot), so compare against the mount point's own `stat().st_dev`, not `f_fsid`.
- **Security calls off the volume** (`SecCodeCopyGuestWithAttributes`, then `SecCodeCopySigningInformation`): Finder 4
  plus 3 ms; both `lsd` processes and a WebContent helper 1 ms or less each (platform identifier 26); iStat Menus Helper
  2 plus 43 ms (no platform identifier).
- **`hdiutil info -plist`** carries the typed per-image keys `hdid-pid` and `diskimages2` besides `image-path`.
- **BSD units repeat at once**: a detached image's `disk8` and `disk9` went to the next attach, and image A came back as
  `disk5` and `disk6` both times it was re-attached.
- **Not run** (new risk, or M3b code): the nested-image signal, `diskimagesiod` holding an outer volume, and the
  self-holder reproduction.

#### 9. Code facts

- **Index databases**: on the Mac, in the app data dir, never on the drive.
  - `index-<volume_id>.db` plus its WAL and SHM (`crates/cmdr-index/src/indexing/lifecycle/state.rs:400`).
  - `media-<volume_id>.db` (`crates/cmdr-index/src/media_index/store/mod.rs:259`); background media passes skip
    `LocalExternal` drives (`media_index/scheduler/lifecycle.rs:184`).
  - `importance-<volume_id>.db`. No indexer writes to the drive.
- **In-progress copy temps**: `<name>.cmdr-tmp-<uuid v4>` beside the final file, plus `<name>.cmdr-temp-<uuid>` for an
  original set aside during an overwrite (`crates/cmdr-fs/src/staging.rs:62,67,153`); `is_staging_temp_name` recognizes
  both (`:80`). A cross-filesystem move stages under `<dest>/.cmdr-staging-<operation_id>/`
  (`write_operations/transfer/move_op/cross_fs.rs:107`), which `is_staging_temp_name` doesn't match.
- **Leftovers after a pull stay**: `discard_temp` ignores the failed remove and deregisters anyway
  (`write_operations/overwrite.rs:176-178`), so the startup sweep (`in_flight_temps.rs:329`) never retries it, and that
  sweep counts NotFound as gone (`:419`). The age-based reaper runs only in the cross-volume engine, not for Mac-to-USB
  copies.
