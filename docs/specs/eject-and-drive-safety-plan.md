# Eject and drive safety

**Problem.** Cmdr doesn't reliably let go of a removable drive before it unmounts, doesn't survive a drive that
vanishes, and can't say what holds a drive it couldn't eject.

- **Letting go is racy.** Cmdr's own eject stops only THIS volume's index. An eject from Finder, `diskutil`, or another
  app reaches Cmdr through `NSWorkspaceWillUnmountNotification`, whose handler spawns a thread and returns, so the
  unmount races the stop (`apps/desktop/src-tauri/src/volumes/watcher.rs:257`). Nothing resumes an index that handler
  stopped when the unmount is then refused. An indexed exFAT drive unmounting under a live FSEvents stream is the FSKit
  wedge that kernel-panicked a Mac on 2026-07-15.
- **"Released" doesn't mean nothing reads the drive.** The per-volume hold drops with the index manager, but 11 kinds of
  worker keep reading after it drops (§ "Code map").
- **A vanished drive corrupts what Cmdr knows.** A child whose stat fails drops out of a listing and its row is deleted;
  a live event whose stat fails for any reason deletes its row; a rebuild deletes a subtree before reading it; a scan
  whose drive left mid-walk stamps itself complete, and its `Abandoned` marks make the next start stamp completion over
  ground nobody walked. A move from the Mac to a drive deletes the Mac sources after a flush that syncs nothing, and a
  transfer ends with a generic error, no progress facts, and a partial file on the drive nothing ever sweeps.
- **A refused eject names nobody**, and a disk with two partitions or APFS volumes can read "ejected" while a sibling
  stays mounted and powered.

**Outcome.**

- Cmdr answers DiskArbitration's unmount approval for every DA-mediated unmount, whoever starts it: it lets go of every
  volume of the disk inside the ask's window, dissents when it can't in time, and resumes what it stopped when the
  unmount doesn't happen, without ever racing the next request.
- "Released" means no Cmdr worker reads the drive; a worker stuck in a read keeps the drive mounted, the log names it,
  and a stuck worker on a drive that's already gone doesn't block that drive's next life.
- A drive that vanishes (pulled cable, raw `umount`) stops every worker quietly: no row deleted from a failed read, no
  index or branch marked complete. An index that may have lost rows is marked for a rebuild on disk and announced; a
  transfer says how far it got and what's where; Mac sources are deleted only after the destination is durable and still
  there; leftovers on the drive are swept when it returns, and the user's originals are never among them.
- Cmdr's own eject works per physical disk: sibling ejects join one flight, every sibling is gated and stopped first, a
  sibling that stays mounted is a refusal, and a refusal or timeout resumes what was stopped.
- A refusal names its holders: an app, several apps, a disk image, Cmdr itself, or macOS.

**Status.** Planned 2026-09-14, not started; adversarial review folded in 2026-09-14. It combines the earlier
DiskArbitration eject plan (review rounds 1–3 and the approval-hook spike) with the drive-safety decisions below.

- **Landed prerequisites**: the refusal retry (`unmount_tool::settle_with_retries`), the `NotEjectable` preflight, the
  eject deadlines, `TOOL_TIMEOUT` at 30 s, and the index-stop wait (`Index::stop_removable_volume` answers
  `RemovableStop`, waiting on `VolumeHold`).

## Loud rules

- ❌ **Never a physical disk, and never a FAT, exFAT, or MS-DOS image, in any test, probe, or spike this plan adds or
  runs.** Synthetic APFS and HFS+ images only. The existing FAT/exFAT tests in
  `crates/cmdr-index/src/indexing/tests/external_drive_fixture.rs` stay hand-run and out of every lane this plan adds.
- ❌ **Every `hdiutil` and `diskutil` call a test makes goes through the guarded runner**: a SIGKILL deadline, a
  machine-wide lock, and, before every mutating verb, an identity check that `hdiutil info -plist` still maps the node
  to OUR `image-path`. DA reuses BSD unit numbers at once, so a stored node can become someone's Time Machine drive. The
  runner's allowed verbs are listed in M1; production code under test (`diskutil eject`) runs through the same runner by
  way of the `run_tool` closure, never unguarded. `hdiutil detach -force` is allowed only on a non-nested image the test
  attached; detach nested images inner first, and never force an outer image.
- ❌ **The approver's ask path touches no filesystem, takes no SQLite connection, and never unwinds across FFI.** An ask
  for a disk with no Cmdr work answers at once, unless it's queued behind another ask on the session's serial queue,
  where it answers within the chain's shared deadline (§ "The unmount approver"). DA ignores a dissent under force, and
  a session DA timed out gets no asks at all until its next callback delivery: those are the real bounds, and the design
  keeps every ask inside DA's 10 s window so the second never happens.
- ❌ **No classification by any string**: `diskutil` stderr, a DA status string, a process name, or a path prefix.
  Decide on typed variants, errnos, DA status codes, PIDs, activation policies, signing attributes, BSD names, device
  IDs, and mount-table entries (`error-string-match`, `cmdr/no-error-string-match`).
- ❌ **`index-crate-isolation` ceilings move only with a stated reason** in the commit and in
  `crates/cmdr-index/src/indexing/handle/DETAILS.md`. Measured 2026-09-14: 53 of 53 root promises, 40 of 40 `Index`
  methods, 17 of 17 public modules, 155 of 156 items. This design spends none; a milestone that finds it must, says why.
- ❌ **Nothing unmounts after a pre-unmount step fails or stalls** in Cmdr's own eject: resolution, the ejectability
  check, and the index stop answer `NotResponding { step }` and stop there.
- ❌ **A worker stuck in a read on a MOUNTED drive keeps it mounted** (`StillReleasing`, a dissent, `NotResponding`).
  Never unmount under it: that's the wedge-prone case.
- ❌ **Every index start after a Cmdr-made stop goes through `drive_release`'s gate** (§ "One release, one resume"), and
  so does the user's per-drive disable. ❌ Never a bare `Index::start_volume` from new code: it writes `user_enabled`
  and deletes `user_disabled` (`store/connection.rs:273-284`).
- ❌ **On a local-scanner volume (`IndexVolumeKind::uses_local_scanner()`), a row is deleted only on
  `NotFound`/`ENOTDIR` from a complete observation of a volume still in the mount table**; no completion claim is
  written for a volume that isn't. SMB, MTP, and ADB deletes come from their own protocols and keep today's behavior.
- ❌ **Invalidating an index never calls `clear_index`** (it deletes the database, and the per-drive intent markers live
  in it, `lifecycle/master.rs:113-121`). The rebuild is the persisted `index_needs_rebuild` marker routed to
  `RebuildThenCoverInPhases`.
- ❌ **Mac sources are deleted only after a flush that returned `Ok` and a destination root still in the mount table.**
- ❌ **A sweep never removes an aside unless the destination is a regular file of exactly the recorded size**, never
  removes a non-empty staging directory, and restores only through `rename_no_replace`.
- ❌ **Private symbols come from `dlsym` only**: `DARegisterIdleCallback` and
  `responsibility_get_pid_responsible_for_pid`. A missing symbol degrades, never fails to link.
- ❌ **Holder facts never run a code-signing query against a process whose executable lives on the target volume**, and
  holders are never scanned on a path that left the mount table or whose root device changed during the scan.
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
   dispatch queue, replacing the `WillUnmount` handler (kept only as the fallback when the approver can't install). It
   does the stop work on every ask; dissent is the non-force fallback. It resumes after DA's idle callback when the
   volume is still mounted. Cmdr's own eject keeps its explicit pre-stop, so the approver answers it at once. A raw
   `umount` bypasses DA and is handled like a pulled drive.
2. **Every background worker that reads the drive carries a labelled share of `VolumeHold`** beside its cancel signal.
   The hold moves to a leaf module the scanner, reconcile, and watch code may import.
3. **A vanished drive is handled gracefully and explained**: detection, quiet workers, impact notices only where
   something needs doing, temp sweeps on return, and the two temp-cleanup bugs fixed.
4. **Partitions and holders**: real disk-image tests with the shared guarded runner, disk resolution and per-disk
   flights with the round-3 should-fixes, a holder scan, holder facts, and copy in every catalog.
5. **Holder details**: responsible-PID attribution through `dlsym` with a fallback and no regular-policy requirement; a
   shell names its terminal app; SMB refusals scan holders too; the eject copy in § "Copy drafts" is approved with names
   unquoted; no Force Eject; an opt-in macOS disk-image lane under `--include-slow`.
6. **The approver dissents while a write op is busy on the disk** (non-force requests honor it; under force DA ignores
   it). **The existing FAT/exFAT image tests stay hand-run**; the new work adds APFS and HFS+ only.
7. **Deferred**: the DA teardown swap, and "unmounted but not powered down" stays a silent success (§ "Deferred").
8. **Fix**: nothing resumes an index `handle_volume_will_unmount` stopped when the unmount is refused.

## Evidence the design rests on

Unless marked otherwise: verified on macOS 26.6.2 (25G83), unsandboxed uid 501, with throwaway probes against 50–60 MB
APFS DMGs attached `-nobrowse`, 2026-09-12 (review rounds 1–3). Apple sources: DiskArbitration-535.0.10, xnu-12377.1.9.
The approval-hook spike (2026-09-14) is § "Spike results" at the end, and it wins where they differ.

### DiskArbitration: statuses, timing, and the unmount flow

- **Busy is `unix_err(EBUSY)`, `0x0000C010`**. The daemon rewrites any `unmount(2)` failure to EBUSY
  (`diskarbitrationd/DARequest.c:1525`). `kDAReturnBusy` (`0xF8DA0002`) covers `/`, the Data volume, or an approval
  dissent.
- **A refusal is slow because of the daemon's holder scan.** After EBUSY, `diskarbitrationd` runs a root
  `proc_listpidspath(PROC_ALL_PIDS, PATH_IS_VOLUME)` before it answers (`DARequest.c:1534→1707`), for `diskutil` (via
  `storagekitd`) and the DA API alike. At load 2.2–2.5: `diskutil eject` of a held APFS image 20.4, 1.8, 0.5, 27.8, and
  0.6 s; DA container Whole 5.1, 1.9, 0.45, 0.42 s; HFS+ `diskutil` 1.15 and 0.50 s. At load about 2 on 2026-09-14
  refusals took 0.13–0.2 s.
- **A container Whole unmount reaches every volume in the container, and partial unmounts are real**: two-volume APFS
  image, file held on volume B, A unmounted, B stayed mounted.
- **Whole links subrequests by BSD unit** (`DAQueue.c:974`), so `DADiskUnmount(physical, Whole)` doesn't reach the APFS
  volumes of its synthesized container.
- **`DADiskCreateFromVolumePath` runs `statfs` then `realpath`** (`DiskArbitration/DADisk.c:413-421`), so it hangs on a
  DMG whose backing file lives on a hung SMB share. It returns NULL for a path that's no longer a mount.
- **DA doesn't track SMB**: `diskutil info /Volumes/naspi` prints "Could not find disk".
- **IOKit ancestry of an APFS volume** (`ioreg -p IOService -t`): `AppleAPFSVolume` → `AppleAPFSContainer` (the
  synthesized whole disk) → `AppleAPFSContainerScheme` → `IOMedia` (the physical store partition) → the physical whole.

The following were verified by reading DiskArbitration-535.0.10 on 2026-09-14:

- **Every approval callback has its own 10 s timer, started when the daemon queues it to the session**
  (`diskarbitrationd/DAQueue.c:627-629`, checked at `:178-180`), not when the client runs it. The daemon queues a second
  callback to a session whose first is unanswered (`DASession.c:312-314`), and the client runs its queue one callback at
  a time (`DiskArbitration/DASession.c:242,257-277`), so a callback's timer runs while the client is still busy with the
  one before it.
- **A timed-out answer counts as approval** (`DAQueue.c:115-121`): DA unmounts.
- **Requests on different disks are in the approval stage at once**: serialization is per disk
  (`kDADiskStateCommandActive`, `DARequest.c:1396,1410,1914`), and a Whole request's per-volume subrequests go out in
  one pass (`DAQueue.c:1000,1025`). Writable media first run `__DARequestUnmountTickle` on a background thread
  (`DARequest.c:1400-1404`), so sibling asks can arrive a few hundred ms apart with their own timers (the 190 ms in §
  "Spike results" 1).
- **A timed-out session is skipped for unmount, eject, and mount approvals, peeks, and claim releases**
  (`DAQueue.c:548-550,604-609`), which DA treats as approval, until the client copies its callback queue
  (`DAServer.c:2147`). Appeared, disappeared, description-changed, and idle callbacks are still queued
  (`DAQueue.c:470, 488-499,663-715,727-734`), and their delivery is what clears the flag.
- **A disk from a callback carries a frozen description** (`DiskArbitration/DiskArbitration.c:477`,
  `DADisk.c:184, 241-245`; nothing ever refreshes it). A fresh `DADiskCreateFromBSDName` has no cached description, so
  `DADiskCopyDescription` on it is a MIG fetch of the daemon's current one (`DADisk.c:100,253`), and NULL once the disk
  left the daemon's list (`DAServer.c:139-158`).
- **Idle never fires between an approval answer and that unmount's completion** (the request stays listed through the
  approval, `DAStage.c:341-343,439-445`; `CommandActive` covers the unmount, `DARequest.c:1442,1592`,
  `DAStage.c:211,338`), **but it does fire between two separate requests**: once one request completes and the stage
  pass is quiet, idle is queued before the tool's next request exists (`DARequest.c:1587-1594`, `DAStage.c:487-500`). So
  `diskutil eject`'s unmount, eject-container, eject-physical sequence has idles in between.
