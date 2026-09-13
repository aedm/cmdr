# Eject and drive safety

**Problem.** Cmdr doesn't reliably let go of a removable drive before it unmounts, doesn't survive a drive that
vanishes, and can't say what holds a drive it couldn't eject.

- **Letting go is racy.** Cmdr's own eject stops only THIS volume's index. An eject from Finder, `diskutil`, or another
  app reaches Cmdr through `NSWorkspaceWillUnmountNotification`, whose handler spawns a thread and returns, so the
  unmount races the stop (`apps/desktop/src-tauri/src/volumes/watcher.rs:257`). Nothing resumes an index that handler
  stopped when the unmount is then refused. An indexed exFAT drive unmounting under a live FSEvents stream is the FSKit
  wedge that kernel-panicked a Mac on 2026-07-15.
- **"Released" doesn't mean nothing reads the drive.** The per-volume hold drops with the index manager, but 11 kinds of
  worker (walker threads, reconcile readers, cover walks, a live loop past its drain timeout) keep reading after it
  drops (§ "Code map").
- **A vanished drive corrupts what Cmdr knows.** Workers turn failed reads into row deletions, a scan whose drive left
  mid-walk stamps itself complete, and a transfer to or from the drive ends with a generic error, no progress facts, and
  a partial file on the drive that nothing ever sweeps.
- **A refused eject names nobody**, and a disk with two partitions or APFS volumes can read "ejected" while a sibling
  stays mounted and powered.

**Outcome.**

- Cmdr answers DiskArbitration's unmount approval for every DA-mediated unmount, whoever starts it: it lets go of every
  volume of the disk inside the ask, dissents when it can't in time, and resumes whatever it stopped when the unmount
  doesn't happen.
- "Released" means no Cmdr worker reads the drive; a worker stuck in a read on that drive keeps it mounted, and the log
  names which worker.
- A drive that vanishes (pulled cable, raw `umount`) stops every worker quietly: no row deleted from a failed read, no
  index marked complete. An index left inconsistent is invalidated for a fresh scan with a notice; a transfer says how
  far it got and what's where; leftover temp files are swept when the drive comes back.
- Cmdr's own eject works per physical disk: it joins ejects of sibling volumes, gates and stops every sibling first,
  refuses honestly when a sibling stays mounted, and resumes after a refusal.
- A refusal names its holders: an app, several apps, a disk image, Cmdr itself, or macOS.

**Status.** Planned 2026-09-14, not started. It combines the earlier DiskArbitration eject plan (review rounds 1–3 and
the approval-hook spike folded in) with the drive-safety decisions below.

- **Landed prerequisites**: the refusal retry (`unmount_tool::settle_with_retries`), the `NotEjectable` preflight, the
  eject deadlines, `TOOL_TIMEOUT` at 30 s, and the index-stop wait (`Index::stop_removable_volume` answers
  `RemovableStop`, waiting on `VolumeHold`).

## Loud rules

- ❌ **Never a physical disk, and never a FAT, exFAT, or MS-DOS image, in any test, probe, or spike this plan adds or
  runs.** Synthetic APFS and HFS+ images only. The existing FAT fixture tests in
  `crates/cmdr-index/src/indexing/tests/external_drive_fixture.rs` keep their hand-run status and stay out of every lane
  this plan adds.
- ❌ **Before every mutating disk call in a test** (`hdiutil detach`,
  `diskutil eject|unmount|mount|partitionDisk|apfs addVolume`), check identity against the image the test attached:
  `hdiutil info -plist` still maps that `/dev/diskN` to OUR `image-path`. DA reuses BSD unit numbers at once, so a
  stored node can become someone's Time Machine drive. Detach nested images inner first; never `-force` an outer image.
- ❌ **Every `hdiutil` and `diskutil` call in a test runs under a hard SIGKILL timeout** (the guarded runner).
  Production `diskutil` runs under `TOOL_TIMEOUT`.
- ❌ **The approver answers "approve" within milliseconds for any disk Cmdr has no index or write op on**, touches no
  filesystem inside a DA callback (DA-cached descriptions and in-memory registries only), and never unwinds across FFI.
  A slow or panicking ask path delays every unmount on the Mac by up to 10 s.
- ❌ **No classification by any string**: `diskutil` stderr, a DA status string, a process name, or a path prefix.
  Decide on typed variants, errnos, DA status codes, PIDs, activation policies, signing attributes, BSD units, device
  IDs, and mount-table entries (`error-string-match`, `cmdr/no-error-string-match`).
- ❌ **`index-crate-isolation` ceilings move only with a stated reason** in the commit and in
  `crates/cmdr-index/src/indexing/handle/DETAILS.md`. Measured 2026-09-14: 53 of 53 root promises, 40 of 40 `Index`
  methods, 17 of 17 public modules, 155 of 156 items. This plan's design spends none; a milestone that finds it must,
  says why.
- ❌ **Nothing unmounts after a pre-unmount step fails or stalls** in Cmdr's own eject: resolution, the ejectability
  check, and the index stop answer `NotResponding { step }` and stop there.
- ❌ **A worker stuck in a read on the drive keeps the drive mounted** (`StillReleasing`, a dissent, `NotResponding`).
  Never unmount under it: that's the wedge-prone case.
- ❌ **A resume goes through `drive_release::resume`**, which re-checks the per-drive intent and the master switch
  (`Index::drives_to_resume`) and skips volumes another owner is ejecting. ❌ Never a bare `Index::start_volume`: it
  writes the "user asked for this drive" marker and would override a disable made mid-eject.
- ❌ **A worker deletes a row only on `ENOENT`/`ENOTDIR` from a complete listing of a volume still in the mount table.**
  Any other read failure writes nothing.
- ❌ **Invalidating a vanished drive's index never calls `clear_index`**: it deletes the database, and the per-drive
  intent markers live in it (`crates/cmdr-index/src/indexing/lifecycle/master.rs:113-121`).
- ❌ **Private symbols come from `dlsym` only**: `DARegisterIdleCallback` and
  `responsibility_get_pid_responsible_for_pid`. A missing symbol degrades (no resume after a refused OS unmount; fewer
  names), never fails to link.
- ❌ **Holder facts never run a code-signing query against a process whose executable lives on the target volume**, and
  holders are never scanned on a path that left the mount table (`PATH_IS_VOLUME` on a plain directory scans the boot
  volume).
- **Every user-facing word comes from the catalog, in all 13 catalogs.** English drafts below are for David's later
  review and don't block.
- **Docs ship with each milestone** (`AGENTS.md` § Docs). `apps/desktop/src-tauri/src/file_system/volume/CLAUDE.md` is
  at 599 words and `crates/cmdr-index/src/indexing/lifecycle/cover/CLAUDE.md` at 598: rewrite a bullet no longer, move
  depth to the sibling `DETAILS.md`.
- **Checks run through `pnpm check`, foreground only.** Until M2 lands the lane, M1's real-image tests are a hand-run
  `cargo nextest` whose result goes in the commit body; from M2 on, `pnpm check disk-images`.

## Decisions

Product decisions are David's; technical ones are the lead's. Don't reopen them.

1. **The drive is let go before any unmount, whoever starts it**, through a DA unmount approver on its own session and
   dispatch queue, replacing the `WillUnmount` handler. It does the stop work on every ask; dissent is the non-force
   fallback. It resumes on DA's idle callback while `DAVolumePath` is still set. Cmdr's own eject keeps its explicit
   pre-stop, so the approver answers it at once. A raw `umount` bypasses DA and is handled like a pulled drive.
2. **Every background worker that reads the drive carries a labelled share of `VolumeHold`** beside its cancel signal.
   The hold moves to a leaf module the scanner, reconcile, and watch code may import.
3. **A vanished drive is handled gracefully and explained**: detection, quiet workers, impact notices only where
   something needs doing, temp sweeps on return, and the two temp-cleanup bugs fixed.
4. **Partitions and holders**: real disk-image tests with the shared guarded runner, disk resolution and per-disk
   flights with the round-3 should-fixes, a holder scan, holder facts, and copy in every catalog.
5. **Holder details**: responsible-PID attribution through `dlsym` with a fallback and no regular-policy requirement; a
   shell names its terminal app; SMB refusals scan holders too; the eject copy in § "Copy drafts" is approved with names
   unquoted; no Force Eject; an opt-in macOS disk-image lane under `--include-slow`.
6. **Deferred**: the DA teardown swap, and "unmounted but not powered down" stays a silent success (§ "Deferred").
7. **Fix**: nothing resumes an index `handle_volume_will_unmount` stopped when the unmount is refused.

## Evidence the design rests on

Unless marked otherwise: verified on macOS 26.6.2 (25G83), unsandboxed uid 501, with throwaway probes against 50–60 MB
APFS DMGs attached `-nobrowse`, 2026-09-12 (review rounds 1–3). Apple sources: DiskArbitration-535.0.10, xnu-12377.1.9.
The approval-hook spike (2026-09-14) is § "Spike results" at the end, and it wins where they differ.

### DiskArbitration

- **Busy is `unix_err(EBUSY)`, `0x0000C010`**. The daemon rewrites any `unmount(2)` failure to EBUSY
  (`diskarbitrationd/DARequest.c:1525`). `kDAReturnBusy` (`0xF8DA0002`) covers `/`, the Data volume, or an approval
  dissent.
- **A refusal is slow because of the daemon's holder scan.** After EBUSY, `diskarbitrationd` runs a root
  `proc_listpidspath(PROC_ALL_PIDS, PATH_IS_VOLUME)` to name the dissenter before it answers (`DARequest.c:1534→1707`),
  for `diskutil` (via `storagekitd`) and the DA API alike. At load 2.2–2.5: `diskutil eject` of a held APFS image 20.4,
  1.8, 0.5, 27.8, and 0.6 s; DA container Whole 5.1, 1.9, 0.45, 0.42 s; HFS+ `diskutil` 1.15 and 0.50 s. At load about 2
  on 2026-09-14 refusals took 0.13–0.2 s.