- **Idle is queued on the same per-session queue, in order**, at most once per burst of callbacks to that session
  (`DAQueue.c:63-75,727-734`, `DASession.c:310`), and once at registration if the daemon is already idle
  (`DAServer.c:2733-2747`).
- **A disk that disappears while mounted**: the daemon settles its pending answers as approvals, fails its queued
  requests (`DAServer.c:1423`, `DAQueue.c:785-813`), sends disappeared (`DAServer.c:1427`), then issues its own
  `DADiskUnmount(disk, Force)` (`:1432-1434`), which skips approval because the disk is a zombie by then
  (`DARequest.c:1388-1391`). No eject approval and no description change is sent for it (`DAQueue.c:665`). An approval
  already queued before the pull can still reach the client; its answer is dropped.
- **Bindings**: `objc2-disk-arbitration` 0.3.2 (published 2025-10-04; `madsmtm/objc2` not archived, 1,032 stars, pushed
  2026-09-12) binds `DASessionSetDispatchQueue`, `DARegisterDiskUnmountApprovalCallback` (callback returns
  `*const DADissenter`), `DARegisterDiskEjectApprovalCallback`, `DARegisterDiskAppearedCallback`,
  `DARegisterDiskDisappearedCallback`, `DARegisterDiskDescriptionChangedCallback`, `DADissenterCreate`,
  `DADiskCreateFromBSDName`, and `DADiskCopyDescription` (verified by reading the published crate, 2026-09-14). Not
  bound: the private `void DARegisterIdleCallback(DASessionRef, void (*)(void *context), void *context)`
  (`DiskArbitrationPrivate.h:329,331`). `dispatch2` 0.3.1 (2026-02-26) and `objc2-io-kit` 0.3.2 are in `Cargo.lock`
  transitively. Re-check versions and the 3-day window on the day of adding.

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

Verified against this branch at `0bac2b015` on 2026-09-14, with `codegraph` and by reading the lines. Index paths are
under `crates/cmdr-index/src/indexing/`, write-operation paths under
`apps/desktop/src-tauri/src/file_system/write_operations/`.

### Eject and unmount hooks

- `apps/desktop/src-tauri/src/file_system/volume/eject/mod.rs`:
  - `eject` (`:237`) → `in_flight::join_or_start(volume_id)` (`in_flight.rs:67`, one flight per volume id, one `Landing`
    per flight, `:111-128`) → `eject_now` (`:243`): busy gate for THIS volume (`:253`), device provider,
    `is_already_unmounted` (`:274`), `resolve_is_ejectable` under `EJECTABILITY_CHECK_DEADLINE` 5 s, then
    `stop_index_then_unmount` (`:436`) with `stop_index_blocking` (`:469`) under `INDEX_STOP_DEADLINE` 15 s, then
    `run_teardown` (`:362`).
  - `EjectError` IS the wire type (`:120`); `EjectStep::{EjectabilityCheck, IndexStop}` (`:179`).
- `eject/unmount_tool.rs`: `TOOL_TIMEOUT` 30 s (`:25`), `run` (`:94`, `std::process::Command::output`, no kill),
  `within_tool_timeout` drops the join handle on expiry (`:109-113`), `settle` (`:169`), `is_still_mounted` (`:192`,
  over `volumes::mounts::is_mount_point`, `volumes/mounts.rs:81-88`, exact-path membership in `getfsstat(MNT_NOWAIT)`),
  `REFUSAL_RETRY_BACKOFF` (`:204`), `RETRY_BUDGET` 30 s (`:218`), `settle_with_retries(target, run_tool, still_mounted)`
  with both closures (`:245-253`).
- `apps/desktop/src-tauri/src/volumes/watcher.rs`: `WillUnmount` observer (`:74-82`, `:102-107`) →
  `handle_volume_will_unmount` (`:257`) → `stop_local_external_index_off_main` (`:301`). `DidUnmount` →
  `handle_volume_unmounted` (`:174`) removes the root and stops a `LocalExternal` index. Installed at
  `apps/desktop/src-tauri/src/lib.rs:406`. Linux's unmount path never stops an index
  (`apps/desktop/src-tauri/src/volumes_linux/watcher.rs:398-478`).
- `apps/desktop/src-tauri/src/volumes/disk_image.rs:39-76`: the only DA code today, raw `extern "C"`.
- `apps/desktop/src-tauri/src/mcp/executor/eject.rs`: flattens the error into `ToolError::internal(format!(..))`.
- Frontend: `apps/desktop/src/lib/file-explorer/navigation/eject-error-messages.ts`, keys `errors.eject.*` in
  `apps/desktop/src/lib/intl/messages/en/errors.json` (a raw family: `getMessage`, literal tokens, no plurals).
- App-side index IPC: `enable_drive_index` (`apps/desktop/src-tauri/src/commands/indexing.rs:261`) and
  `disable_drive_index` (`:290-291`, `index().disable_volume`).

### The index: holds, starts, and routes

- `lifecycle/state/release.rs`: `VolumeHold` (`:47`), a per-volume count and one condvar, `wait_until_released` (`:98`).
  Depends only on `volume.rs` and `cmdr_fs`. Taken inside `try_reserve_initializing_phase`'s critical section
  (`state/reservation.rs:120-128`), stored as the manager's `_hold` (`lifecycle/manager.rs:112`).
- `state/teardown.rs`: `stop_removable_volume` (`:119`: `NothingToStop` at `:131` when no instance and nothing held);
  `finish_stopping` (`:242-256`: `mgr.shutdown()`, then `start_again` for a start recorded during the drain, while `mgr`
  and its hold are still alive until the function returns); `record_the_disable` (`:320`), the after-drain write through
  its own connection.
- **A start has no instance and no hold until its reservation**: `Index::start_volume` (`handle/mod.rs:207`) records the
  enable marker (`:261`), `start_indexing_for_local_external` (`transports/local_external/index.rs:136-150`) runs a
  mount-facts probe of up to `FS_PROBE_TIMEOUT` 2 s (`:35`, `:103-113`), and `start_indexing_for`
  (`lifecycle/state/startup.rs:231-303`) opens the store and pool before `try_reserve_initializing_phase`. A stop in
  that window answers `NothingToStop`, and nothing records it. No "start in progress" marker exists.
- `IndexManager::shutdown` (`lifecycle/manager.rs:716-754`): cancels the volume token, stops phases without joining,
  drops `scan_handle` without joining, stops the watcher (joins its run loop), waits for the live loop at most 5 s
  (`:741`, then detaches), shuts the writer down (`:751`).
- `lifecycle/master.rs`: `master_enabled()` is an in-memory atomic (`:63-65`); `drive_index_should_run` (`:113-121`)
  opens up to three read connections; `drives_to_resume` (`:175-196`) runs it for every registered volume, and its doc
  (`:169-174`) says the master toggle is deliberately its one caller.
- **Launch routes** (`lifecycle/manager/launch_route.rs:14-27,71-86`): `ReplayTheJournal`, `ScanTheVolume` (completed
  index, or phases off), `RebuildThenCoverInPhases` (rows but no covered-branch set), `CoverInPhases`. The rebuild is
  `PhasedStart::RebuildFirst` (`manager/phased.rs:158-182`): `TruncateData` (keeps `meta`, `writer/entries.rs:757`),
  delete `scan_completed_at` and `home_covered_at`, clear the branch set. Every `start_scan` deletes `scan_completed_at`
  (`manager/start.rs:505-510`). `RescanReason::IncompletePreviousScan` (`events/payload.rs:114-115`) is never emitted.
  No "needs rebuild" marker exists.
- **Quit never stops an index** (`apps/desktop/src-tauri/src/app_lifecycle.rs:30-37,141-161`).
- **Volume ids**: a local volume's id comes from its volume UUID (`crates/cmdr-fs/src/volume/ids.rs:126-139`), so a
  re-plugged drive keeps its id; a volume with no UUID gets a path-derived id (`:150-155`).

### Workers that still read the drive after the hold drops

File:line of the spawn:

1. walker worker threads `index-walk` (`scanner/walker/engine.rs:311`; deliberately never joined, `:104-106`);
2. `index-scanner` (`scanner/mod.rs:567`) and `index-local-reconcile` (`reconcile/local_reconcile.rs:234`);
3. `reconcile-read` threads, abandoned on timeout (`local_reconcile.rs:128`, `:183-190`);
4. the scan completion task: its replay (`lifecycle/scan_completion.rs:366-375`, no token check) and a live loop it can
   spawn after `shutdown` emptied the slot (`:411-431`, `manager.rs:737`);
5. `rescan-subtree` threads (`reconcile/reconciler/rescan/mod.rs:493`);
6. `index-phases` (`lifecycle/phases/mod.rs:317`) and its `index-cover` walk;
7. search cover walks, on the CALLER's token (`lifecycle/cover/mod.rs:261`, `:311`), which teardown never cancels;
8. verifier tasks and their `scan_subtree` walks (`lifecycle/state/scan_control.rs:78,106`,
   `reconcile/verifier.rs:120,249`);
9. the live loop after its 5 s drain timeout;
10. `index-mount-probe` threads (`lifecycle/cover/bootstrap.rs:209`);
11. a media network pass on a `LocalExternal` id in a hand-edited opt-in list: `run_network_pass_blocking`
    (`crates/cmdr-index/src/media_index/scheduler/mod.rs:474-526`) has no kind gate.

Not workers for this plan: the start probe above (no reservation exists; B1's gate covers it); the importance scheduler
reads the DB (its Spotlight sample, `crates/cmdr-index/src/importance/last_used.rs:53`, is believed to use
index-relative paths: M4 verifies); thumbnails (`apps/desktop/src-tauri/src/commands/media_index/thumbnail.rs:38-70`)
are foreground. Nothing in `scanner/`, `reconcile/`, or `watch/` imports `lifecycle::state`; `volume.rs` (`:3-7`, "pure
predicates only") and `metadata.rs` are the shared leaves.

### Index writes from a failed or missing observation

The full inventory (M7 and M8 gate each; `reconciler.rs` is `reconcile/reconciler.rs`):

- **`diff_dir_against_db`** (`reconciler.rs:818-827`): a DB child missing from a listing is deleted. A failed directory
  read deletes nothing (`:1152-1158`), but a child whose entry iteration or stat fails is dropped from the listing
  (`:1408-1413` macOS bulk path, `:1440-1448` portable path) and so deleted. Local callers: `reconcile_subtree`
  (`:1184`) for the `MustScanSubDirs` drain (`reconcile/reconciler/rescan/mod.rs:526`) and cover repair
  (`lifecycle/cover/ground.rs:214`); the full local rescan (`local_reconcile.rs:542`); the stitch
  (`lifecycle/phases/stitch.rs:103`). The trait-scanned caller is `network_scanner/reconcile_scan.rs:287`.
- **The verifier's own diff** (`reconcile/verifier.rs:249-257`: `.flatten()` and a stat `Err(_) => continue` drop
  children; deletes at `:312-324`).
- **`handle_removal`** (`reconciler.rs:1580`: anything but `symlink_metadata().is_ok()` deletes, `:1615-1619`) and
  **`handle_creation_or_modification`** (`:1646-1669`: any stat `Err` deletes). Reached per event from the live loop
  (`:535`), the post-scan replay (`scan_completion.rs:366`), and cold-start replay (`watch/replay.rs:190,622`).
- **Boot-disk cold-start verification** (`watch/event_loop/verification.rs:340-349`): `!Path::exists()` (false on any
  error) deletes each DB child, and the parent's `read_dir` failure is checked only after (`:353-356`). `Local` kind
  only.
- **`ScanRoot::Rebuild`** (`scanner/mod.rs:815-819`): `DeleteDescendantsById` before the walk reads anything. Callers:
  `verifier.rs:457`, `verification.rs:108`.
- **`Abandoned` marks**: any non-permission read error, a timeout, or a pruned dir is marked
  (`scanner/insert_visitor.rs:442-467`) and persisted through `MarkDirsUnreadable` (`scanner/mod.rs:100-115,910`,
  `writer/mod.rs:1484-1498`). `Abandoned` ground isn't frontier (`read/coverage.rs:226-229`,
  `phases/completion.rs:6-9`), so the phase machine's `take_stock` (`completion.rs:46-53`, run after every drain,
  `phases/mod.rs:391,450,453`) stamps completion over it in the same session, and `writer/abandoned_retry.rs:42-46`
  never walks it (its doc still says the phase machine "doesn't exist yet").
  `IndexStore::clear_unreadable_cause(conn, cause)` exists (`store/meta.rs:190`).