- **A container Whole unmount reaches every volume in the container, and partial unmounts are real**: two-volume APFS
  image, file held on volume B, A unmounted, B stayed mounted.
- **Whole links subrequests by BSD unit** (`DAQueue.c:974`), so `DADiskUnmount(physical, Whole)` doesn't reach the APFS
  volumes of its synthesized container.
- **`DADiskCreateFromVolumePath` runs `statfs` then `realpath`** (`DiskArbitration/DADisk.c:413-421`), so it hangs on a
  DMG whose backing file lives on a hung SMB share. It returns NULL for a path that's no longer a mount.
- **DA doesn't track SMB**: `diskutil info /Volumes/naspi` prints "Could not find disk".
- **Bridge facts**: callbacks arrive on the disk's own session (`DiskArbitration.c:257,1273`);
  `DASessionSetDispatchQueue` retains the session until cancelled (`DASession.c:720,737`); `_DASessionCallback` makes a
  synchronous MIG call on its queue (`DASession.c:242`), so a queue belongs to one session.
- **IOKit ancestry of an APFS volume** (`ioreg -p IOService -t`): `AppleAPFSVolume` → `AppleAPFSContainer` (the
  synthesized whole disk) → `AppleAPFSContainerScheme` → `IOMedia` (the physical store partition) → the physical whole.
- **Bindings**: `objc2-disk-arbitration` 0.3.2 (published 2025-10-04; `madsmtm/objc2` not archived, 1,032 stars, pushed
  2026-09-12) binds `DASessionSetDispatchQueue`, `DARegisterDiskUnmountApprovalCallback` (callback returns
  `*const DADissenter`), `DARegisterDiskEjectApprovalCallback`, `DARegisterDiskDisappearedCallback`,
  `DARegisterDiskDescriptionChangedCallback`, `DADissenterCreate`, `DADiskUnmount`, and `DADiskEject` (verified by
  reading the published crate, 2026-09-14). Not bound: the private `DARegisterIdleCallback`, declared
  `void DARegisterIdleCallback(DASessionRef, void (*)(void *context), void *context)`
  (`DiskArbitrationPrivate.h:329,331`). `dispatch2` 0.3.1 (2026-02-26) and `objc2-io-kit` 0.3.2 are already in
  `Cargo.lock` transitively. Re-check versions and the 3-day window on the day of adding.

### Holders

- **`proc_listpidspath` is public (`libproc.h:85`) but sees same-uid processes only** (`CHECK_SAME_USER`,
  `xnu/bsd/kern/proc_info.c:2196-2211`). It catches open files, cwd, and per-thread cwd, misses memory-mapped-only
  holders, and `stat`s its path first (`proc_listpidspath.c:83`). With `PATH_IS_VOLUME`: 142 ms to 9.4 s under load; a
  healthy SMB share took 67 ms.
- **A Security query can make the prober the holder.** With a binary running FROM the volume,
  `SecCodeCopyGuestWithAttributes` put the probe in `proc_listpidspath`, `SecCodeCopySigningInformation` took 4.5 s, and
  a Whole unmount in that window named the probe as dissenter.
- **Typed classification signals** (`NSRunningApplication`, `SecCodeCopyGuestWithAttributes` +
  `SecCodeCopySigningInformation`, ppid from `PROC_PIDT_SHORTBSDINFO`):
  - `/usr/libexec/lsd` (uid 0 and uid 501): parent launchd, Apple platform binary (platform identifier 26), no
    `NSRunningApplication`. `mds_stores` (uid 308): same shape.
  - Finder: platform binary AND a regular activation policy.
  - `/bin/zsh` in a terminal: platform binary, no app; the parent chain reaches Warp (regular).
  - `/bin/sleep` and Calculator are platform binaries too, so "platform binary" alone never means "macOS".
  - Responsible-PID attribution works, but responsible apps are often accessory apps, and Google Drive's has no
    `NSRunningApplication` (§ "Spike results" 8).
- **ERR-TT2FH**: the dissenter was `/usr/libexec/lsd`, registering an unseen `.app` after a pane fetched its icon, for
  about 0.3–0.9 s. The retry exists for it.
- **Holds "wait a minute" doesn't fit**: `diskimagesiod` holds a volume while an image stored on it stays attached;
  `backupd` holds it for a whole Time Machine backup.

## Code map

Verified against this branch at `15292aa2b` on 2026-09-14, with `codegraph` and by reading the lines.

### Eject and unmount hooks

- `apps/desktop/src-tauri/src/file_system/volume/eject/mod.rs`:
  - `eject` (`:237`) → `in_flight::join_or_start` (`in_flight.rs:67`, one flight per volume id) → `eject_now` (`:243`):
    busy gate for THIS volume (`:253`), device provider, `is_already_unmounted` (`:274`), `resolve_is_ejectable` under
    `EJECTABILITY_CHECK_DEADLINE` 5 s, then `stop_index_then_unmount` (`:436`) with `stop_index_blocking` (`:469`) under
    `INDEX_STOP_DEADLINE` 15 s, then `run_teardown` (`:362`).
  - `EjectError` IS the wire type (`:120`); `EjectStep::{EjectabilityCheck, IndexStop}` (`:179`).
- `eject/unmount_tool.rs`: `TOOL_TIMEOUT` 30 s (`:25`), `run` (`:94`), `within_tool_timeout` drops the join handle on
  expiry (`:109-113`), `settle` (`:169`), `is_still_mounted` (`:192`, over `volumes::mounts::is_mount_point`,
  `volumes/mounts.rs:81`, `getfsstat(MNT_NOWAIT)`), `REFUSAL_RETRY_BACKOFF` (`:204`), `RETRY_BUDGET` 30 s (`:218`),
  `settle_with_retries` (`:245`).
- `apps/desktop/src-tauri/src/volumes/watcher.rs`: `WillUnmount` observer (`:74-82`, `:102-107`) →
  `handle_volume_will_unmount` (`:257`) → `stop_local_external_index_off_main` (`:301`, a thread, `INDEX_RELEASE_WAIT`
  15 s, logs only). `DidUnmount` → `handle_volume_unmounted` (`:174`) removes the root and stops a `LocalExternal` index
  as cleanup. Installed at `apps/desktop/src-tauri/src/lib.rs:406`.
- `apps/desktop/src-tauri/src/volumes/disk_image.rs:39-76`: the only DA code today, raw `extern "C"`.
- `apps/desktop/src-tauri/src/mcp/executor/eject.rs`: flattens the error into `ToolError::internal(format!(..))`.
- Frontend: `apps/desktop/src/lib/file-explorer/navigation/eject-error-messages.ts` (`EJECT_MESSAGE` record,
  `wordEjectRefusal`), keys `errors.eject.*` in `apps/desktop/src/lib/intl/messages/en/errors.json` (a raw family:
  `getMessage`, literal tokens, no plurals).

### The index: holds, workers, and the resume door

- `crates/cmdr-index/src/indexing/lifecycle/state/release.rs`: `VolumeHold` (`:47`), `take` (`:54`), a per-volume count
  and one condvar, `wait_until_released` (`:98`). Depends only on `volume.rs` and `cmdr_fs`. Taken in
  `try_reserve_initializing_phase` (`state/reservation.rs:77`), stored as the manager's `_hold`
  (`lifecycle/manager.rs:112`).
- `state/teardown.rs`: `stop_removable_volume` (`:119`) stops, then waits on the hold; `stop_the_volume` (`:163`);
  `finish_stopping` (`:242`); `record_the_disable` (`:320`), the after-drain write slot.
- `IndexManager::shutdown` (`lifecycle/manager.rs:716-754`): cancels the volume token, stops phases without joining,
  drops `scan_handle` without joining, stops the watcher (joins its run loop), waits for the live loop at most 5 s
  (`:741`, then detaches it), shuts the writer down.
- `Index` (`handle/mod.rs`): `start_volume` (`:207`, records the enable marker at `:261`), `stop_removable_volume`
  (`:356`), `drives_to_resume` (`:382` → `master.rs:175-196`, volumes whose intent says run and that aren't active),
  `volume_kind` (`:440`), `cover` (`:584`).
- **Workers that still read the drive after the hold drops** (file:line of the spawn):
  1. walker worker threads `index-walk` (`scanner/walker/engine.rs:311`; deliberately never joined, `:104-106`);
  2. `index-scanner` (`scanner/mod.rs:567`) and `index-local-reconcile` (`reconcile/local_reconcile.rs:234`);
  3. `reconcile-read` threads, abandoned on timeout (`local_reconcile.rs:128`, `:183-190`);
  4. the scan completion task, including its replay and a live loop it can spawn after `shutdown` emptied the slot
     (`lifecycle/scan_completion.rs:411-431`, `manager.rs:737`);
  5. `rescan-subtree` threads (`reconcile/reconciler/rescan/mod.rs:493`);
  6. `index-phases` (`lifecycle/phases/mod.rs:317`) and its `index-cover` walk;
  7. search cover walks, on the CALLER's token (`lifecycle/cover/mod.rs:261`, `:311`), which teardown never cancels;
  8. verifier tasks and their `scan_subtree` walks (`lifecycle/state/scan_control.rs:78,106`,
     `reconcile/verifier.rs:120,249`);
  9. the live loop after its 5 s drain timeout;
  10. `index-mount-probe` threads (`lifecycle/cover/bootstrap.rs:209`);
  11. a media network pass on a `LocalExternal` id in a hand-edited opt-in list: `run_network_pass_blocking`
      (`media_index/scheduler/mod.rs:474-526`) has no kind gate, while background passes skip `LocalExternal`
      (`media_index/scheduler/lifecycle.rs:184`).
- Not workers for this plan: the enable probe's `mount_facts` blocking task
  (`transports/local_external/index.rs:103-113`) runs before any reservation exists; the importance scheduler reads the
  DB (its Spotlight sample, `importance/last_used.rs:53`, is believed to use index-relative paths: M3 verifies);
  thumbnails (`apps/desktop/src-tauri/src/commands/media_index/thumbnail.rs:38-70`) are foreground reads like a listing.
- **Layering**: nothing in `scanner/`, `reconcile/`, or `watch/` imports `lifecycle::state`; `volume.rs` and
  `metadata.rs` are the shared leaves, and `volume.rs:3-7` says "pure predicates only".

### The index on a vanished drive

- **Rows deleted from failed reads**:
  - the reconcile listing drops entries whose iteration or stat fails (`reconcile/reconciler.rs:1408-1414`,
    `:1439-1448`), then the diff deletes whatever the listing lacks (`:817-826`);
  - the verifier drops entries whose stat fails (`reconcile/verifier.rs:249-257`) and deletes the rest (`:312-324`);
  - the live loop deletes a row when ANY stat fails (`reconciler.rs:1646-1669`), and for a removal event (`:1578-1619`).
- **False completion**:
  - a full scan whose drive left after the root listed marks each failed directory `Abandoned`
    (`scanner/insert_visitor.rs:442-460`), returns `Ok`, runs `ComputeAllAggregates`, and stamps `scan_completed_at`
    (`lifecycle/scan_completion.rs:297-346`);
  - the phase machine skips its vanished-volume check when a pass covered anything (`phases/mod.rs:454-461`), and
    `Abandoned` ground isn't frontier, so it stamps completion (`phases/completion.rs:106-108`, `:128-143`).
- **Existing vanish checks**: `ScanError::RootUnlistable` (`scanner/mod.rs:463-473`, `scan_completion.rs:92`) and
  `report_a_vanished_volume_if_that_is_what_happened` (`phases/mod.rs:746-770`). Nothing in the crate asks the mount
  table. Only a fatal SQLite error fails an index.
- **The host seam** `host::volumes::VolumeProvider` (`host/volumes.rs:99-150`) has no presence query today.

### Transfers and temps

- The frontend routes every copy, and every move touching a non-root volume, through `copy_between_volumes` /
  `move_between_volumes` (`apps/desktop/src/lib/file-operations/transfer/transfer-dispatch.ts:83-92`, `:162-168`).
  Between two `LocalPosixVolume`s (Mac ↔ USB) that runs the LOCAL engine, with both volume ids in the busy set
  (`write_operations/transfer/volume/copy.rs:143-186`, `write_operations/status_cache.rs:226-230`).
- `classify_io_error` (`write_operations/error_classification.rs:25-61`) maps only `ENODEV` to
  `WriteOperationError::DeviceDisconnected { path }` (`types.rs:403-406`); `ENXIO` and `EIO` become `IoError`, `ENOENT`
  becomes `SourceNotFound` on either side. `WriteErrorEvent` (`types/events.rs:158-165`) carries no progress;
  `files_done`/`bytes_done` live in the status cache (`status_cache.rs:70-74`) and go when the op unregisters.
- Temps: `STAGING_TEMP_MARKER` `.cmdr-tmp-` and `STAGING_ASIDE_MARKER` `.cmdr-temp-`
  (`crates/cmdr-fs/src/staging.rs:62,67`), `is_staging_temp_name` (`:79-81`). The cross-FS move stages under
  `<dest>/.cmdr-staging-<operation_id>/` (`write_operations/transfer/move_op/cross_fs.rs:107-111`), which nothing
  recognizes by name.
- The ledger `in-flight-temps.log` (`write_operations/in_flight_temps.rs`): the local engine registers `LocalFs` temps
  (`overwrite.rs:96`); the aside (`overwrite.rs:~107`) is never registered. `discard_temp` ignores a failed remove and
  deregisters anyway (`overwrite.rs:176-179`). The startup sweep counts `NotFound` as gone and forgets the record
  (`in_flight_temps.rs:419`), so a partial on a drive absent at launch is forgotten. Volume-homed records defer to
  `VolumeManager::on_volume_arrival` (`volume/manager.rs:135`; `in_flight_temps.rs:359-374`, `:469-479`).

## Target design

### One release, one resume, three triggers

`apps/desktop/src-tauri/src/file_system/volume/drive_release.rs` (new, `pub(crate)`) is the one place that stops
removable indexes and the one place that resumes them. Three callers use it: the approver (M4), a vanished drive (M5a),
and Cmdr's own eject (M6). **Why one module**: the eject flight's explicit sibling stop and the approver's stop are the
same work with different budgets; building it twice would let their answers drift.

- `release(volume_ids, budget) -> Release`: runs `Index::stop_removable_volume` for every id concurrently on blocking
  threads, each bounded by `budget` from one start instant, and answers per id: `NothingToStop`,
  `Released { was_indexing }`, or `StillReleasing`. `was_indexing` is captured before the stop
  (`Index::volume_kind(id) == Some(LocalExternal)`). A stop still running at the budget keeps running detached; its
  eventual answer is delivered to an optional continuation (M6 uses it to resume).
- `resume(candidates, owner)`: for each id that `was_indexing`, is still in the mount table, is in
  `Index::drives_to_resume()` (intent says run, master switch aside), and isn't being ejected by a different owner,
  spawn `Index::start_volume(id)` on `tauri::async_runtime` (❌ never `tokio::spawn` from a DA or GCD thread).
  `start_volume` then applies the master switch. `owner` is `Approver` or `EjectFlight(DiskKey)`: the approver skips ids
  in the ejecting set, a flight resumes only its own.
- **Why `drives_to_resume` instead of a new `Index` method**: it already encodes "intent says run, no veto" and the
  `Index` surface is at its ceiling (40 of 40).

### Worker holds (M3)

- **Home**: `crates/cmdr-index/src/indexing/hold.rs`, a third shared leaf beside `volume.rs` and `metadata.rs`.
  `release.rs` moves there unchanged in mechanics. **Why a new leaf**: scanner, reconcile, and watch must import it,
  they may not import `lifecycle::state`, and `volume.rs` promises pure predicates.
- **Labels**: `VolumeHold { volume_id, kind: HoldKind }`. `HoldKind` has one variant per spawn site in § "Code map" plus
  `Reservation` (the root taken at reservation). The count table becomes per volume, per kind, so `wait_until_released`
  answers `Released` or `StillHeld(Vec<(HoldKind, usize)>)`, and `stop_removable_volume`'s `warn` names the kinds.
  `RemovableStop`'s public shape doesn't change (no ceiling spent).
- **The pairing is a type**: `VolumeWork { cancel: CancellationToken, hold: VolumeHold }` with
  `child(kind) -> VolumeWork` (a child token plus a share). Every spawn function in the inventory takes `VolumeWork`
  where it took a bare `CancellationToken`, so drive-reading work without a hold doesn't compile. The share moves into
  the thread or task and drops when it exits, abandoned walker workers included.
- **Roots**: the registry instance's `signals` carries the volume's root `VolumeWork` (its `cancel` is today's
  `signals.cancel`), so work started from outside the manager (the verifier via `scan_control.rs`, cover walks via
  `Index::cover`) takes its share from the instance. The manager keeps its own share.
- **Search cover walks** stop on either token: `VolumeWork::linked(caller_token, &volume_work, kind)` keeps the walk a
  child of the caller's token (preemption and search cancel unchanged) and cancels it when the volume's token cancels.
- **Late live loop**: the scan completion task spawns the live loop only while the volume token isn't cancelled, under
  the slot lock, and the loop carries a `LiveLoop` share.
- **Media**: `run_network_pass_blocking` refuses a volume whose index kind isn't a network kind, before any read.
- **What "released" means afterwards**: no Cmdr index worker reads the drive. `lifecycle/DETAILS.md` § "When a volume
  has been let go" loses its "neither is part of the answer" paragraph.

### The unmount approver (M4)

**Home**: `apps/desktop/src-tauri/src/volumes/unmount_approver/` (macOS only), installed from `lib.rs` next to
`start_volume_watcher` and after `index_host::install`:

- `mod.rs`: `install(seams)`; one `DASession`, scheduled with `DASessionSetDispatchQueue` on its own serial
  `dispatch2::DispatchQueue`; callback registration; `catch_unwind` around every callback body.
- `ask.rs`: the pure decision over typed inputs.
- `resume.rs`: the pure resume decision.
- `causes.rs` (M5a): the pure unmount-cause machine.
- `private_symbols.rs`: the `DARegisterIdleCallback` lookup through `dlsym(RTLD_DEFAULT, ..)`, once.
- `volumes/disk_units.rs` (M4, shared with M6): maps the non-blocking mount table to BSD units through
  `DADiskCreateFromBSDName` on a given session (touches no filesystem) and answers
  `mounted_volumes_on(session, units) -> Vec<MountedVolume { bsd_unit, whole_unit, path }>`, rebuilt from a fresh table
  on every call.

**Callbacks registered** (NULL match, so every disk; the ask filters):

- unmount approval (the ask);
- disk appeared, disappeared, and description changed (watch key `kDADiskDescriptionVolumePathKey`): M5a's causes, and
  they keep the session recoverable after a DA response timeout (§ "Spike results" 3);
- idle, through `dlsym`: the resume trigger;
- eject approval (M5a): records the eject and approves at once.

**An ask** (on the DA queue):

1. Copy the disk's description: `VolumePath` and the whole disk's BSD unit (`DADiskCopyWholeDisk`,
   `kDADiskDescriptionMediaBSDUnitKey`). No path, or no Cmdr volume whose ACTIVE root is that path → approve.
2. The group: every registered volume mounted on the same whole unit (`disk_units`). **Why the whole unit and not the
   physical disk**: DA links a Whole request's asks by BSD unit, and asks of one request arrive back to back sharing one
   response window, so the first ask must stop the whole unit's group. A physical disk's other container is a different
   request with its own window.
3. `drive_release::release(group, APPROVAL_STOP_BUDGET)`. A group with no index work answers `NothingToStop` for every
   id at once: that's the "approve immediately" path.
4. Answer: dissent (`DADissenterCreate(kDAReturnBusy, NULL)`) if any id is `StillReleasing`, or if any id is in
   `busy_volume_ids()`; otherwise approve. The callback can't see the force option: DA ignores a dissent under force (§
   "Spike results" 4), which is why the stop runs first on every ask.