- **Not observation-driven, left alone**: a type change seen in a successful listing (`reconciler.rs:762-767`,
  `verifier.rs:367-372`, `writer/entries.rs:141-156`); `MoveEntryV2`'s destination delete (`writer/entries.rs:518-520`);
  every `TruncateData` (`manager/start.rs:533`, `manager/phased.rs:173`, `lifecycle/network_scan.rs:294`); SMB and MTP
  watch deletes resolved against the index (`transports/smb/watch.rs:218`, `transports/mtp/watch.rs:143`).
- **Completion claims** on the full-scan and local-reconcile path: `ComputeAllAggregates` queued by the scanner thread
  (`scanner/mod.rs:596-605`) and by `finish_reconcile` (`reconciler.rs:877-884`) before the completion task decides;
  then only when `was_completed`: `scan_completed_at`, the shallow sweep keys, calibration keys, and `volume_path`
  (`scan_completion.rs:298-346`); not gated: freshness `ScanCompleted` (Fresh, `:388-395`), phase Live, the live loop.
  On the phase path: `stamp_home` (`phases/completion.rs:83-102`, home volume only) and `run_the_completion_sequence`
  (`:128-222`: the stamp, calibration, ledger, sweep keys, freshness, `branches::collapse_to`).
- **Trait-scanned volumes**: MTP roots are `mtp://<device>/<storage>` (`crates/cmdr-mtp/src/volume/mod.rs:98`), never in
  the mount table; `IndexVolumeKind::uses_local_scanner()` is `Local | LocalExternal` (`volume.rs:80-82`).

### Transfers and temps

- The frontend routes every copy, and every move touching a non-root volume, through `copy_between_volumes` /
  `move_between_volumes` (`apps/desktop/src/lib/file-operations/transfer/transfer-dispatch.ts:83-92`, `:162-168`). Both
  take `source_volume_id` + `source_volume` and the same for the destination (`transfer/volume/copy.rs:122-133`,
  `transfer/volume/move.rs:49-60`), derive roots through `local_path()` (`copy.rs:144`, `move.rs:77`), and hand the
  LOCAL engine only absolute paths plus a positional `vec![source, dest]` used for busy tracking (`copy.rs:177-182`).
- **The local cross-FS move** (`transfer/move_op/cross_fs.rs`), used by both Mac → drive and drive → Mac: staging dir
  `destination.join(".cmdr-staging-<op>")` (`:107`, on the destination's filesystem); Phase 2 copies into it
  (`:177-249`, pause gate per file `:183`); Phase 3 renames out (`:290-409`, `rename_onto_free_name` `:402`); commit
  (`:443-456`); `flush_created_destinations` (`:457-468`, returns `()`); Phase 4 deletes sources unconditionally
  (`SourceSweep::plan` `:473-482`); Phase 5 `let _ = fs::remove_dir(&staging_dir)` (`:489`).
- **The flush syncs nothing for a chunked copy**: `durability.rs:70-72` skips paths in `already_synced`, before both the
  data sync and the directory fsync, and every chunked-copied file is marked durable
  (`transfer/copy_strategy.rs:236-242`, `transfer/copy/single_item.rs:563-564`). Mac ↔ USB always copies chunked
  (`copy_strategy.rs:166-172`), and the chunked copy's only sync is `sync_data` on the temp
  (`transfer/chunked_copy.rs:171`); the landing rename and the aside removal get no directory fsync
  (`overwrite.rs:120-165`).
- **The source sweep** (`transfer/move_op/source_sweep.rs`, only the local cross-FS move): `remove_landed_file` treats
  `NotFound` as done (`:304`), `remove_swept_dir` treats any failed stat as gone (`:313-316`), and a top-level source
  that can't be stat'ed is skipped and still counted done (`:238,269`).
- **Errors**: `classify_io_error(e, path)` (`error_classification.rs:25-61`) maps only `ENODEV` to
  `WriteOperationError::DeviceDisconnected { path }` (`types.rs:403-406`); `ENXIO` and `EIO` become `IoError`, `ENOENT`
  `SourceNotFound`. `map_volume_error(context_path, role, e)` knows the `PathRole` but drops it for `DeviceDisconnected`
  (`transfer/volume/transfer_error.rs:251-255,285`). `WriteErrorEvent` (`types/events.rs:158-165`) carries no progress;
  `files_done`/`bytes_done` live in the status cache (`status_cache.rs:70-74`).
- **Asides**: four creators mint `.cmdr-temp-<uuid>` and none registers it:
  - `stage_and_land_file` (`overwrite.rs:108`): the existing destination FILE set aside while a new file lands;
  - `displace_with_directory` (`overwrite.rs:364`, from `transfer/copy/single_item.rs:274`): a blocking FILE set aside
    so a directory can be built at its path, filled leaf by leaf; the failure path keeps it as a recovered sibling
    (`ledger.rs:335-341`, `overwrite.rs:311`);
  - `safe_overwrite_dir` (`overwrite.rs:423-429`, from `single_item.rs:421` and `transfer/move_op/mod.rs:356`): the
    original set aside while the closure materializes the destination;
  - the volume engine's `displace_destination` (`transfer/volume/displaced_destination.rs:49`). `rename_no_replace`
    (`overwrite.rs:202`: `renamex_np(RENAME_EXCL)` on macOS, `renameat2(RENAME_NOREPLACE)` on Linux, a racy fallback
    otherwise) and `recovered_sibling` (`unique_name.rs:243,255-266`, "notes (recovered).txt") exist.
- **The ledger** `in-flight-temps.log` (`in_flight_temps.rs`):
  `#[serde(untagged)] RecordedTemp { Local(PathBuf), OnVolume(VolumeTemp) }` (`:113-134`), lines `+`/`-` then JSON;
  `read_recorded` skips unparsable JSON and ignores unknown op bytes (`:593-616`); the startup sweep truncates the log
  (`:339`), deletes local records once, counts `NotFound` as gone (`:410-429`); `pending: BTreeSet<VolumeTemp>` (`:196`)
  holds volume-homed records only, swept on `VolumeManager::on_volume_arrival` (`volume/manager.rs:135`;
  `in_flight_temps.rs:469-479`), where anything failing `is_one_of_ours` is retired without deleting (`:490-493`).
  `is_one_of_ours` accepts `.cmdr-tmp-` and `.cmdr-temp-` names (`:532-544`, `crates/cmdr-fs/src/staging.rs:79-81`).
  `discard_temp` ignores a failed remove and deregisters (`overwrite.rs:176-179`).
- **Test seams**: `copy_file_using(LocalCopyStrategy::Chunked, ..)` with an observe callback
  (`transfer/copy_strategy.rs:207`, used by `transfer/volume/copy_crashsafe_tests.rs:539-608`); nothing can pause a
  local move mid-file (the chunk loop checks cancel only, `chunked_copy.rs:126,158-160`).
- **Every `DeviceDisconnected` site**: Rust `types.rs:404`, `error_classification.rs:44`, `transfer_error.rs:285`,
  `transfer/volume/merge_case_fold_tests.rs:392`; TS `transfer-error-messages.ts:108,206,439`,
  `apps/desktop/src/lib/ipc/bindings.ts`, `apps/desktop/src/lib/file-explorer/types.ts:653`, the dialog gallery
  (`apps/desktop/src/lib/dialog-gallery/fixtures/transfer-error.ts:112-114`, `gallery-registry.ts:196`), tests
  (`transfer-error-messages.test.ts`, `transfer-error-messages.parity.test.ts`, `transfer-dialogs.a11y.test.ts`,
  `transfer-parts.a11y.test.ts`), and keys `errors.write.deviceDisconnected.*`.

## Target design

### One release, one resume (M5)

`apps/desktop/src-tauri/src/file_system/volume/drive_release.rs` (new, `pub(crate)`, macOS and Linux) is the one place
that stops removable indexes after Cmdr decided to, and the one place that starts them again. **Why one module**: the
eject flight's sibling stop, the approver's stop, and the vanish stop are one piece of work with different budgets, and
the resume must be serialized against all of them.

- **The gate**: per volume id, a mutex-held `epoch: u64` and an optional in-flight resume ticket, plus a condvar.
- **`release(volume_ids, deadline) -> Release`**: for each id, bump its epoch first (an unstarted resume aborts), then
  wait until no resume ticket is in flight for it (bounded by `deadline`), then run `Index::stop_removable_volume`
  concurrently on blocking threads, all bounded by the same `deadline` instant. Per id: `NothingToStop`,
  `Released { was_indexing }`, or `StillReleasing`; `was_indexing` is captured before the stop
  (`Index::volume_kind(id) == Some(LocalExternal)`). A stop still running at the deadline keeps running detached, and
  its eventual answer goes to an optional continuation. The stop function is a parameter beneath `release`, so tests
  inject a slow or stuck stop and still exercise the real deadline.
- **`resume(candidates, owner)`**, never on a DA queue:
  1. wait `RESUME_SETTLE` 2 s (one timer, cancelled by any `release` of the id): tools send follow-up requests within
     milliseconds, and each would stop a fresh start again;
  2. on a blocking thread, read intent for the whole candidate set once (`Index::drives_to_resume()`, which opens each
     registered volume's DB; kept off the ask path);
  3. per id, under the gate: the epoch equals the one recorded at the stop, the owner's generation check passes (the
     approver's per-whole-disk ask generation, § "The unmount approver"), the volume is still in the mount table, it's
     in the intent set, and no other owner is ejecting it; then take the ticket;
  4. `Index::start_volume(id)` (on `tauri::async_runtime`; it returns once the reservation exists, so the hold is
     visible to the next stop);
  5. drop the ticket and notify.
- **`disable(volume_id)`**: `disable_drive_index` (`commands/indexing.rs:290`) goes through the gate: bump the epoch,
  wait for an in-flight ticket, then `Index::disable_volume`. So a disable is always the last word over a resume, which
  closes the window where `start_volume` would delete a fresh `user_disabled`.
- **Owners**: `Approver`, `EjectFlight(DiskKey)`, `Vanish` (never resumes). The approver skips ids in the ejecting set;
  a flight resumes only its own.
- **Why `drives_to_resume` instead of a per-id `Index` method**: the `Index` surface is at its ceiling (40 of 40), and a
  handful of read opens off the ask path is cheap. Its doc (`master.rs:169-174`) gets the resume named as its second
  caller, and still no launch caller.
- **What a start racing a stop can still do**: a user's enable or a search walk landing during a drain restarts the
  index before the old manager drops (`finish_stopping`), so the waiter answers `StillReleasing` and the eject or ask
  refuses honestly. The gate removes Cmdr's own resumes from that race; user actions keep today's honest refusal.

### Worker holds (M3 mechanism, M4 wiring)

- **Home**: `crates/cmdr-index/src/indexing/hold.rs`, a third shared leaf beside `volume.rs` and `metadata.rs`. **Why a
  new leaf**: scanner, reconcile, and watch must import it, they may not import `lifecycle::state`, and `volume.rs`
  promises pure predicates.
- **Keys**: a hold is `VolumeHold { volume_id, generation, kind: HoldKind }`. `generation` is a counter taken at each
  reservation; the table counts per (volume id, generation, kind), and remembers each generation's root.
- **Labels**: `HoldKind` has one variant per spawn site in § "Workers" plus `Reservation` (the root).
  `wait_until_released` answers `Released` or `StillHeld(Vec<(HoldKind, usize)>)`, and `stop_removable_volume`'s `warn`
  names the kinds. `RemovableStop`'s public shape doesn't change.
- **A drive that's gone doesn't stay held**: when `stop_removable_volume` runs, it asks the host's presence seam for
  every live generation's root; a generation whose root is `Some(false)` is flagged vanished at once. A wait counts only
  non-vanished generations, so a stuck worker on a dead device never blocks the re-plugged drive (same UUID, same id).
  After the stop's wait, vanished generations still held move to a zombie table with one `warn` naming their kinds.
- **Presence seam**: `VolumeProvider::is_mounted(&self, root: &Path) -> Option<bool>` (`None`: the table couldn't be
  read), answered app-side from the mount table (`volumes::mounts::is_mount_point`,
  `file_system::linux_mounts::is_mount_point`). `FakeVolumeProvider` gains `mark_unmounted`. No new type. Only
  local-scanner kinds ask it.
- **The pairing is a type**: `VolumeWork { cancel: CancellationToken, hold: VolumeHold }` with `child(kind)`. Every
  spawn function in § "Workers" takes `VolumeWork` where it took a bare `CancellationToken`, so drive-reading work
  without a hold doesn't compile. The share moves into the thread or task, abandoned walker workers included.
- **Roots**: the registry instance's `signals` carries the root `VolumeWork` (its `cancel` is today's `signals.cancel`),
  so work started from outside the manager (the verifier via `scan_control.rs`, cover walks via `Index::cover`) takes
  its share from the instance; the manager keeps its own.
- **Search cover walks**: `VolumeWork::linked(caller_token, &volume_work, kind)` keeps the walk a child of the caller's
  token and also cancels it when the volume's token cancels.
- **The scan completion task** checks the volume token before its replay and before spawning the live loop, under the
  slot lock; the loop carries a `LiveLoop` share.
- **Media**: `run_network_pass_blocking` refuses a volume whose index kind isn't a network kind, before any read.

### The unmount approver (M6)

**Home**: `apps/desktop/src-tauri/src/volumes/unmount_approver/` (macOS only), installed from `lib.rs` next to
`start_volume_watcher` and after `index_host::install`:

- `mod.rs`: `install(seams) -> Result<(), InstallFailure>`; one `DASession`, `DASessionSetDispatchQueue` on its own
  serial `dispatch2::DispatchQueue`; registration; `catch_unwind` around every callback body.
- `ask.rs`: the pure decision, including the shared deadline.
- `records.rs`: the pure record and resume-candidate state.
- `causes.rs` (M9): the pure unmount-cause machine.
- `private_symbols.rs`: `DARegisterIdleCallback` through `dlsym(RTLD_DEFAULT, ..)`, once.
- `volumes/disk_units.rs` (shared with M12): maps the non-blocking mount table to BSD names and whole units through
  `DADiskCreateFromBSDName` on a given session (no filesystem access; a MIG call per mounted local disk, so it runs only
  when an ask needs a group) and answers
  `mounted_volumes_on(session, whole_units) -> Vec<MountedVolume { bsd_name, whole_unit, volume_uuid, path }>` from a
  fresh table each call.

**Install failure**: if the session, queue, or any registration fails, `install` returns the failure, the app logs
`error`, and the `WillUnmount` observer is installed as before. Otherwise it isn't.

**Callbacks registered** (NULL match; the ask filters): unmount approval; appeared, disappeared, and description changed
(watch key `kDADiskDescriptionVolumePathKey`), which also keep the session recoverable; idle through `dlsym`; eject
approval (M9, always answered at once).

**The shared deadline.** DA times each callback from when it queued it, and asks run one at a time, so an ask that
starts right behind another was probably queued during it. Each ask computes its deadline:

- if it starts within `CHAIN_GAP` 50 ms of the previous ask's end, it joins that chain:
  `deadline = chain_start + APPROVAL_STOP_BUDGET`;
- otherwise it starts a chain: `deadline = now + APPROVAL_STOP_BUDGET`;
- an ask whose deadline already passed does no waiting: it approves when its group has nothing to stop and is not busy;
  otherwise it hands the stop to a detached `release` and dissents.

`CHAIN_GAP` is 50 ms because a callback queued during an ask runs microseconds after it returns, and 50 ms absorbs a
loaded scheduler without swallowing an unrelated later request.

**`APPROVAL_STOP_BUDGET` = 7 s per chain.** DA's timer is 10 s from queueing (`DAQueue.c:178-180`; the 1 s grace isn't
relied on). 7 s leaves 3 s (30 %) for the client's wake-up and queue copy, the asks behind the stop that answer at once,
and scheduling under load, and it still covers `shutdown`'s 5 s live-loop drain. The residual risk: several indexed
drives ejected together share one 7 s chain; the later ones dissent on a non-force request (honest), and under force
their stop races the unmount. That's the case a single serial session can't do better on.