5. Record every `Released { was_indexing: true }` id with its retained `DADiskRef` in `stopped_by_ask`.
6. Log one `info` per ask (volume, group, outcome, elapsed ms); the crate's `warn` names holders on `StillReleasing`.

**`APPROVAL_STOP_BUDGET` = 7 s.** DA's response timer is 10 s from the solicitation (`__kDAResponseTimerLimit`,
`DAQueue.c:45-46`; the 1 s grace isn't relied on). The first ask of a Whole request spends the window of the asks behind
it, which then answer in milliseconds; delivery measured 7–12 ms at load 2, and a loaded machine adds scheduling jitter.
7 s leaves 3 s (30 %) for those, and still covers the 5 s live-loop drain bound inside `shutdown`. A stop that runs past
it answers `StillReleasing` and dissents: honest, and the drive stays mounted on a non-force request.

**Resume** (on the idle callback, same queue): for each `stopped_by_ask` record, copy the disk's description.
`VolumePath` still set → the unmount didn't happen → `drive_release::resume([id], Approver)`, drop the record.
`VolumePath` gone, or the disk disappeared → drop the record. If the idle symbol is missing, log one `warn` at install
and resume nothing (a refused OS unmount leaves that drive unindexed until its next start, today's behavior); ❌ no
polling fallback.

**Cmdr's own eject** pre-stops its volumes under `INDEX_STOP_DEADLINE`, so its asks find nothing to stop and approve at
once, and the flight owns the resume (the approver skips ids in the ejecting set).

**What goes**: the `WillUnmount` observer and `handle_volume_will_unmount`. `DidUnmount` stays as registry cleanup.
Linux has no approver and keeps its `DidUnmount`-style cleanup.

### A vanished drive (M5a, M5b)

**Causes** (`causes.rs`, pure, fed in callback order: one serial queue makes order the truth, ❌ never timing):

- `Appeared(whole)` resets that disk's facts.
- `UnmountAsked(volume, whole)` marks the volume asked.
- `EjectApproved(whole)` marks the disk ejected.
- `VolumePathCleared(volume)` (description change; the machine keeps the last-known path) → `Asked` if marked, otherwise
  `Unasked` (a raw `umount` by the mount's owner, or DA's own zombie force unmount of a pulled disk, which skips
  approval, `DARequest.c:1388`).
- `Disappeared(whole)` → `Pulled` if the disk had a Cmdr-known volume mounted since it appeared and no eject approval,
  else `Ejected`.
- For `Unasked` or `Pulled`: drop that disk's `stopped_by_ask` records (never resume a vanished drive), hand
  `drive_release::release([id], VANISH_STOP_WAIT)` to a thread off the DA queue, and log a `warn` naming the cause.

**The index writes nothing wrong** (crate-side, whatever the host detects and whenever):

- **Presence seam**: `VolumeProvider::is_mounted(&self, root: &Path) -> Option<bool>` (`None`: the table couldn't be
  read), answered app-side from `volumes::mounts::is_mount_point` (macOS) and
  `file_system::linux_mounts::is_mount_point` (Linux). `FakeVolumeProvider` gains a `mark_unmounted` switch. No new
  type, so no ceiling spent (M5a confirms with `pnpm check index-crate-isolation`).
- **Deletes**: a directory listing with any per-entry error is incomplete and deletes nothing; a stat failure deletes
  only on `NotFound`/`ENOTDIR`; a delete batch is sent only when `is_mounted(root) == Some(true)`; after sending one,
  `Some(false)` marks the volume **suspect** (the batch may have come from a drive going away).
- **Completion**: `scan_completion` and the phase machine stamp `scan_completed_at` and run the final aggregation only
  when `is_mounted(root) == Some(true)`. A walk with abandoned ground on a volume now gone is a vanish: no stamp,
  `ScanAborted`, freshness `ScanFailed`.
- **Partial index**: no stamp means the next start resumes or rescans (`IncompletePreviousScan` or the phase frontier).
  No notice.
- **Suspect index**: after the drain, in the after-drain slot `record_the_disable` uses, the stop invalidates the index
  so the next start is a fresh full walk while the per-drive intent markers stay (reuse the invalidation of
  `crates/cmdr-index/src/indexing/network_scanner/DETAILS.md` § "Rebuilding an index that predates the current list"),
  and emits a new `IndexEvent::IndexNeedsFreshScan { volume_id }` (no new type). The app words it as a notice. **Why not
  a bare stamp removal**: the phase machine would find its coverage marks intact and stamp again over the missing rows.

**Transfers** (M5b):

- **Classification**: an I/O error on a side whose volume root has left the mount table is
  `DeviceDisconnected { path, side }`, whatever the errno; `ENXIO` joins `ENODEV` as typed evidence. The engines capture
  each side's mount root at start from the non-blocking table.
- **Progress facts**: `WriteErrorEvent` gains `progress_at_stop` (`files_done`, `files_total`, `bytes_done`,
  `bytes_total`), read from the status cache before the op unregisters, plus what a move had done to its sources.
- **Mac-side cleanup** stays what it is (the local partial goes via `discard_temp`, the Mac staging dir via
  `remove_dir_all_in_background`). On the gone drive nothing can be removed, so everything left is recorded for the
  sweep.

**Temps** (M5b):

1. `discard_temp` deregisters only when the remove succeeded, or failed with `NotFound` while the temp's mount is still
   listed. Otherwise the record stays and joins the pending-arrival set at once, installing the arrival listener lazily,
   so a re-plug in the same session sweeps it.
2. A temp on a non-root mount registers volume-homed (`TempHome::Volume(volume_id)`, path relative to the root), so a
   launch with the drive absent defers it to arrival instead of forgetting it.
3. The `.cmdr-temp-` aside registers too, and its sweep restores or removes: destination missing → rename the aside
   back; destination present → remove the aside. ❌ Never remove an aside whose destination is missing: it holds the
   user's original.
4. `cmdr_fs::staging` gains `STAGING_DIR_PREFIX` (`.cmdr-staging-`) and `is_staging_dir_name`. The cross-FS move
   registers its staging dir when it creates it and deregisters it when it removes it; the listing hide gate learns the
   name, still by ownership.
5. The sweep runs on `on_volume_arrival`, through the existing volume-record path.

### Cmdr's own eject, per physical disk (M6)

Order of an eject of a disk volume:

1. **Join a disk flight first**: if the volume id belongs to a running disk flight (a `volume_id → DiskKey` map the
   flight fills), join it. **Why first**: a sibling B whose flight already unmounted it would otherwise pass
   `is_already_unmounted` and answer `Ok` while A's flight may still refuse (round-3 should-fix 5).
2. Per-volume preflight, unchanged: busy gate, device provider, `is_already_unmounted`, the ejectability check.
3. **Resolve** (`eject/disk_target.rs`) under `DISK_RESOLVE_DEADLINE` 5 s on a session and queue of the flight's own:
   `DADiskCreateFromVolumePath`, `DADiskCopyWholeDisk`; an `AppleAPFSContainer` whole walks `kIOServicePlane` parents to
   the physical store and its whole disk; `DiskKey` = `IORegistryEntryGetRegistryEntryID` of the physical whole;
   `container_units` from its descendants. A stall → `NotResponding { step: DiskResolve }`. A NULL disk → the mount
   table decides: gone → `Ok` (already unmounted); still listed (a non-DA volume like macFUSE) → `disk: None` and
   today's per-volume path (should-fix 3).
4. **Join or start the disk flight** (`eject/disk_flight.rs`), keyed by `DiskKey`; its volumes join the ejecting set.
5. **Capture siblings**: registered volumes whose `volume.root()` is a path in
   `disk_units::mounted_volumes_on(session, [physical_whole_unit] + container_units)`. ❌ Not `find_by_root` alone (it
   matches any known root). The captured paths are kept for the post-teardown check.
6. **Sibling busy gate**: `Busy` if any sibling is in `busy_volume_ids()`.
7. **Stop every sibling**: `drive_release::release(siblings, INDEX_STOP_DEADLINE)`. Any `StillReleasing` →
   `NotResponding { IndexStop }`, and nothing unmounts. Resume the siblings that did release (nothing was unmounted),
   and resume a still-releasing one from its continuation when its stop ends, if it's still mounted (should-fix 1,
   second half).
8. **Teardown**: `diskutil eject <path>` through `settle_with_retries`, with `still_mounted` = any captured sibling path
   still listed OR a fresh `mounted_volumes_on` non-empty (fail closed, should-fix 4), and each retry aimed at a
   still-listed captured path (the original path answers "Failed to find disk" after a partial unmount).
9. **Final `UnmountRefused`** → holders (M7, M8), then `drive_release::resume(siblings, EjectFlight)` for the ones still
   mounted (should-fix 2 lives in `resume`).