**An ask** (on the DA queue):

1. Copy the callback disk's description: `VolumePath`, `VolumeUUID`, BSD name, and the whole disk's unit
   (`DADiskCopyWholeDisk`). No path, or no Cmdr volume whose ACTIVE root is that path → approve.
2. Bump the ask generation of that whole disk (keyed by whole BSD name, reset on `Appeared(whole)`).
3. The group: every registered volume mounted on the same whole unit (`disk_units`). **Why the whole unit**: DA links a
   Whole request's asks by BSD unit and they arrive back to back, so the first ask must stop the unit's group; a
   physical disk's other container is a different request.
4. `drive_release::release(group, deadline)`. A group with no index work answers `NothingToStop` for every id at once.
5. Answer: dissent (`DADissenterCreate(kDAReturnBusy, NULL)`) if any id is `StillReleasing` or in `busy_volume_ids()`;
   otherwise approve.
6. Record every `Released { was_indexing: true }` id, keyed by BSD name + `VolumeUUID`, with the epoch `release` set and
   the whole disk's ask generation. An ask for an already-recorded volume (stopped by an earlier refused request)
   carries its record forward with the new generation, since `was_indexing` now reads false.
7. Log one `info` (volume, group, outcome, deadline left); the crate's `warn` names holders on `StillReleasing`.

**Resume** (idle callback, same queue): snapshot the records as candidates and hand them to
`drive_release::resume(candidates, Approver)`, where step 3's owner check is "the whole disk's ask generation still
equals the record's" (no newer ask arrived) and presence is the mount table's entry for that BSD name. Records are
dropped on `Appeared(whole)` and `Disappeared(whole)`. The callback does no I/O. If the idle symbol is missing, log one
`warn` at install and resume nothing; ❌ no polling fallback.

**Cmdr's own eject** pre-stops its volumes, so its asks find nothing to stop and approve at once; before M12, siblings
aren't in the ejecting set, and the approver's gated resume covers them.

**Linux** has no approver; `drive_release` serves its eject flight.

### A vanished drive, index side (M7, M8)

Crate-side, whatever the host detects and whenever; every gate below applies to local-scanner kinds only.

**Deletes (M7)**, with the gate point per site:

- **Listings become typed**: `read_fs_children` and the verifier's disk read answer
  `Listing { children, complete: bool }`; any entry-iteration or stat error sets `complete = false`.
  `diff_dir_against_db` deletes nothing for an incomplete listing. Before sending a directory's deletes, one
  `is_mounted(root)` must be `Some(true)`. Gate point: per directory, for every local caller (`reconcile_subtree` and
  its three users, the full local rescan, the stitch) and the verifier's diff. The trait-scanned caller keeps today's
  behavior.
- **Per-event deletes** (`handle_removal`, `handle_creation_or_modification`): a stat error deletes only on
  `NotFound`/`ENOTDIR`. The live loop, the post-scan replay, and cold-start replay gather an event batch's deletes and
  ask `is_mounted(root)` once per batch before sending them.
- **Boot-disk verification**: `Path::exists()` becomes an errno-typed stat; the parent's `read_dir` result is checked
  before any child delete.
- **`ScanRoot::Rebuild`**: `DeleteDescendantsById` is sent only after the walk reports the root's own read succeeded; a
  failed root read deletes nothing.
- **The delete generation**: a per-volume counter of delete batches sent since the last `Some(true)` presence reading
  after a successful root listing.

**Completion and rebuilds (M8)**:

- **`Abandoned` marks** are sent only while `is_mounted(root) == Some(true)`; a walk that would mark while the root is
  unlisted drops the marks and reports a vanish. A `LocalExternal` start clears `UnreadableCause::Abandoned` once
  (`IndexStore::clear_unreadable_cause`) through the writer before its first walk, so ground an earlier vanish left
  unwalked is frontier again. `writer/abandoned_retry.rs`'s stale doc is fixed.
- **Completion gate**: one `is_mounted(root) == Some(true)` read decides, per path:
  - scanner thread and `finish_reconcile`: before queueing `ComputeAllAggregates` and `WalCheckpoint`;
  - `scan_completion`: `was_completed` also requires it; gated writes are `scan_completed_at`, sweep keys, calibration,
    `volume_path`, freshness `ScanCompleted`, phase Live, and the live-loop spawn; a failed gate is a vanish
    (`ScanAborted`, freshness `ScanFailed`, no stamp);
  - the phase machine's `take_stock`: before `stamp_home` and `run_the_completion_sequence`.
- **The rebuild marker**: when a presence read is `Some(false)` with a non-zero delete generation, or a stop finds the
  root unlisted with a non-zero generation, the crate persists meta `index_needs_rebuild = 1`: through the writer when
  it's alive (then flush), otherwise through a short-lived connection in the after-drain slot (the
  `set_drive_index_intent` pattern). It emits `IndexEvent::IndexNeedsFreshScan { volume_id }` (no new type) when the
  marker lands. Persisted, so it survives quit, a restart during the drain, and Linux, where unmounts never stop an
  index.