10. **`TimedOut`** → answer `TimedOut` at once; `within_tool_timeout` hands the tool's join handle to a detached settle
    task, and when the tool exits, resume still-mounted siblings (should-fix 1).
11. SMB keeps `diskutil unmount` and gains holders (M7); Linux keeps `umount`.

`EjectStep` gains `DiskResolve` (the existing `notResponding` copy stays true: nothing started).

### Holders (M7 scan and wire, M8 facts)

- **When**: once, in `run_teardown`, after a final `UnmountRefused`, for disks and SMB shares.
- **Scan**: `proc_listpidspath(PROC_ALL_PIDS, 0, path, PATH_IS_VOLUME | EXCLUDE_EVTONLY)` over each still-listed
  captured path (the share's mount path for SMB), on ONE abandonable std thread under `HOLDER_BUDGET` 1.5 s (a
  parameter; tests inject their own). A path that left the table isn't scanned. Past the budget the thread is detached
  and the eject answers with what it has. FFI: a two-line `extern "C"` plus `PROC_ALL_PIDS = 1`,
  `PROC_LISTPIDSPATH_PATH_IS_VOLUME = 1`, `PROC_LISTPIDSPATH_EXCLUDE_EVTONLY = 2` (`libproc.h:52,61`,
  `sys/proc_info.h:51`). ❌ No `libproc` crate (it build-depends on `bindgen`). Linux answers empty.
- **Facts**, cheapest first, inside `objc2::rc::autoreleasepool`, walking ancestors up to eight levels and stopping at
  PID 1:
  1. `pid == own_pid` or an ancestor is → `Cmdr`.
  2. `NSRunningApplication` for the process, then each ancestor, with any activation policy but prohibited →
     `App { name, bundle_id }` (a shell under Warp names Warp).
  3. `responsibility_get_pid_responsible_for_pid` through `dlsym`; a responsible process with an `NSRunningApplication`
     of any policy → `App`; one without (Google Drive) → `App` named from its signing information's Info.plist
     (`kSecCodeInfoPList`: `CFBundleDisplayName`, then `CFBundleName`) when its executable isn't on the target volume.
     Symbol missing or no answer → next step.
  4. The executable's device (`lstat` of `proc_pidpath`) equals a mounted target root's own `stat().st_dev` (❌ not
     `f_fsid`) → `Tool { name }`, with no Security call.
  5. Apple platform binary (`SecCodeCopyGuestWithAttributes` + `SecCodeCopySigningInformation` →
     `kSecCodeInfoPlatformIdentifier`) → `System`; else `Tool`.
- **Nested images**: an attached image whose backing file's `st_dev` equals a target root's `st_dev` → one
  `DiskImage { name }` holder. M8 picks how to list attached images and their backing paths (an IOKit property of the
  disk-image device, or `hdiutil info -plist` under a timeout), inside the same budget.
- **Merge**: dedupe by PID in first-seen order, drop ESRCH.
- **Accepted tradeoffs** (recorded in `volume/DETAILS.md`): `backupd` reads `System` for a whole backup; a helper with
  no responsible answer and a launchd parent reads `System`; an orphaned platform CLI (`nohup sleep`) reads `System`; a
  path heuristic was rejected (`/bin/zsh` would read as macOS); root-owned holders (`mds_stores`) aren't visible to a
  same-uid scan, so those refusals name nobody until the deferred DA teardown.

**Wire** (M7):

```rust
UnmountRefused {
    /// Who held the drive when the last attempt was refused, deduped by PID. Empty when nothing could be named.
    holders: Vec<VolumeHolder>,
    /// The tool's own output, for the log and the details line. ❌ Never the message.
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

- M7 ships every holder `Unclassified` with its executable name; M8 fills in kinds.
- `Display`: `unmount refused (held by Warp [app, pid 94646], lsd [system, pid 983]): <detail>`.
- MCP: keep the message; set `ToolError.data` (`mcp/executor/mod.rs:62`) to
  `{ "outcome": "unmountRefused", "holders": [...] }`.

### Budgets and deadlines

- `APPROVAL_STOP_BUDGET` 7 s (new): the approver's stop per ask. Rationale in § "The unmount approver".
- `VANISH_STOP_WAIT` 15 s (new, replaces `INDEX_RELEASE_WAIT`): the stop after a vanish, off the DA queue; logs only.
- `EJECTABILITY_CHECK_DEADLINE` 5 s (unchanged).
- `DISK_RESOLVE_DEADLINE` 5 s (new): resolution is local DA and IOKit, except the `statfs` + `realpath` inside
  `DADiskCreateFromVolumePath`.
- `INDEX_STOP_DEADLINE` 15 s (unchanged), now covering all sibling stops, which run concurrently.
- `TOOL_TIMEOUT` 30 s, `REFUSAL_RETRY_BACKOFF` 0.5, 1, 1.5 s, `RETRY_BUDGET` 30 s (unchanged).
- `HOLDER_BUDGET` 1.5 s (new, injected).
- **What a person waits**: a clean eject under a second; a typical refusal about 3 s of retries plus the daemon's scan
  time per attempt plus 1.5 s; the worst case, every step at its limit, 5 + 5 + 15 + 60 + 1.5 ≈ 87 s (today about 80 s),
  with the ejecting spinner throughout.

### Copy drafts

**Approved** (decision 5, raw family, names unquoted), each read after "Couldn't eject {volumeName}: ":

- `errors.eject.unmountRefusedByApp`: "{app} is still using this drive. Close anything it has open there, then eject
  again."
- `errors.eject.unmountRefusedByApps`: "{apps} are still using this drive. Close anything they have open there, then
  eject again."
- `errors.eject.otherApps`: "other apps"
- `errors.eject.unmountRefusedByDiskImage`: "A disk image stored on this drive is still open. Eject that image first,
  then eject this drive."
- `errors.eject.unmountRefusedBySystem`: "macOS is still working with this drive. Wait a minute, then eject again."
- `errors.eject.unmountRefusedByCmdr`: "Cmdr itself is still using this drive. Wait a moment and eject again, or send a
  report if it keeps happening."

Precedence in `wordUnmountRefusal(holders)`: named `App`/`Tool` holders deduped by name (one → `ByApp`; two or more →
`ByApps`, `{apps}` = up to three names plus `otherApps` as the last item from four, joined by `formatConjunctionList`),
then `DiskImage`, then `Cmdr` (the backend also logs `warn`), then `System`, then the existing `unmountRefused`.
`formatConjunctionList` lives in a new `$lib/intl/list-format.ts` (`Intl.ListFormat` with `getUiLocale()`, memoized per
locale; `cmdr/no-raw-locale-format` forbids it in feature code).

**Drafts for David's later review** (they don't block; key families follow the surface's existing family):

- Index notice (M5a), an info toast: "{volumeName} was disconnected while Cmdr was updating its index. The next scan of
  this drive starts from scratch, so its folder sizes are right again."
- Transfer, copy to the drive (M5b): "{volumeName} was disconnected after Cmdr copied {done} of {total} files to it.
  Your originals are still on your Mac. Connect the drive and copy the rest again; Cmdr removes the unfinished file it
  left there."
- Transfer, copy from the drive: "{volumeName} was disconnected after Cmdr copied {done} of {total} files to
  {destination}. The rest are still on the drive."
- Transfer, move to the drive, no original removed yet: "{volumeName} was disconnected before anything moved, so all
  your files are still on your Mac."
- Transfer, move from the drive: "{volumeName} was disconnected after Cmdr moved {done} of {total} files to
  {destination}. The rest are still on the drive."

## Milestones

Order: **M1 → M2 → M3 → M4 → M5a → M5b → M6 → M7 → M8 → M9 → release checkpoint.**

- **M1 first**: every later milestone's real-image test runs on its harness.
- **M2, the lane, right after M1** (earlier than first sketched): it's small, and it turns every later real-image test
  into `pnpm check disk-images` instead of a hand-run someone can forget.
- **M3 before M4**: the approver's "approve" is only honest when `Released` means no worker reads the drive.
- **M4 before M5a**: pulled-drive detection rides the approver's session, and "asked or not" is the cause.
- **M5a/M5b before M6**: they're the data-integrity risks live today; M6 is Cmdr's own eject, which the approver already
  makes sibling-safe for indexes in the meantime.
- **M7 → M8 → M9** as before; the checkpoint is an FF-merge to David's local `main`, and a tagged release only when he
  says.
- **Cadence**: plain `pnpm check` per milestone; `pnpm check --include-slow` after M4, M5b, M7, and before the
  checkpoint.

### M1: disk-image harness and pins of today's behavior

- **Scope**:
  - The guarded runner moves into `cmdr-fs` behind its `testing` feature, macOS only (a `disk_images` submodule of
    `cmdr_fs::testing`, which becomes a directory module): `run_hdiutil_guarded`, a `run_diskutil_guarded` limited to
    `info -plist`, `apfs addVolume`, `partitionDisk`, and `mount -mountOptions nobrowse`, each with the SIGKILL
    deadline; `external_drive_fixture` repoints at it (one runner, no `jscpd` pair).
  - `DiskImage::attach(ImageSpec)`: `Apfs`, `ApfsTwoVolumes` (`hdiutil create -size 1100m -type SPARSE -fs APFS`, then
    `addVolume <container> APFS <name> -nomount`, then a nobrowse mount), and `HfsTwoPartitions`
    (`-layout GPTSPUD -fs HFS+`, then `diskutil partitionDisk <disk> GPT JHFS+ A 60M JHFS+ B R`, then nobrowse
    remounts). Nodes come from `attach -plist`; `Drop` detaches after the `info -plist` identity check.
  - `#[ignore]` pins in `apps/desktop/src-tauri/src/file_system/volume/eject/real_image.rs`, declared
    `#[cfg(all(test, target_os = "macos"))] mod real_image;`.
  - An `#[ignore]` pin in `crates/cmdr-index/src/indexing/tests/vanish_tests.rs` (macOS): a real `IndexManager` over an
    HFS+ image (the `event_stream_tests.rs` shape) with a tree big enough to scan for a few seconds, detached with
    `hdiutil detach -force` at the first `ScanProgress`.
- **Intentions**:
  - Eject pins enter at `run_teardown(volume_id, Teardown::Tool { verb: UnmountVerb::Eject, mount_path })`, never
    `eject()` (it needs the registry, `index_host::install`, and NSURL): idle → `Ok` and detached; a held file →
    `UnmountRefused`; two-volume APFS with a file held on B, eject A → whatever today answers, recorded and commented as
    the gap M6 flips; the same on the two-partition HFS+ image.
  - The vanish pin records today's outcome (expected: `scan_completed_at` written, rows gone), commented as the gap M5a
    flips.
  - Plist parsing uses a workspace dependency already in `Cargo.lock` if one fits; otherwise the dependency guide.
- **Landmines**:
  - A child `sleep` spawned by a test descends from the test process, so M8 classifies it `Cmdr`: assert PID membership.
  - `crates/cmdr-index/src/indexing/tests/CLAUDE.md` says "never `diskutil unmount` a path": scope that sentence to
    FAT/exFAT images in this milestone, since APFS/HFS+ ejects under test are the point.
  - `nextest-filter-coverage`: add `[[profile.default.overrides]]` in the `disk-image` group for both new test paths, 30
    s cap, with the `allowed-unmatched-nextest-filter` comment (`.config/nextest.toml:76-85` is the model).
  - `addVolume` answers -69493 on a small container; the sparse 1.1 GB image is what works.
- **Test plan**:
  - `pnpm check rust`.
  - Hand runs (named exception, results in the commit body):
    `cargo nextest run -p cmdr --run-ignored only -E 'test(file_system::volume::eject::real_image::)'` and
    `cargo nextest run -p cmdr-index --run-ignored only -E 'test(indexing::tests::vanish_tests::)'`, plus the existing
    `external_drive_fixture` run from `crates/cmdr-index/src/indexing/tests/DETAILS.md`.
  - **Must not change**: `unmount_tool::tests::*`, `eject::tests::*`, `external_drive_fixture::tests::*`.
- **DONE**: pins green on current code, results in the commit body, checks green.
- **Docs**: `crates/cmdr-fs/DETAILS.md` (the runner), `crates/cmdr-index/src/indexing/tests/CLAUDE.md` and `DETAILS.md`
  (the runner's home, the scoped guardrail), `apps/desktop/src-tauri/src/file_system/volume/DETAILS.md` § "Eject" (the
  pins).
- **Size**: about 450 lines.

### M2: the opt-in macOS disk-image lane

- **Scope**: a check `desktop-rust-disk-images` (nickname `disk-images`) in `scripts/check/checks/registry.go`.
- **Intentions**:
  - `IsSlow: true`; `NotInCI` with the reason (every CI runner is ubuntu, and `hdiutil` has no Linux counterpart);
    `Exclusive: ResourceCargoBuildDir`; depends on clippy.
  - On a non-darwin host it answers OK with a "skipped: macOS only" message.
  - It runs nextest `--run-ignored only` over an explicit filter union of this plan's real-image paths, serialized by
    the `disk-image` group. ❌ Not `external_drive_fixture::` (FAT).
  - Each later milestone adds its real-image path to that union.
- **Landmines**: `ci-coverage` and `fixture-lane-coverage`-style checks may want an entry or an exemption; follow what
  they print. The runner's name union and `.config/nextest.toml` overrides must name the same paths.