- **The route**: `launch_route` gains a `needs_rebuild` input checked first → `RebuildThenCoverInPhases`, whose
  `RebuildFirst` clears the marker with the stamps. The partial case needs no marker: no stamp and rows without a branch
  set already route there. (`IncompletePreviousScan` is a never-emitted `RescanReason`; don't use the name.)

### A vanished drive, host side (M9)

**Causes** (`causes.rs`, pure, fed in callback order on the one serial queue; ❌ never timing):

- `Appeared(whole)` resets that disk's facts and drops its records.
- `UnmountAsked(volume, whole)` marks the volume asked.
- `EjectApproved(whole)` marks the disk ejected.
- `VolumePathCleared(volume)` (description change; the machine keeps the last-known path) → `Asked` if marked, otherwise
  `Unasked` (a raw `umount`, or an unmount DA made while our session was skipped).
- `Disappeared(whole)` → `Pulled` if a Cmdr-known volume was mounted since it appeared and no eject approval came, else
  `Ejected`. If any ask since the disk appeared ran past DA's window (the client knows its own ask durations), the cause
  is `Unknown`: a skipped eject approval looks like a pull. `Unknown` acts like `Pulled` and only the log wording
  differs.
- For `Unasked`, `Pulled`, or `Unknown`: drop the disk's records, run `drive_release::release([id], VANISH_STOP_WAIT)`
  with owner `Vanish` on a thread off the DA queue, and log a `warn` naming the cause. The crate's stop flags the
  generation vanished and writes the rebuild marker if deletes were in flight.
- The app maps `IndexNeedsFreshScan` to an info toast (§ "Copy drafts").

### A vanished drive, transfers (M10)

- **Typed sides**: `TransferSide { volume_id, volume_name, root }`, built where `copy_between_volumes` /
  `move_between_volumes` already resolve both volumes (`copy.rs:144`, `move.rs:77`), and threaded into the local engine
  beside the paths. ❌ No path-prefix lookup.
- **Classification**: an I/O error on a side whose `root` has left the mount table is
  `DeviceDisconnected { path, side }`, whatever the errno; `ENXIO` joins `ENODEV` as typed evidence. The wire variant
  carries `side: DisconnectedSide { role: Source | Destination, volume_id, volume_name }`: the frontend's volume list
  drops an unmounted volume, so the name is captured at start. `classify_io_error` takes the side context;
  `map_volume_error` maps its `PathRole`. Every site in § "Transfers and temps" updates, bindings regenerate.
- **Progress facts**: `WriteErrorEvent` gains `progress_at_stop` (`files_done`, `files_total`, `bytes_done`,
  `bytes_total`) read from the status cache before the op unregisters, plus, for a move, `sources_removed` and
  `sources_left` counts.
- **Moves delete sources only after a real flush**: `flush_created_destinations` returns `Result`, and for a cross-FS
  move it fsyncs every destination directory that received a rename (the landed parents and the destination) even when a
  file's data is already durable. Phase 4 runs only when the flush returned `Ok` AND the destination side's root is
  still listed; otherwise the sources stay, and the error names what's where. ❗ Whether `sync_data` issues
  `F_FULLFSYNC` on Apple targets is unverified; M10 checks the std source and records it.
- **The source sweep asks the mount table**: `remove_landed_file`'s `NotFound`, `remove_swept_dir`'s failed stat, and
  the per-source skip (`source_sweep.rs:238`) each check the source root; unlisted → stop the sweep and report
  `sources_left`.
- **Mac-side cleanup** stays: the local partial goes via `discard_temp`, a Mac-side staging dir via
  `remove_dir_all_in_background`.

### A vanished drive, temps and asides (M11)

1. **Records get a shape old builds skip.** New op bytes `A` (add) and `a` (retire) carry tagged JSON:
   `{"kind": "temp"|"aside"|"staging_dir", "volume_id", "path", ...}`. An older `read_recorded` ignores unknown op bytes
   and truncates the log at launch, so a reverted build forgets these records and deletes nothing. Existing `+`/`-`
   lines still replay.
2. **Temps on a non-root mount register volume-homed** (`volume_id` from the typed destination side, path relative to
   the root), so a launch with the drive absent defers them to arrival. Mac-internal temps stay `LocalFs`.
3. **`discard_temp` deregisters only when the remove succeeded**, or failed with `NotFound` while the temp's root is
   still listed. Otherwise the record stays and joins `pending` at once, installing the arrival listener lazily, so a
   re-plug in the same session sweeps it. (Needs item 2: `pending` holds only volume-homed records.)
4. **Every aside registers with its kind**: `FileAside { expected_size }` (`stage_and_land_file`; the new file's size),
   `DisplacedFile` (`displace_with_directory`), `DirOverwriteAside` (`safe_overwrite_dir`), and `VolumeAside`
   (`displaced_destination.rs`). The sweep:
   - destination missing → `rename_no_replace(aside, dest)`;
   - `FileAside` and the destination is a regular file of exactly `expected_size` → remove the aside;
   - anything else → rename the aside to `recovered_sibling(dest)` through `rename_no_replace` and log `warn`. A
     displaced file whose directory filled leaf by leaf is always kept.
5. **Staging dirs**: `cmdr_fs::staging` gains `STAGING_DIR_PREFIX` (`.cmdr-staging-`) and `is_staging_dir_name`, a
   strict parse of the prefix plus the operation-id shape `cross_fs.rs:107` writes (verify it at
   `write_operations/manager.rs:431`). The move registers the dir when it creates it and retires it when Phase 5's
   `remove_dir` succeeds. The sweep only ever `remove_dir`s it: a non-empty staging dir stays, with a `warn` and the
   notice in § "Copy drafts". `is_one_of_ours` accepts it; the listing hide gate learns the name, still by ownership.
6. **The sweep runs on `on_volume_arrival`**, through the existing volume-record path.

### Cmdr's own eject, per physical disk (M12)

1. **One flight per disk, visible per volume.** A disk flight inserts every sibling id into `IN_FLIGHT` with the disk
   flight's shared future and one `Landing` per id, so a later `eject(B)` joins in `join_or_start` before
   `is_already_unmounted` ever runs. A B whose own flight started before A's disk flight adopted it resolves to the same
   `DiskKey` and awaits the disk flight from inside its own; both orders are pinned.
2. Per-volume preflight, unchanged: busy gate, device provider, `is_already_unmounted`, the ejectability check.
3. **Resolve** (`eject/disk_target.rs`) under `DISK_RESOLVE_DEADLINE` 5 s on the flight's own session and queue:
   `DADiskCreateFromVolumePath`, `DADiskCopyWholeDisk`; an `AppleAPFSContainer` whole walks `kIOServicePlane` parents to
   the physical store and its whole; `DiskKey` = `IORegistryEntryGetRegistryEntryID` of the physical whole;
   `container_units` from its descendants. A stall → `NotResponding { step: DiskResolve }`. A NULL disk → gone from the
   table → `Ok`; still listed (a non-DA volume like macFUSE) → `disk: None`, today's per-volume path.
4. **Capture siblings**: registered volumes whose `volume.root()` is a path in
   `mounted_volumes_on(session, [physical_whole_unit] + container_units)`; keep their paths.
5. **Sibling busy gate**: `Busy` if any sibling is in `busy_volume_ids()`.
6. **Stop every sibling**: `drive_release::release(siblings, now + INDEX_STOP_DEADLINE)`. Any `StillReleasing` →
   `NotResponding { IndexStop }` and nothing unmounts; resume the released ones (owner `EjectFlight`), and a
   still-releasing one from its continuation when its stop ends.
7. **Teardown**: `diskutil eject <path>` through `settle_with_retries`, with `still_mounted` = any captured path still
   listed OR a fresh `mounted_volumes_on` non-empty (fail closed), and each retry aimed at a still-listed captured path.
8. **Final `UnmountRefused`** → holders (M13, M14), then `drive_release::resume(siblings, EjectFlight)`.
9. **`TimedOut`** → answer at once; `within_tool_timeout` hands the join handle to a detached settle task, and when the
   tool exits, resume still-listed siblings.
10. SMB keeps `diskutil unmount` and gains holders; Linux keeps `umount`. `EjectStep` gains `DiskResolve`.

### Holders (M13 scan and wire, M14 facts)

- **When**: once, in `run_teardown`, after a final `UnmountRefused`, for disks and SMB shares.
- **Scan**: `proc_listpidspath(PROC_ALL_PIDS, 0, path, PATH_IS_VOLUME | EXCLUDE_EVTONLY)` over each still-listed
  captured path (the share's mount path for SMB), on ONE abandonable std thread under `HOLDER_BUDGET` 1.5 s (a
  parameter). The thread records the root's `stat().st_dev` before and after each scan and discards a result whose
  device changed or whose stat failed. Past the budget the thread is detached. FFI: a two-line `extern "C"` plus
  `PROC_ALL_PIDS = 1`, `PROC_LISTPIDSPATH_PATH_IS_VOLUME = 1`, `PROC_LISTPIDSPATH_EXCLUDE_EVTONLY = 2`
  (`libproc.h:52,61`, `sys/proc_info.h:51`). ❌ No `libproc` crate. Linux answers empty.
- **Facts**, cheapest first, inside `objc2::rc::autoreleasepool`, ancestors up to eight levels, stopping at PID 1:
  1. `pid == own_pid` or an ancestor is → `Cmdr`.
  2. `NSRunningApplication` for the process, then each ancestor, any activation policy but prohibited →
     `App { name, bundle_id }` (a shell under Warp names Warp).
  3. `responsibility_get_pid_responsible_for_pid` through `dlsym`: a responsible process with an `NSRunningApplication`
     → `App`; one without (Google Drive) → `App` named from its signing Info.plist (`kSecCodeInfoPList`:
     `CFBundleDisplayName`, then `CFBundleName`) when its executable isn't on the target volume.
  4. The executable's device (`lstat` of `proc_pidpath`) equals a mounted target root's own `stat().st_dev` (❌ not
     `f_fsid`) → `Tool { name }`, with no Security call.
  5. Apple platform binary (`kSecCodeInfoPlatformIdentifier`) → `System`; else `Tool`.
- **Nested images**: an attached image whose backing file's `st_dev` equals a target root's → one `DiskImage { name }`.
  M14 picks how to list attached images (an IOKit property of the disk-image device, or `hdiutil info -plist` under a
  timeout) inside the budget.
- **Merge**: dedupe by PID in first-seen order, drop ESRCH.
- **Accepted tradeoffs** (recorded in `volume/DETAILS.md`): `backupd` reads `System`; a helper with no responsible
  answer and a launchd parent reads `System`; an orphaned platform CLI reads `System`; a path heuristic was rejected;
  root-owned holders aren't visible to a same-uid scan, so those refusals name nobody until the deferred DA teardown.

**Wire** (M13):

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

- M13 ships every holder `Unclassified` with its executable name; M14 fills in kinds.
- `Display`: `unmount refused (held by Warp [app, pid 94646], lsd [system, pid 983]): <detail>`.
- MCP: keep the message; set `ToolError.data` (`mcp/executor/mod.rs:62`) to
  `{ "outcome": "unmountRefused", "holders": [...] }`.

### Budgets and deadlines

- `APPROVAL_STOP_BUDGET` 7 s per ask chain, `CHAIN_GAP` 50 ms (new): § "The unmount approver".
- `RESUME_SETTLE` 2 s (new): the quiet period before a Cmdr resume starts, cancelled by any stop of that volume.
- `VANISH_STOP_WAIT` 15 s (new, replaces `INDEX_RELEASE_WAIT`): the stop after a vanish, off the DA queue; logs only.
- `EJECTABILITY_CHECK_DEADLINE` 5 s (unchanged). `DISK_RESOLVE_DEADLINE` 5 s (new).
- `INDEX_STOP_DEADLINE` 15 s (unchanged), covering all sibling stops, concurrently.
- `TOOL_TIMEOUT` 30 s, `REFUSAL_RETRY_BACKOFF` 0.5, 1, 1.5 s, `RETRY_BUDGET` 30 s (unchanged).
- `HOLDER_BUDGET` 1.5 s (new, injected).
- **What a person waits**: a clean eject under a second; a typical refusal about 3 s of retries plus the daemon's scan
  time per attempt plus 1.5 s; the worst case, every step at its limit, 5 + 5 + 15 + 60 + 1.5 ≈ 87 s (today about 80 s).

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
locale).

**Drafts for David's later review** (they don't block; key families follow each surface's existing family):

- Index notice (M9), an info toast: "{volumeName} was disconnected while Cmdr was updating its index. The next scan of
  this drive starts from scratch, so its folder sizes are right again."
- Transfer, copy to the drive (M10): "{volumeName} was disconnected after Cmdr copied {done} of {total} files to it.
  Your originals are still on your Mac. Connect the drive and copy the rest again; Cmdr removes the unfinished file it
  left there."
- Transfer, copy from the drive: "{volumeName} was disconnected after Cmdr copied {done} of {total} files to
  {destination}. The rest are still on the drive."
- Transfer, move to the drive, sources kept: "{volumeName} was disconnected before Cmdr could finish the move, so all
  your files are still on your Mac."
- Transfer, move from the drive: "{volumeName} was disconnected after Cmdr moved {done} of {total} files to
  {destination}. The rest are still on the drive."
- Staging dir kept on a returning drive (M11), an info toast: "Cmdr found files from an unfinished move on {volumeName}
  and left them in place, in a hidden folder named {folderName}."

## Milestones

Order: **M1 → M2 → M3 → M4 → M5 → M6 → M7 → M8 → M9 → M10 → M11 → M12 → M13 → M14 → M15 → release checkpoint.**

- **M1, M2 first**: every later real-image test runs on the harness and the lane.
- **M3, M4 before M5, M6**: the approver's "approve" is only honest when `Released` means no worker reads the drive, and
  the presence seam M3 adds is what keeps a dead drive's holds from blocking its next life.
- **M5 before M6**: no resume may exist without the gate.
- **M7, M8 before M9**: the host's vanish handling relies on the crate refusing wrong writes and persisting the marker.
- **M10, M11** are app-side and independent of M7–M9; they may run in a parallel worktree after M6.
- **M12 → M13 → M14 → M15**, then the checkpoint (an FF-merge to David's local `main`; a tagged release only when he
  says).
- **Cadence**: plain `pnpm check` per milestone; `pnpm check --include-slow` after M4, M6, M8, M10, M12, M14, and before
  the checkpoint.

### M1: disk-image harness and pins of today's behavior

- **Scope**:
  - `cmdr_fs::testing` becomes a directory module with a macOS `disk_images` submodule: `DiskImageSession` (a
    machine-wide `flock` on `$TMPDIR/cmdr-disk-image-tests.lock`, held for a test's whole body), unique per-run volume
    names (`CMDR<pid><n>`), and the guarded runner with an explicit verb list:
    - `hdiutil`: `create`, `attach -plist -nobrowse`, `info -plist`, `detach`, `detach -force` (non-nested, ours only);
    - `diskutil`: `info -plist`, `apfs addVolume`, `partitionDisk`, `mount -mountOptions nobrowse`, `unmount`,
      `unmountDisk`, `eject`. Every mutating verb re-checks identity first; every call has the SIGKILL deadline.
  - `DiskImage::attach(ImageSpec)`: `Apfs`, `ApfsTwoVolumes` (`hdiutil create -size 1100m -type SPARSE -fs APFS`, then
    `addVolume <container> APFS <name> -nomount`, then a nobrowse mount), `HfsTwoPartitions`
    (`-layout GPTSPUD -fs HFS+`, then `partitionDisk <disk> GPT JHFS+ A 60M JHFS+ B R`, then nobrowse remounts). `Drop`
    detaches after the identity check.
  - `external_drive_fixture` repoints at the runner (one runner, no `jscpd` pair); its FAT/exFAT tests stay as they are.
  - A guarded `run_tool` closure for `unmount_tool::settle_with_retries` (a `diskutil eject` through the runner, each
    attempt identity-checked).
  - `#[ignore]` pins in `apps/desktop/src-tauri/src/file_system/volume/eject/real_image.rs`
    (`#[cfg(all(test, target_os = "macos"))] mod real_image;`), calling `settle_with_retries` with the guarded closure.
  - An `#[ignore]` pin in `crates/cmdr-index/src/indexing/tests/vanish_tests.rs` (macOS): a real `IndexManager` over an
    HFS+ image (the `event_stream_tests.rs` shape) with a tree big enough to scan for a few seconds, detached with
    `hdiutil detach -force` at the first `ScanProgress`.
- **Intentions**:
  - Eject pins: idle → `Ok` and detached; a held file → `UnmountRefused`; two-volume APFS with a file held on B, eject A
    → whatever today answers, recorded and commented as the gap M12 flips; the same on the two-partition HFS+ image.
  - The vanish pin records today's outcome (expected: rows gone and `scan_completed_at` written, or `Abandoned` marks
    written), commented as the gap M7 and M8 flip.
  - Plist parsing uses a workspace dependency already in `Cargo.lock` if one fits; otherwise the dependency guide.
- **Landmines**:
  - A child `sleep` spawned by a test descends from the test process, so M14 classifies it `Cmdr`: assert PID
    membership.
  - `crates/cmdr-index/src/indexing/tests/CLAUDE.md` says "never `diskutil unmount` a path": scope that sentence to
    FAT/exFAT images, since APFS/HFS+ unmounts under test are the point.
  - Two worktrees run the lane at once: the lock and unique names are what keep them apart, and a test that forgets the
    session lock can stall another run's unmounts. Take it in the fixture constructor, not per call site.
  - `nextest-filter-coverage`: `[[profile.default.overrides]]` in the `disk-image` group for both new paths, 30 s cap,
    with the `allowed-unmatched-nextest-filter` comment (`.config/nextest.toml:76-85` is the model).
  - `addVolume` answers -69493 on a small container; the sparse 1.1 GB image works.
- **Test plan**:
  - Smoke first: one pin, then the rest.
  - `pnpm check rust`.
  - Hand runs (named exception, results in the commit body):
    `cargo nextest run -p cmdr --run-ignored only -E 'test(file_system::volume::eject::real_image::)'` and
    `cargo nextest run -p cmdr-index --run-ignored only -E 'test(indexing::tests::vanish_tests::)'`, plus the existing
    `external_drive_fixture` run from `crates/cmdr-index/src/indexing/tests/DETAILS.md`.
  - **Must not change**: `unmount_tool::tests::*`, `eject::tests::*`, `external_drive_fixture::tests::*`.
- **DONE**: pins green on current code, results in the commit body, checks green.
- **Docs**: `crates/cmdr-fs/DETAILS.md` (the runner, its verbs, the lock),
  `crates/cmdr-index/src/indexing/tests/CLAUDE.md` and `DETAILS.md`,
  `apps/desktop/src-tauri/src/file_system/volume/DETAILS.md` § "Eject" (the pins).
- **Size**: 600–750 lines.

### M2: the opt-in macOS disk-image lane

- **Scope**: a check `desktop-rust-disk-images` (nickname `disk-images`) in `scripts/check/checks/registry.go`.
- **Intentions**:
  - `IsSlow: true`; `NotInCI` (every CI runner is ubuntu, and `hdiutil` has no Linux counterpart);
    `Exclusive: ResourceCargoBuildDir`; depends on clippy.
  - On a non-darwin host it answers OK with "skipped: macOS only".
  - It runs nextest `--run-ignored only` over an explicit filter union of this plan's real-image paths. ❌ Not
    `external_drive_fixture::`.
  - Machine-wide serialization comes from M1's per-test lock, not from the lane: the Go runner holding the same lock
    would block its own test processes.
  - Each later milestone adds its real-image path to the union.
- **Landmines**: `ci-coverage` may want an entry or an exemption; follow what it prints. The union and the
  `.config/nextest.toml` overrides must name the same paths.
- **Test plan**: `pnpm check disk-images` on macOS (runs M1's pins); a Go unit test for the filter union;
  `pnpm check go`.
  - **Must not change**: every other lane's selection (`scripts/check/cli_test.go`).
- **DONE**: `pnpm check --include-slow` runs the pins on macOS; the hand-run exception no longer applies.
- **Docs**: `scripts/check/checks/DETAILS.md`, `docs/tooling/testing.md`,
  `crates/cmdr-index/src/indexing/tests/DETAILS.md`, `volume/DETAILS.md` § "Eject".
- **Size**: 150–200 lines.

### M3: the hold leaf, generations, and the presence seam

- **Scope**: `crates/cmdr-index/src/indexing/hold.rs` (from `state/release.rs`) with `HoldKind`, per-generation keys,
  the vanished flag and zombie table, `VolumeWork` and `VolumeWork::linked`; `VolumeProvider::is_mounted` with its app
  implementation and fake; the reservation's root `VolumeWork` on the instance's `signals`; the media kind gate.
- **Intentions**:
  - The hold is still taken inside the reservation's critical section.
  - `wait_until_released` counts non-vanished generations and names holders.
  - `stop_removable_volume` flags generations whose root is `Some(false)` before waiting, and zombifies the still-held
    vanished ones after.
  - `indexing/CLAUDE.md` names three shared leaves.
- **Landmines**:
  - The `Initializing` teardown arm removes the instance; the instance's root share must drop there exactly as the
    manager's does today.
  - `None` from `is_mounted` (an unreadable table) is never "vanished".
  - `cover/CLAUDE.md` is at 598 words.
  - Don't widen `lifecycle::state` imports below `lifecycle`.
- **Test plan** (red first):
  - `hold` unit tests: per-kind counts; a waiter naming its holders; a stuck share of a vanished generation stops
    blocking a new generation's stop at once, and moves to the zombie table after the wait; a linked walk cancelled by
    either token.
  - The media gate refuses a `LocalExternal` id before any read.
  - `pnpm check`; `pnpm check index-crate-isolation` shows no ceiling moved.
  - **Must not change** (moved with the module, names unchanged): the three `release::tests`;
    `state::tests::a_removable_stop_waits_for_the_start_it_cancelled`; `cover::cold_drive_tests::removals`;
    `eject::tests::an_index_still_letting_go_of_the_drive_never_meets_the_unmount`;
    `deadlines::tests::a_stuck_index_stop_never_reaches_the_unmount_and_the_flight_lands`.
- **DONE**: the mechanism exists with its tests; checks green.
- **Docs**: `crates/cmdr-index/src/indexing/CLAUDE.md` (leaves), `lifecycle/DETAILS.md` § "When a volume has been let
  go", `host/DETAILS.md` (`is_mounted`).
- **Size**: 450–550 lines.

### M4: every worker carries a share

- **Scope**: every spawn site in § "Workers" takes `VolumeWork` (or `linked`): the walker and its workers, the scanner
  thread, local reconcile and its reader threads, the subtree rescan chain, the verifier and its `scan_subtree`, the
  phase machine, cover walks and the mount probe, the scan completion task (token check before replay and before the
  live-loop spawn), and the live loop.
- **Intentions**: no drive-reading spawn site takes a bare `CancellationToken`; verify the importance Spotlight sample
  never touches a `LocalExternal` mount, and give it a share if it does.
- **Landmines**:
  - An abandoned walker worker holding its share turns a hung read on a mounted drive into `StillReleasing`; that's
    intended.
  - Preemption and search cancel of cover walks must not change.
  - Thread `VolumeWork` through signatures without widening `pub` items (`index-crate-isolation`).
- **Test plan** (red first per worker):
  - With a gated read seam or a parked task, assert the volume stays held after the manager dropped and releases when
    the worker exits: an abandoned walker worker, a `reconcile-read` thread, a `rescan-subtree` thread, the live loop
    past its drain timeout, a verifier task, a search cover walk (cancelled by the volume stop), and the completion task
    (no replay, no live-loop spawn after cancel). ❌ No sleeps (`cmdr_fs::testing::wait_until`).
  - `pnpm check`, then `pnpm check --include-slow`.
  - **Must not change**: M3's list, plus `event_stream_tests`, `stress_tests_lifecycle`, `integration_tests.rs`,
    `phases::tests`, `cover::cold_drive_tests::*`.
- **DONE**: every worker in the inventory holds its share; checks green.
- **Docs**: `lifecycle/DETAILS.md` § "When a volume has been let go" (drop "neither is part of the answer"),
  `transports/DETAILS.md` § "The drain is cooperative", `cover/DETAILS.md` (the linked cancel), `scanner/DETAILS.md`.
- **Size**: 900–1,100 lines.

### M5: `drive_release`, the gated stop and resume

- **Scope**: `file_system/volume/drive_release.rs` with the gate, epochs, tickets, `RESUME_SETTLE`, the off-queue intent
  read, owners, and `disable`; `stop_index_blocking` in `eject/mod.rs` moves onto `release` for its one volume;
  `disable_drive_index` goes through the gate.
- **Intentions**: as § "One release, one resume"; `master.rs:169-174` names the resume as the second caller.
- **Landmines**:
  - `release` bumps the epoch BEFORE waiting on a ticket; the other order lets an unstarted resume slip through.
  - `start_volume` must have returned (reservation made) before the ticket drops; don't drop it at spawn.
  - Never block a DA or GCD thread on the gate; `release` is called from the approver's queue, so its waits stay inside
    the caller's deadline.
  - `start_volume` runs on `tauri::async_runtime`, ❌ never `tokio::spawn` from a non-runtime thread.
- **Test plan** (pure, deterministic, injected stop and start seams):
  - a release arriving before the resumed start reserves: the epoch aborts the resume, or the release waits for the
    ticket and then stops the new instance;
  - a release during `RESUME_SETTLE` cancels the resume;
  - a disable during an in-flight resume waits for it and wins;
  - the deadline is one instant across ids; a stuck stop answers `StillReleasing` at the deadline and reaches its
    continuation later;
  - the intent read happens once per resume batch and never for a candidate that failed an earlier check;
  - `pnpm check`.
  - **Must not change**: `eject::tests::*` (the stop's answers), `deadlines::tests::*`.
- **DONE**: every Cmdr-made stop and start of a removable index goes through the module.
- **Docs**: `volume/DETAILS.md` § "Eject" (one stop, one resume), `lifecycle/DETAILS.md` (the resume caller).
- **Size**: 450–550 lines.

### M6: the unmount approver

- **Scope**: `volumes/unmount_approver/{mod.rs, ask.rs, records.rs, private_symbols.rs}`, `volumes/disk_units.rs`;
  `objc2-disk-arbitration` (features `DADisk`, `DADissenter`, `DASession`, `dispatch2`) and `dispatch2` as direct
  dependencies per `docs/guides/add-rust-dependency.md`; the install in `lib.rs` with the `WillUnmount` fallback.
- **Intentions**: the ask, the shared deadline, dissent, records keyed by BSD name + `VolumeUUID`, the per-whole-disk
  ask generation, and the resume hand-off exactly as § "The unmount approver"; decision 8 fixed by construction.
- **Landmines**:
  - A test's approval session is asked about EVERY unmount on the Mac for its lifetime: its seams act only on the test
    image's BSD names and approve everything else at once, and it's unscheduled on drop. M1's session lock is held.
  - A retained callback disk's description is frozen: presence comes from the mount table.
  - An ask never opens SQLite and never calls `drive_release::resume` inline.
  - `volume/CLAUDE.md` is at 599 words; `volumes/CLAUDE.md` at 490.
  - Hardened runtime: `dlsym` of a system symbol should work in a signed build; the checkpoint smoke-tests one.
- **Test plan**:
  - Pure `ask.rs`: nothing to stop → approve; released → approve; still releasing → dissent; busy → dissent after the
    stop; the first ask of a group stops every sibling, the second approves at once; queued behind a 5 s ask → 2 s left;
    a passed deadline → answers at once (approve when nothing to stop, else detached stop and dissent).
  - Pure `records.rs`: a record survives a refused request and resumes after idle; a newer ask on the whole disk cancels
    it; an ask for a recorded volume carries it forward; `Appeared` and `Disappeared` drop it.
  - Lane (the stop is injected beneath `release`, never the budget):
    - `diskutil unmount` of an idle volume → asked while still listed, unmounted, no resumed start;
    - a held file → refused, one resumed start after idle and settle;
    - `diskutil unmount A` then `diskutil unmount B` back to back on the two-partition image, A's ask having stopped
      both → no start for B between the requests, B unmounted with no live index;
    - `diskutil unmountDisk` on the two-partition image → the second ask found nothing to stop, no resumed start in
      between, both unmounted;
    - an injected stop that overruns → dissent at the deadline, `diskutil unmount` refused;
    - with that stop, `hdiutil detach -force` → detached anyway (force ignores dissent);
    - an install failure (injected) → the `WillUnmount` observer is installed.
  - `pnpm check`, `pnpm check disk-images`, then `pnpm check --include-slow`.
  - **Must not change**: `volumes::watcher::tests::*` (the will-unmount handler stays as the fallback),
    `eject::tests::*`, M5's tests.
- **DONE**: the approver runs in the app with the fallback; lane and checks green.
- **Docs**: `volumes/CLAUDE.md`, `volumes/DETAILS.md` § "Key decisions" (DiskArbitration owns the pre-unmount hook; the
  fallback), `transports/DETAILS.md` § "Unmount/eject lifecycle", `volume/DETAILS.md` § "Eject", `docs/architecture.md`
  (the `volumes/` map line).
- **Size**: 800–950 lines.

### M7: index delete gates

- **Scope**: typed `Listing { children, complete }` for `read_fs_children` and the verifier; the per-directory gate in
  `diff_dir_against_db`'s local callers and the verifier's diff; errno-typed per-event deletes with a per-batch presence
  read in the live loop and both replays; the boot-disk verification fix; `ScanRoot::Rebuild`'s delete after the root
  read; the delete generation.
- **Intentions**: every site in § "Index writes from a failed or missing observation" gets its named gate; re-audit the
  code for any site the inventory missed before changing anything.
- **Landmines**:
  - The trait-scanned `diff_dir_against_db` caller and the SMB/MTP watch deletes don't change; gate at the local call
    sites, keyed on `uses_local_scanner()`.
  - A live removal on a healthy drive must keep deleting (`ENOENT` while listed is a real delete).
  - One presence read per directory diff or event batch, never per entry.
  - An incomplete listing must still upsert what it did see.
- **Test plan** (red first; `FakeVolumeProvider::mark_unmounted` for determinism):
  - each local site deletes nothing when unmounted, and nothing on a non-`NotFound` error while mounted;
  - an incomplete listing deletes nothing and still upserts;
  - `Rebuild` with a failing root read keeps the subtree;
  - boot-disk verification with `EACCES` deletes nothing;
  - the delete generation counts batches and resets on a good root listing;
  - lane: M1's vanish pin loses no rows; a new pin force-detaches while a live watcher processes removals;
  - `pnpm check`, `pnpm check disk-images`.
  - **Must not change**: the reconcile and verifier suites, `watch` suites, `integration_tests.rs`, `network_scanner`
    reconcile tests (an SMB/MTP reconcile still deletes a removed entry), the SMB and MTP watch tests.
- **DONE**: no local-scanner site deletes from a failed or unlisted observation.
- **Docs**: `reconcile/DETAILS.md` (the gates), `watch/DETAILS.md` (per-batch presence), `scanner/DETAILS.md`
  (`Rebuild`).
- **Size**: 650–800 lines.

### M8: completion gates, `Abandoned` marks, and the rebuild marker

- **Scope**: presence-gated `Abandoned` marks; the `LocalExternal` start clearing `UnreadableCause::Abandoned`; the
  completion gate on the scanner thread, `finish_reconcile`, `scan_completion`, and the phase machine's `take_stock`;
  `index_needs_rebuild` (writer or after-drain connection), `IndexEvent::IndexNeedsFreshScan`; the `needs_rebuild`
  launch-route input; the stale `abandoned_retry.rs` doc.
- **Intentions**: as § "A vanished drive, index side"; M1's vanish pin flips fully.
- **Landmines**:
  - ❌ `clear_index` for invalidation.
  - The after-drain connection must never open while a writer thread lives (the `set_drive_index_intent` contract); use
    the writer when it's alive.
  - `IndexEvent` variants carry no new type (a carried type spends a root promise).
  - `finish_stopping` skips the after-drain slot when a restart landed; the marker must already be on disk by then.
- **Test plan** (red first):
  - a walk whose root goes unlisted persists no `Abandoned` marks, no aggregates, no stamp, and emits `ScanAborted`;
  - a phase pass whose root goes unlisted stamps nothing, and `take_stock` stamps nothing;
  - `Abandoned` marks from an earlier session are frontier again after a `LocalExternal` start;
  - a delete batch then an unlisted root writes the marker (writer alive, and after drain), and emits the event once;
  - the route table: `needs_rebuild` wins over a completed index; `RebuildFirst` clears it with `user_enabled` intact;
  - lane: the vanish pin leaves no stamp, and a re-attach routes to a rebuild when deletes were in flight;
  - `pnpm check`, `pnpm check disk-images`, then `pnpm check --include-slow`.
  - **Must not change**: `scan_completion` tests (`scan_failure_is_vanished_volume`), `phases::tests`, `launch_route`
    tests (existing rows unchanged), `writer::abandoned_retry` tests, `cover::cold_drive_tests::*`.
- **DONE**: no completion claim for an unlisted volume; the rebuild survives quit and restart.
- **Docs**: `lifecycle/DETAILS.md` (gates, marker, route), `phases/DETAILS.md` (`take_stock`), `writer/DETAILS.md` (the
  marker), `events/DETAILS.md` (the variant).
- **Size**: 600–750 lines.

### M9: vanish causes and the index notice

- **Scope**: `unmount_approver/causes.rs`, the eject-approval registration, owner `Vanish` stops off the queue; the app
  mapping of `IndexNeedsFreshScan` to a toast, translated in every catalog.
- **Intentions**: as § "A vanished drive, host side".
- **Landmines**:
  - A raw `umount` racing Cmdr's own eject: the flight's resume must find the volume unlisted and do nothing.
  - The toast fires once per marker write, not per launch.
- **Test plan**: pure `causes.rs` (asked unmount, raw `umount`, eject then disappear, disappear with no eject, an
  overlong ask making it `Unknown`, re-appear); lane: `/sbin/umount` of an indexed image's volume → `Unasked`, index
  stopped, no resume; the toast's copy test; `pnpm check`, `pnpm check disk-images`, the i18n checks from M15's list.
  - **Must not change**: M6's tests.
- **DONE**: a raw `umount` and a vanished disk stop quietly and never resume.
- **Docs**: `volumes/DETAILS.md` (causes), `transports/DETAILS.md` (a vanished drive), the frontend indexing docs.
- **Size**: 400–500 lines plus keys.

### M10: transfers on a vanished drive

- **Scope**: `TransferSide` threaded into the local engine; presence-aware classification and the
  `DeviceDisconnected { path, side }` wire change at every listed site; `progress_at_stop` and move source counts;
  `flush_created_destinations` returning `Result` with directory fsyncs for moves; Phase 4 gating; the source-sweep
  presence checks; a `cfg(test)` chunk hook in `chunked_copy.rs` that parks after N bytes; the transfer copy,
  translated.
- **Intentions**: as § "A vanished drive, transfers"; name each operation's "what's left where" from typed facts only;
  check whether `sync_data` is `F_FULLFSYNC` on Apple targets and record it.
- **Landmines**:
  - A `NotFound` from a gone mount isn't a gone file: every "already gone" decision in `source_sweep.rs` asks the mount
    table.
  - `WriteErrorEvent` is wire: `pnpm bindings:regen`; the transfer copy's key family decides whether counts are ICU
    plurals.
  - The flush's directory fsyncs add latency on slow media; keep one fsync per distinct directory.
- **Test plan**:
  - unit: classification with a listed and an unlisted side for `ENOENT`, `EIO`, `ENXIO`, `EBADF`; Phase 4 skipped on a
    failed flush and on an unlisted destination; the sweep stops on an unlisted source;
  - frontend: `transfer-error-messages.test.ts` cases per direction and op; the gallery fixture;
  - lane: copy onto an HFS+ image parked by the chunk hook, `detach -force`, assert `DeviceDisconnected` with progress
    and the Mac source untouched, and record the errnos actually seen in the commit body; a Mac → image move
    force-detached between Phase 3 and Phase 4 keeps every Mac source;
  - `pnpm check`, `pnpm check disk-images`, then `pnpm check --include-slow`.
  - **Must not change**: `transfer/volume/copy_crashsafe_tests.rs`, `merge_case_fold_tests.rs` (updated for the field
    only), `transfer-error-messages.parity.test.ts`, `overwrite_tests.rs`.
- **DONE**: a pulled transfer reports how far it got, and a move never deletes sources it can't prove landed.
- **Docs**: `write_operations/transfer/DETAILS.md` (sides, flush, Phase 4), `write_operations/DETAILS.md`
  (classification), `apps/desktop/src/lib/file-operations/transfer/DETAILS.md`, `docs/guides/error-handling.md` if it
  lists the variant.
- **Size**: 850–1,000 lines.

### M11: temps, asides, and staging dirs

- **Scope**: the `A`/`a` record shapes; volume-homed temps on non-root mounts; the `discard_temp` fix with in-session
  pending; aside registration with kinds and the restore-or-recover sweep; staging-dir recognition, registration, and
  the empty-only sweep with its notice.
- **Intentions**: as § "A vanished drive, temps and asides", in that order.
- **Landmines**:
  - ❌ Never remove an aside whose destination isn't a regular file of exactly the recorded size; ❌ never a non-empty
    staging dir.
  - Every restore and recovery rename goes through `rename_no_replace`; a collision tries the next recovered name.
  - `is_one_of_ours` must accept staging dirs and asides with strict parsing, or `sweep_on_volume` retires them silently
    (`in_flight_temps.rs:490-493`).
  - The old `+`/`-` lines still replay.
- **Test plan**:
  - unit: an old-format reader skips `A` lines (a copy of today's `read_recorded` in the test); a local temp on a
    non-root mount defers at launch; `discard_temp` keeps the record when the root is gone; each aside kind's sweep arm
    (missing destination restores; exact-size file removes; short file, displaced file, and dir overwrite recover); a
    non-empty staging dir stays and warns; a strict-parse rejection of a look-alike name;
  - lane: a copy onto an HFS+ image parked mid-file, `detach -force`, re-attach the same image in the same session → the
    partial is swept; an overwrite parked mid-file, detached, re-attached → the original is back or recovered, never
    gone;
  - `pnpm check`, `pnpm check disk-images`.
  - **Must not change**: `in_flight_temps_tests.rs`, `overwrite_tests.rs`, `copy_crashsafe_tests.rs`, the `staging.rs`
    tests.
- **DONE**: nothing Cmdr left on a drive is forgotten, and no sweep can delete a user's original.
- **Docs**: `write_operations/DETAILS.md` (the ledger, record shapes, aside kinds), `file_system/DETAILS.md` § "Hiding
  transient scratch", `crates/cmdr-fs/DETAILS.md` (staging names).
- **Size**: 750–900 lines.

### M12: disk resolution, per-disk flights, and sibling safety

- **Scope**: `eject/disk_target.rs`, `eject/disk_flight.rs` (sibling ids adopted into `IN_FLIGHT`),
  `EjectStep::DiskResolve`, `disk: Option<&DiskTarget>` on `Teardown::Tool`, the detached settle after `TimedOut`, and
  `objc2-io-kit` as a direct dependency.
- **Intentions**: the order in § "Cmdr's own eject", each round-3 should-fix as its own tested behavior (resume after a
  settled timeout and after a partial `NotResponding`; resume through the gate; the NULL-resolution arms; fail closed; a
  sibling joins before `is_already_unmounted`).
- **Landmines**:
  - Resolution runs before the join, so two callers may each resolve (read-only, accepted).
  - A retry aimed at the original path after a partial unmount reads as done: aim at a still-listed captured path.
  - `volumes/disk_image.rs` stays on raw FFI (the DA teardown swap is deferred).
  - Register test volumes in the global `VolumeManager` under unique ids and remove them (`volume/DETAILS.md` § "Test
    isolation for the global `VolumeManager`").
- **Test plan**:
  - pure: sibling selection over a fake target and registry; the busy gate; adoption into `IN_FLIGHT` in both orders (B
    after A's adoption joins in `join_or_start`; B before it awaits the disk flight); the resume matrix; the NULL arms;
    the fail-closed `still_mounted`;
  - lane: M1's two sibling pins flipped to `UnmountRefused` with a resume; a two-volume container keys the physical
    whole and stops both before the teardown;
  - `pnpm check`, `pnpm check disk-images`, then `pnpm check --include-slow`.
  - **Must not change**: `in_flight::tests::*`, `unmount_tool::tests::*`, `eject::tests::*`, M5 and M6 tests.
- **DONE**: siblings are joined, gated, stopped, checked, and resumed.
- **Docs**: `volume/DETAILS.md` § "Eject" (delete the "Known gap" sentence), `volume/CLAUDE.md` (the eject bullet, no
  longer), `transports/DETAILS.md` § "Unmount/eject lifecycle", `apps/desktop/src-tauri/src/commands/DETAILS.md` if it
  describes eject.
- **Size**: 700–850 lines.

### M13: holder scan and wire type

- **Scope**: `eject/holders/{mod.rs, scan.rs}`, `merge`, `UnmountRefused { holders, detail }`, `VolumeHolder`,
  `Display`, the MCP `data`, bindings, and a frontend stub keeping today's `unmountRefused` copy.
- **Intentions**: scan once after the final refusal, disks and SMB, still-listed paths only, the `st_dev` before and
  after check, one abandonable thread, injected budget; Linux empty.
- **Landmines**: scanning inside the retry loop multiplies up to 9.4 s by the attempts; an SMB `stat` can hang, so the
  budget detaches the thread; `// SAFETY:` on the FFI; `specta` doc comments land in `bindings.ts`.
- **Test plan**: pure `merge` and the budget on a paused clock; a device change mid-scan discards the result; an
  unignored macOS test where a child holds a temp FILE and `proc_listpidspath` on that file path WITHOUT
  `PATH_IS_VOLUME` returns its PID inside the 8 s cap; lane: M1's held-file pin asserts the holder PID; `pnpm check`,
  `pnpm check disk-images`.
  - **Must not change**: `eject::tests::eject_error_crosses_the_wire_as_a_tagged_value` (updated for the field only),
    the `in_flight` and `unmount_tool` tests that build `UnmountRefused`.
- **DONE**: MCP replies carry holders in `data`; bindings regenerated.
- **Docs**: `volume/DETAILS.md` § "Eject", `mcp/DETAILS.md` (the `eject` tool's `data`),
  `docs/guides/error-handling.md`.
- **Size**: 300–400 lines.

### M14: holder facts and classification

- **Scope**: `eject/holders/{facts.rs, nested_images.rs}`, `classify`, the responsible-PID `dlsym`, and the Security
  extern for the platform identifier and Info.plist.
- **Intentions**: the facts order in § "Holders"; a `classify` table for `lsd`, Finder, zsh under Warp, the WebKit
  helper with and without the responsible symbol, Google Drive's nil-`NSRunningApplication` responsible app, an
  accessory app, an orphaned `sleep`, a third-party daemon, an executable on the target volume, self, and a descendant
  of self; the nested-image signal picked and recorded in § "Spike results" with an evidence anchor; facts past the
  budget stay `Unclassified`.
- **Landmines**: a Security call against an executable on the volume makes Cmdr the holder; `NSRunningApplication`
  without an autorelease pool leaks; release every `SecCode` and CF reference; ❌ not `csops`; ❌ no path-prefix test.
- **Test plan**: pure `classify` over recorded facts; lane: a binary copied onto the image and run from it names `Tool`;
  a nested image stored on the outer image names `DiskImage`; `pnpm check`, `pnpm check disk-images`, then
  `pnpm check --include-slow`.
  - **Must not change**: M13's tests.
- **DONE**: kinds filled in.
- **Docs**: `volume/DETAILS.md` § "Eject" (classification, accepted tradeoffs).
- **Size**: 450–550 lines.

### M15: eject copy in every catalog

- **Scope**: `wordUnmountRefusal`, `formatConjunctionList`, the six approved keys with `@key` descriptions, translations
  per `docs/guides/i18n-translation.md` (ten full locales; `en-GB`/`en-AU` only where wording differs).
- **Intentions**: the precedence in § "Copy drafts"; each `@key` names the toast surface, says it follows "Couldn't
  eject {volumeName}: ", explains the tokens, marks Cmdr and macOS as names, and for `otherApps` says it's the last list
  item.
- **Landmines**: a raw family fails `desktop-i18n-icu` on a doubled apostrophe; tokens stay verbatim for
  `desktop-i18n-parity`; a key with no call site fails `desktop-message-keys-unused`; run `pnpm intl:keys` and
  `node apps/desktop/scripts/sync-locale-keys.ts`; fold the list formatter into `apps/desktop/src/lib/intl/CLAUDE.md`'s
  number-format bullet.
- **Test plan**: `eject-error-messages.test.ts` (one, two, three, four, and six apps; `DiskImage`; `Cmdr`; `System`;
  mixed; empty; only `Unclassified`); a list-formatter test with a pinned locale; `pnpm check svelte` plus
  `desktop-i18n-icu`, `desktop-i18n-parity`, `desktop-i18n-coverage`, `desktop-i18n-term-consistency`,
  `desktop-message-keys-fresh`, `desktop-message-keys-unused`.
  - **Must not change**: the existing `eject-error-messages.test.ts` cases.
- **DONE**: every locale carries the keys.
- **Docs**: `apps/desktop/src/lib/file-explorer/navigation/DETAILS.md` § "A refusal speaks the catalog", its `CLAUDE.md`
  line on `wordEjectRefusal`, `apps/desktop/src/lib/intl/CLAUDE.md`.
- **Size**: 150–200 lines plus six keys across the catalogs.

### Release checkpoint

- **Close-out sweep**: a conformance pass against § "Invariants"; read every commit body and every touched `CLAUDE.md`;
  `pnpm check docs-dead-links docs-reachable docs-link-text docs-section-refs claude-md-length resident-doc-budget oxfmt`;
  `pnpm check --include-slow`.
- **Manual QA list for David** (none of it automated): an APFS USB stick and an exFAT one ejected from Finder while Cmdr
  indexes them; two indexed sticks ejected together from Finder; an SD card; a two-partition drive; a DMG opened from
  Finder; an app launched from its DMG; Preview holding a file; a terminal `cd`'d into the drive; a copy and a move to a
  stick with the cable pulled mid-file, then re-plugged; an overwrite on a stick pulled mid-file; a signed release
  build.
- **Then**: FF-merge to David's local `main`; move this plan to "Shipped, kept for review" in `docs/specs/index.md`.

## Rollback

- Each milestone lands as its own commits and reverts with `git revert`.
- Persistent shapes: the `index_needs_rebuild` meta key (an older build ignores it and routes as today), the `A`/`a`
  ledger records (an older build ignores them and forgets them at launch, deleting nothing), and wire types
  (`WriteErrorEvent`, `UnmountRefused`; regenerate bindings after a revert).
- Reverting M6 brings back the unconditional `WillUnmount` handler; M5's gate stays harmless without it.
- M7, M8, M10, and M11 revert independently. M12 reverts alone as long as M13's scan falls back to this volume's root
  when `disk` is `None`. M13, M14, and M15 revert together.

## Invariants

The conformance register for the checkpoint.

1. No classification by any string.
2. Before a DA-mediated unmount of a volume Cmdr indexes, an ask stops every volume on that whole unit within the
   chain's shared deadline, or dissents. Bounds: DA ignores the dissent under force, and several indexed drives ejected
   together share one chain.
3. `Released` means no Cmdr index worker of a live generation holds the volume; every drive-reading spawn takes
   `VolumeWork`; a vanished generation never blocks a later one.
4. Every Cmdr-made start of a removable index passes the gate: epoch unchanged, no newer ask on its whole disk, still
   listed, intent says run, not ejected by another owner, after `RESUME_SETTLE`. A disable is serialized with it.
5. An `Unasked`, `Pulled`, or `Unknown` volume is never resumed.
6. On a local-scanner volume, no row is deleted from an incomplete listing, a non-`NotFound` error, or an unlisted
   volume; no `Abandoned` mark, aggregate, or completion claim is written for an unlisted volume.
7. Deletes that may have come from a leaving drive persist `index_needs_rebuild`, which routes the next start to a
   rebuild with intent markers kept, announced once.
8. Mac sources are deleted only after a flush that returned `Ok` and a destination still listed.
9. A temp, aside, or staging dir on a drive stays recorded until its handling is observed on a listed mount; an aside is
   removed only against an exact-size regular file, otherwise restored or recovered; a non-empty staging dir is never
   removed.
10. Every registered volume on the physical disk passes the busy gate and has its index stopped inside one deadline
    before Cmdr's own teardown; a sibling's eject joins the disk flight in `join_or_start`.
11. Success needs every captured sibling path gone and a fresh `mounted_volumes_on` empty.
12. Nothing unmounts after resolution, the ejectability check, or an index stop fails or stalls.
13. Holders are captured once per eject, after the last attempt, over still-listed paths with an unchanged root device,
    within the injected budget.
14. No code-signing query runs against a process whose executable is on the target volume.
15. Private symbols come from `dlsym`; their absence degrades, never fails. An approver that can't install leaves the
    `WillUnmount` handler in place.
16. Every user-facing word is catalog copy in all 13 catalogs.
17. Tests and probes: synthetic APFS/HFS+ only, through the guarded runner (lock, identity, SIGKILL).
18. SMB keeps `diskutil unmount` and Linux keeps `umount`; neither gets an approver.

## Deferred

- **The DA teardown swap** (Whole-unmount each synthesized container, then the physical whole, then `DADiskEject`, with
  typed statuses and DA's root-visible dissenter PID). The full design is in git:
  `docs/specs/eject-diskarbitration-plan.md` at `15292aa2b`, § "The DA teardown (M5)" and § "Statuses". **Revisit when**
  the eject `warn` lines show refusals with no nameable holder often enough to matter, or a report shows `diskutil`
  answering a partial unmount this plan's fail-closed check can't classify.
- **"Unmounted but not powered down"** stays a silent `Ok` with an `info` line. **Revisit when** a person reports a
  drive still powered after Cmdr's eject, or the DA teardown lands.
- **Linux unmounts don't stop an index** (`volumes_linux/watcher.rs:398-478`). The rebuild marker covers correctness;
  **revisit when** Linux builds ship (`docs/specs/later/linux-builds-plan.md`).
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
  refusal; the trigger is DA's idle callback while the volume is still mounted.
  - ❗ Today's `handle_volume_will_unmount` (`apps/desktop/src-tauri/src/volumes/watcher.rs`) stops the index, and
    nothing resumes it when that unmount is refused.
- **`WillUnmount` is posted synchronously from AppKit's own DA approval callback, on the main thread.** Today's handler
  is racy only because it spawns a thread and returns. Blocking there would hold the unmount, but it freezes Cmdr's main
  thread and meets the same 10 s limit, so a dedicated DA session on its own queue is the better home.
- **Dissent works on non-force requests, and each caller reports it differently.** `diskutil` names the dissenting PID
  and its parent; the DA API answers `0xF8DA0002` with the dissenter's PID; NSWorkspace throws a bare `OSStatus` -47 and
  can leave a multi-partition disk partly unmounted.
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
- **Unverified by measurement: a MOUNTED volume vanishing** (a real pulled cable). The source reading in § "Evidence the
  design rests on" says the daemon's own force unmount skips approval and no eject approval is sent. Not run: killing
  the helper under a mounted filesystem is a new kernel-level risk.

#### 6. Refused-unmount settle signal

- **A holder process with the H1 volume root open**, so the kernel answers EBUSY:
  - `diskutil unmount`: the idle callback came 53 ms after DA logged the failure, and `diskutil` exited 81 ms after
    that; its stderr named the holder PID.
  - DA API unmount: status `0x0000C010` with the holder as dissenter PID, in 132 ms; idle 1 ms after the requester's
    callback.
  - `unmountDisk` with H1 held: H2 unmounted, H1 refused (partial), idle after each.
- **A refusal brings no description change and no `DidUnmount`**, but `WillUnmount` was posted for it.
- **Idle means "DA's queue is quiet"** and also fires after successes and between separate requests, so pair it with the
  volume's current mount state. It's private (`DiskArbitrationPrivate.h:331`, `DARegisterIdleCallback` through `dlsym`);
  no public callback marks a refusal to an observer.
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
- **Not run** (new risk, or M14 code): the nested-image signal, `diskimagesiod` holding an outer volume, and the
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