- **Test plan**: `pnpm check disk-images` on macOS (green, runs M1's pins); the check's Go unit test for the filter
  union; `pnpm check go`.
  - **Must not change**: every other lane's selection (`scripts/check/cli_test.go`).
- **DONE**: `pnpm check --include-slow` runs the pins on macOS; the hand-run exception in this plan's loud rules no
  longer applies.
- **Docs**: `scripts/check/checks/DETAILS.md` (the lane), `docs/tooling/testing.md` (tools inventory),
  `crates/cmdr-index/src/indexing/tests/DETAILS.md` and `volume/DETAILS.md` § "Eject" (how the pins run now).
- **Size**: about 120 lines.

### M3: every worker carries a share of the hold

- **Scope**: `crates/cmdr-index/src/indexing/hold.rs` (from `state/release.rs`), `HoldKind`, `VolumeWork`, and the
  wiring of every worker in § "Code map" items 1–11; the late live-loop guard; the media kind gate.
- **Intentions**:
  - Every spawn site in the inventory takes `VolumeWork` (or `VolumeWork::linked`) and moves its share into the thread
    or task.
  - `wait_until_released` names holders; `stop_removable_volume`'s `warn` lists them.
  - A search cover walk on a volume stops when the volume stops, with preemption unchanged.
  - The live loop is never spawned after the volume token is cancelled.
  - Verify the importance Spotlight sample never touches a `LocalExternal` mount; give it a share if it does.
  - `indexing/CLAUDE.md` names three shared leaves.
- **Landmines**:
  - The hold is still taken INSIDE the reservation's critical section (`state/release.rs` doc): taken after, a stop
    could answer `Released` while a start stands a manager up.
  - An abandoned walker worker holding its share turns a hung read into `StillReleasing`. That's intended; don't "fix"
    it by dropping the share early.
  - `cover/CLAUDE.md` is at 598 words.
  - The `Initializing` teardown arm removes the instance; the instance's root share must drop there exactly as the
    manager's does today.
  - Don't widen `lifecycle::state` imports below `lifecycle`.
- **Test plan** (red first for each wiring test):
  - `hold` unit tests: per-kind counts, a waiter naming its holders, a linked walk cancelled by the volume token and by
    the caller's.
  - One test per tricky worker, with a gated read seam or a parked task, asserting the volume stays held after the
    manager dropped and releases when the worker exits: an abandoned walker worker, the live loop past its drain
    timeout, the late live-loop spawn (never spawned), a search cover walk (cancelled by the stop), a verifier task. ❌
    No sleeps (`cmdr_fs::testing::wait_until`).
  - The media gate refuses a `LocalExternal` id before any read.
  - `pnpm check`; `pnpm check index-crate-isolation` shows no ceiling moved.
  - **Must not change**: `state::release::tests::*` (move with the module),
    `state::tests::a_removable_stop_waits_for_the_start_it_cancelled`, `cover::cold_drive_tests::removals`,
    `event_stream_tests`, `stress_tests_lifecycle`,
    `eject::tests::an_index_still_letting_go_of_the_drive_never_meets_the_unmount`,
    `deadlines::tests::a_stuck_index_stop_never_reaches_the_unmount_and_the_flight_lands`.
- **DONE**: no drive-reading spawn site takes a bare `CancellationToken`; tests and checks green.
- **Docs**: `crates/cmdr-index/src/indexing/CLAUDE.md` (leaves), `lifecycle/DETAILS.md` § "When a volume has been let
  go", `transports/DETAILS.md` § "The drain is cooperative", `cover/DETAILS.md` (the linked cancel),
  `scanner/DETAILS.md` (workers hold the volume).
- **Size**: about 550 lines.

### M4: the unmount approver and resume

- **Scope**: `volumes/unmount_approver/{mod.rs, ask.rs, resume.rs, private_symbols.rs}`, `volumes/disk_units.rs`,
  `file_system/volume/drive_release.rs`; `objc2-disk-arbitration` (features `DADisk`, `DADissenter`, `DASession`,
  `dispatch2`) and `dispatch2` as direct dependencies per `docs/guides/add-rust-dependency.md`; removal of the
  `WillUnmount` observer and handler; the install in `lib.rs`.
- **Intentions**:
  - The ask, budget, dissent, and resume exactly as § "The unmount approver"; `stop_index_blocking` in `eject/mod.rs`
    moves onto `drive_release::release` for its one volume (one stop function).
  - Seams: `install` takes `release` and `resume` functions, so a test injects recorders.
  - Decision 7 is fixed by construction: the approver's resume replaces the handler that never resumed; a regression
    test pins it.
  - The ask for a disk with no Cmdr volume returns in milliseconds and takes no lock a stop holds.
- **Landmines**:
  - A test process's approval session is asked about EVERY unmount on the Mac for its lifetime: the test's seams act
    only on the test image's BSD units and approve everything else at once, and the session is unscheduled on drop.
  - Verify that no idle callback is delivered between an ask's answer and that unmount's completion (a second approver
    session in the test holds its answer 2 s after ours). If one is, resume must also require the root to be listed
    after the next idle and not merely at the first; record the finding in § "Spike results".
  - A `DADiskRef` from another session never calls back; retain the ask's disk only for description reads.
  - `index_host::index()` needs `install(&AppHandle)` first; install the approver after it.
  - `volume/CLAUDE.md` is at 599 words.
  - Hardened runtime: `dlsym` of a system symbol is expected to work in a signed build; the checkpoint's manual QA
    smoke-tests a signed release build.
- **Test plan**:
  - Pure tables: `ask.rs` (nothing to stop → approve; released → approve; still releasing → dissent; busy → dissent
    after the stop; the first ask of a group stops every sibling, the second approves at once), `resume.rs` (path set or
    gone × was indexing × intent says run × ejecting set × owner), `drive_release` (budget from one instant, detached
    continuation, `resume` filters).
  - Real images (lane): `diskutil unmount` of an idle indexed-looking volume (recording seam) → asked while still
    listed, unmounted, no resume; a held file → refused, resume seam called once after idle; `diskutil unmountDisk` on
    the two-partition image → first ask released both, second answered under 50 ms; a release seam that overruns →
    dissent at 7 s, `diskutil unmount` refused; `hdiutil detach -force` with that seam → detached anyway (force ignores
    dissent).
  - `pnpm check`, `pnpm check disk-images`, then `pnpm check --include-slow`.
  - **Must not change**: `volumes::watcher::tests::*` except the removed will-unmount wiring (mount/unmount registry
    tests, `stopping_a_registered_local_external_index_removes_its_instance`,
    `unmount_cleanup_leaves_a_non_local_external_index_alone`, `end_to_end_post_notification_runs_handler`);
    `eject::tests::*`.
- **DONE**: the observer is gone, the approver runs in the app, lane and checks green.
- **Docs**: `volumes/CLAUDE.md` and `volumes/DETAILS.md` § "Key decisions" (DiskArbitration now owns the pre-unmount
  hook), `transports/DETAILS.md` § "Unmount/eject lifecycle" (the approver replaces the racy hook), `volume/DETAILS.md`
  § "Eject" (why the approver answers Cmdr's eject at once), `docs/architecture.md` (the `volumes/` map line).
- **Size**: about 750 lines.

### M5a: a vanished drive, index side

- **Scope**: `unmount_approver/causes.rs` plus the eject-approval and appeared registrations; the presence seam; the
  delete, completion, and aggregation gates; the suspect flag and invalidation; `IndexEvent::IndexNeedsFreshScan` and
  its app mapping and notice (translated in every catalog).
- **Intentions**: as § "A vanished drive"; audit every delete-from-absence and completion site again from the code (the
  inventory in § "Code map" is the starting list, not the proof); M1's vanish pin flips.
- **Landmines**:
  - ❌ `clear_index` for invalidation (it deletes the intent markers with the database).
  - The presence check is one table read per batch or per completion, never per entry.
  - The live loop's removal events on a healthy drive must keep deleting (`ENOENT` while listed is a real delete).
  - An `Unasked` unmount of a volume Cmdr's own eject is running can't happen (Cmdr's eject asks), but a raw `umount`
    racing it can: the flight's resume must find the volume unlisted and do nothing.
  - `IndexEvent` variants carry no new type (a carried type spends a root promise).
- **Test plan**:
  - Pure: `causes.rs` table (asked unmount, raw `umount`, eject then disappear, disappear with no eject, re-appear).
  - Crate, deterministic with `FakeVolumeProvider::mark_unmounted`: each delete site writes nothing when unmounted or on
    a non-`NotFound` error; an incomplete listing deletes nothing; completion doesn't stamp; a batch followed by
    unmounted marks suspect, and the stop invalidates while keeping `user_enabled`.
  - Lane: M1's vanish pin flipped (no stamp, no rows lost); a new pin detaching `-force` while a live watcher runs and
    files are removed.
  - `pnpm check`, `pnpm check disk-images`.
  - **Must not change**: `scan_completion` tests (`scan_failure_is_vanished_volume`), `phases::tests`, the reconcile and
    verifier suites, `integration_tests.rs`, `cover::cold_drive_tests::*`.
- **DONE**: no worker writes a deletion or completion for a gone volume; the notice fires for a suspect index only.
- **Docs**: `reconcile/DETAILS.md` (the delete gates), `lifecycle/DETAILS.md` (completion gates, invalidation),
  `host/DETAILS.md` (`is_mounted`), `events/DETAILS.md` (the variant), `transports/DETAILS.md` (a vanished drive),
  `volumes/DETAILS.md` (causes).
- **Size**: about 550 lines.

### M5b: a vanished drive, transfers and temps

- **Scope**: presence-aware `classify_io_error`, `DeviceDisconnected { path, side }`, `progress_at_stop` on
  `WriteErrorEvent`, the frontend copy in `transfer-error-messages.ts` (translated), and the five temp fixes.
- **Intentions**: as § "A vanished drive"; read `cross_fs.rs` and confirm no source is removed while its copy still sits
  in staging before the staging sweep ships; name each operation's "what's left where" from typed facts only.
- **Landmines**:
  - A `NotFound` from a gone mount isn't a gone file: every "already gone" decision asks the mount table.
  - ❌ Never remove an aside whose destination is missing.
  - `WriteErrorEvent` is wire: `pnpm bindings:regen`, and the transfer copy's key family decides whether counts are ICU
    plurals.
  - The ledger's log line format changes for volume-homed local temps: a record written by an older build must still
    replay.
- **Test plan**:
  - Unit: `classify_io_error` with a mounted and an unmounted side for `ENOENT`, `EIO`, `ENXIO`, `EBADF`; `discard_temp`
    keeps the record when the mount is gone; a local temp on a non-root mount defers at launch; the aside sweep's
    restore and remove arms; the staging dir registers and sweeps; an in-session re-plug sweeps.
  - Frontend: `transfer-error-messages.test.ts` cases per direction and op.
  - Lane: copy onto an HFS+ image, `detach -force` mid-file, assert `DeviceDisconnected` with progress, the Mac source
    untouched, the record pending; re-attach the same image and assert the sweep removed the partial.
  - `pnpm check`, `pnpm check disk-images`, then `pnpm check --include-slow`.
  - **Must not change**: `in_flight_temps_tests.rs`, `overwrite_tests.rs`, `transfer/volume/copy_crashsafe_tests.rs`,
    `transfer-error-messages.parity.test.ts`, the `staging.rs` tests.
- **DONE**: a pulled transfer reports how far it got; nothing it left on the drive is forgotten.
- **Docs**: `write_operations/DETAILS.md` (the ledger and `discard_temp`), `write_operations/transfer/DETAILS.md`,
  `file_system/DETAILS.md` § "Hiding transient scratch", `crates/cmdr-fs/DETAILS.md` (staging names),
  `apps/desktop/src/lib/file-operations/transfer/DETAILS.md`.
- **Size**: about 650 lines.

### M6: disk resolution, per-disk flights, and sibling safety

- **Scope**: `eject/disk_target.rs`, `eject/disk_flight.rs`, `EjectStep::DiskResolve`, `disk: Option<&DiskTarget>` on
  `Teardown::Tool`, the detached settle after `TimedOut`, and `objc2-io-kit` as a direct dependency.
- **Intentions**: the order in § "Cmdr's own eject", each round-3 should-fix as its own tested behavior:
  1. resume after a timed-out request settles with siblings still mounted, and after a partial
     `NotResponding { IndexStop }`;
  2. resume re-checks intent and the master switch (`drive_release::resume`);
  3. a NULL resolution answers `Ok` when unmounted and falls back to `disk: None` when still listed;
  4. fail closed: every captured sibling path gone AND a fresh `mounted_volumes_on` empty before success;
  5. a sibling's eject joins the running disk flight before `is_already_unmounted`.
- **Landmines**:
  - Resolution runs before the join, so two callers may each resolve (read-only, accepted).
  - Retry aimed at the original path after a partial unmount reads as done: aim at a still-listed captured path.
  - `volumes/disk_image.rs` stays on raw FFI (the DA teardown swap is deferred).
  - Register test volumes in the global `VolumeManager` under unique ids and remove them (`volume/DETAILS.md` § "Test
    isolation for the global `VolumeManager`").
- **Test plan**:
  - Pure: sibling selection over a fake target and registry (`volume.root()` compared), the busy gate, the join map, the
    resume matrix (refused, timed out then settled mounted or gone, not responding with some released, success × was
    indexing × still mounted × intent), the NULL-resolution arms, the fail-closed `still_mounted`.
  - Lane: M1's two sibling pins flipped to `UnmountRefused` with a resume; eject of a two-volume container keys the
    physical whole and stops both before the teardown.
  - `pnpm check`, `pnpm check disk-images`.
  - **Must not change**: `in_flight::tests::*`, `unmount_tool::tests::*`, `eject::tests::*`, M4's approver tests.
- **DONE**: siblings are joined, gated, stopped, checked, and resumed; checks and lane green.
- **Docs**: `volume/DETAILS.md` § "Eject" (resolution, disk flights, siblings, `DiskResolve`, resume; delete the "Known
  gap" sentence), `volume/CLAUDE.md` (the eject bullet, no longer), `transports/DETAILS.md` § "Unmount/eject lifecycle"
  (siblings), `apps/desktop/src-tauri/src/commands/DETAILS.md` if it describes eject.
- **Size**: about 650 lines.

### M7: holder scan and wire type

- **Scope**: `eject/holders/{mod.rs, scan.rs}`, `merge`, `UnmountRefused { holders, detail }`, `VolumeHolder`,
  `Display`, the MCP `data`, bindings, and a frontend stub keeping today's `unmountRefused` copy.
- **Intentions**: scan once after the final refusal, disks and SMB, still-listed paths only, one abandonable thread,
  injected budget; Linux empty.
- **Landmines**:
  - Scanning inside the retry loop multiplies up to 9.4 s by the attempts.
  - An SMB `stat` can hang: the budget detaches the thread; nothing awaits it.
  - `// SAFETY:` on the FFI; `specta` doc comments land in `bindings.ts`.
- **Test plan**:
  - Pure: `merge` (dedupe, ESRCH), the budget on a paused clock.
  - An unignored macOS test: a child holds a temp FILE, and `proc_listpidspath` on that file path WITHOUT
    `PATH_IS_VOLUME` returns its PID, well inside the 8 s cap.
  - Lane: M1's held-file pin asserts the holder PID.
  - `pnpm check`, `pnpm check disk-images`, then `pnpm check --include-slow`.
  - **Must not change**: `eject::tests::eject_error_crosses_the_wire_as_a_tagged_value` (updated for the new field
    only), the `in_flight` and `unmount_tool` tests that build `UnmountRefused`.
- **DONE**: MCP replies carry holders in `data`, bindings regenerated, checks green.
- **Docs**: `volume/DETAILS.md` § "Eject" (holders, capture order, budget), `mcp/DETAILS.md` (the `eject` tool's
  `data`), `docs/guides/error-handling.md` (`detail` beside holders).
- **Size**: about 300 lines.

### M8: holder facts and classification

- **Scope**: `eject/holders/{facts.rs, nested_images.rs}`, `classify`, the responsible-PID `dlsym`, and the Security
  extern for the platform identifier and Info.plist.
- **Intentions**: the facts order in § "Holders"; a `classify` table for `lsd`, Finder, zsh under Warp, the WebKit
  helper with and without the responsible symbol, Google Drive's nil-`NSRunningApplication` responsible app, an
  accessory app, an orphaned `sleep`, a third-party daemon, an executable on the target volume, self, and a descendant
  of self; the nested-image signal picked and recorded in § "Spike results" with an evidence anchor; facts past the
  budget stay `Unclassified`.
- **Landmines**:
  - A Security call against an executable on the volume makes Cmdr the holder.
  - `NSRunningApplication` without an autorelease pool leaks; release every `SecCode` and CF reference at scope end.
  - ❌ Not `csops` (its header isn't in the SDK); ❌ no path-prefix test for "system".
- **Test plan**: pure `classify` over recorded facts; lane: a binary copied onto the image and run from it holds the
  volume, and the refusal names it `Tool`; a nested image stored on the outer image names `DiskImage`; `pnpm check`,
  `pnpm check disk-images`.
  - **Must not change**: M7's tests.
- **DONE**: kinds filled in, lane and checks green.
- **Docs**: `volume/DETAILS.md` § "Eject" (classification, accepted tradeoffs).
- **Size**: about 450 lines.

### M9: eject copy in every catalog, then the release checkpoint

- **Scope**: `wordUnmountRefusal`, `formatConjunctionList`, the six approved keys with `@key` descriptions, and
  translations per `docs/guides/i18n-translation.md` (ten full locales; `en-GB`/`en-AU` only where wording differs).
- **Intentions**: the precedence in § "Copy drafts"; each `@key` names the toast surface, says it follows "Couldn't
  eject {volumeName}: ", explains the tokens, marks Cmdr and macOS as names, and for `otherApps` says it's the last list
  item and must read naturally after the language's "and".
- **Landmines**: a raw family fails `desktop-i18n-icu` on a doubled apostrophe; tokens stay verbatim for
  `desktop-i18n-parity`; a key with no call site fails `desktop-message-keys-unused`; run `pnpm intl:keys` and
  `node apps/desktop/scripts/sync-locale-keys.ts`; `apps/desktop/src/lib/intl/CLAUDE.md` is near its limit, so fold the
  list formatter into the existing number-format bullet.
- **Test plan**: `eject-error-messages.test.ts` (one, two, three, four, and six apps; `DiskImage`; `Cmdr`; `System`;
  mixed; empty; only `Unclassified`); a list-formatter test with a pinned locale; `pnpm check svelte` plus
  `desktop-i18n-icu`, `desktop-i18n-parity`, `desktop-i18n-coverage`, `desktop-i18n-term-consistency`,
  `desktop-message-keys-fresh`, `desktop-message-keys-unused`.
  - **Must not change**: the existing `eject-error-messages.test.ts` cases.
- **DONE**: every locale carries the keys, checks green.
- **Docs**: `apps/desktop/src/lib/file-explorer/navigation/DETAILS.md` § "A refusal speaks the catalog", its `CLAUDE.md`
  line on `wordEjectRefusal`, `apps/desktop/src/lib/intl/CLAUDE.md`.
- **Size**: about 150 lines of code plus six keys across the catalogs.

### Release checkpoint

- **Close-out sweep**: a conformance pass against § "Invariants"; read every commit body and every touched `CLAUDE.md`;
  `pnpm check docs-dead-links docs-reachable docs-link-text docs-section-refs claude-md-length resident-doc-budget oxfmt`;
  `pnpm check --include-slow`.
- **Manual QA list for David** (none of it automated): an APFS USB stick and an exFAT one ejected from Finder while Cmdr
  indexes them; an SD card; a two-partition drive; a DMG opened from Finder; an app launched from its DMG; Preview
  holding a file; a terminal `cd`'d into the drive; a copy to a stick with the cable pulled mid-file, then re-plugged; a
  signed release build.
- **Then**: FF-merge to David's local `main`; move this plan to "Shipped, kept for review" in `docs/specs/index.md`.

## Rollback

- Each milestone lands as its own commits and reverts with `git revert`. Nothing persistent changes shape except the
  temp ledger's volume-homed local records (M5b reads both shapes) and `WriteErrorEvent` (wire, regenerated).
- Reverting M4 brings back the `WillUnmount` handler and keeps M3's holds.
- M5a and M5b revert independently. M6 reverts alone as long as M7's scan falls back to this volume's root when `disk`
  is `None`. M7, M8, and M9 revert together (wire and keys).

## Invariants

The conformance register for the checkpoint.

1. No classification by any string.
2. Before any DA-mediated unmount of a volume Cmdr indexes, the approver's ask stops every volume on that whole unit, or
   dissents.
3. `Released` means no Cmdr index worker holds the volume; every drive-reading spawn takes `VolumeWork`.
4. A stopped index resumes only through `drive_release::resume`: still listed, was indexing, intent says run, not
   ejected by another owner.
5. A vanished (`Unasked` or `Pulled`) volume is never resumed.
6. No row is deleted from an incomplete listing, a non-`NotFound` error, or an unlisted volume; no completion stamp or
   final aggregation runs for an unlisted volume.
7. A suspect index is invalidated with its intent markers kept, and announced once.
8. A temp or staging dir on a drive stays recorded until its removal is observed on a listed mount; an aside is
   restored, never dropped, when its destination is missing.
9. Every registered volume on the physical disk passes the busy gate and has its index stopped inside one deadline
   before Cmdr's own teardown; a sibling's eject joins the disk flight.
10. Success needs every captured sibling path gone and a fresh `mounted_volumes_on` empty.
11. Nothing unmounts after resolution, the ejectability check, or an index stop fails or stalls.
12. Holders are captured once per eject, after the last attempt, over still-listed paths, within the injected budget.
13. No code-signing query runs against a process whose executable is on the target volume.
14. Private symbols come from `dlsym`; their absence degrades, never fails.
15. Every user-facing word is catalog copy in all 13 catalogs.
16. Tests and probes: synthetic APFS/HFS+ only, plist-verified identity before every mutating call, SIGKILL-guarded.
17. SMB keeps `diskutil unmount` and Linux keeps `umount`; neither gets an approver.

## Deferred

- **The DA teardown swap** (Whole-unmount each synthesized container, then the physical whole, then `DADiskEject`, with
  typed statuses and DA's root-visible dissenter PID). The full design is in git:
  `docs/specs/eject-diskarbitration-plan.md` at `15292aa2b`, § "The DA teardown (M5)" and § "Statuses". **Revisit when**
  the eject `warn` lines show refusals with no nameable holder (root-owned or other-uid holders) often enough to matter,
  or a report shows `diskutil` answering a partial unmount this plan's fail-closed check can't classify.
- **"Unmounted but not powered down"** stays a silent `Ok` with an `info` line, today's `diskutil` parity. **Revisit
  when** a person reports a drive still powered after Cmdr's eject, or the DA teardown lands (which can see it).
- **Unverified and left alone**: whether AppKit's own DA session filters by disk (§ "Spike results" 7), and a MOUNTED
  volume vanishing under a real pulled cable (§ "Spike results" 5). The checkpoint's manual QA covers the second.

## Spike results

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
  with `diskutil mount -mountOptions nobrowse`. Real-image tests can cover the sibling-partition case.

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
- **Refusals took 0.13–0.2 s at load about 2**, far below the 20–28 s outliers under load in § "Evidence the design
  rests on".

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

#### 8. Holder facts checked here

- **`dlsym(RTLD_DEFAULT, "responsibility_get_pid_responsible_for_pid")`** resolves in an unsandboxed, non-root, unsigned
  Swift binary, at 2–8 µs per call:
  - `com.apple.WebKit.WebContent` (parent launchd) → BetterDisplay (accessory), Google Drive (`NSRunningApplication` nil
    for the responsible PID), CleanShot X (accessory), and Cmdr.app (regular).
  - `com.apple.WebKit.GPU` → iStat Menus Helper (accessory); a Chrome renderer → Google Chrome (regular).
  - Standalone agents (iStat Menus Helper, `ViewBridgeAuxiliary`, `DiskUnmountWatcher`) → themselves.
- **`DADissenterGetProcessID` through `dlsym`** resolves in a binary linking DA.
- **`lstat` devices**: on the APFS image, `st_dev`, the mount point's `f_fsid.val[0]`, and its `getfsstat` entry all
  read 16777241. On the boot volume, `/`'s `st_dev` (16777234) differs from its `f_fsid.val[0]` (16777235, the sealed
  system snapshot), so compare against the mount point's own `stat().st_dev`, not `f_fsid`.
- **Security calls off the volume** (`SecCodeCopyGuestWithAttributes`, then `SecCodeCopySigningInformation`): Finder 4
  plus 3 ms; both `lsd` processes and a WebContent helper 1 ms or less each (platform identifier 26); iStat Menus Helper
  2 plus 43 ms (no platform identifier).
- **`hdiutil info -plist`** carries the typed per-image keys `hdid-pid` and `diskimages2` besides `image-path`.
- **BSD units repeat at once**: a detached image's `disk8` and `disk9` went to the next attach, and image A came back as
  `disk5` and `disk6` both times it was re-attached.
- **Not run** (new risk, or M8 code): the nested-image signal, `diskimagesiod` holding an outer volume, and the
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
