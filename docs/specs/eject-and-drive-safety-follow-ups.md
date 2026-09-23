# Eject and drive safety: follow-ups

The eject and drive-safety work shipped in v0.46.0: moves keep their sources until the destination is durable, a
DiskArbitration unmount approver lets go of every volume of a disk before any DA-mediated unmount (and hands the index
back after a refusal), every drive-reading index worker carries a share of `VolumeHold`, a vanished drive stops its
index quietly and marks it for a rebuild, transfers and leftovers survive a pulled cable, Cmdr's own eject works per
physical disk, and a refusal names who held the drive. Where it lives: `apps/desktop/src-tauri/src/volumes/DETAILS.md` §
"The unmount approver", `apps/desktop/src-tauri/src/file_system/volume/DETAILS.md` § "Eject" and § "One release, one
start", `crates/cmdr-index/src/indexing/reconcile/DETAILS.md` § "The delete gates", the write-operations `DETAILS.md`
(durability, leftovers), and `apps/desktop/src/lib/file-explorer/navigation/DETAILS.md` § "A refused unmount names who
held the drive". The measured DiskArbitration evidence: `docs/notes/diskarbitration-unmount-evidence-2026-09.md`.

What's left is one manual QA pass, one copy decision, and three deferred designs, each with its revisit trigger.

## 1. Manual QA of eject and drive safety on real drives

- **Problem**: every automated test of this work runs on synthetic APFS and HFS+ disk images, by rule: a FAT32 SD card
  unmounted under a live FSEvents stream once wedged macOS's FSKit `msdos` service and kernel-panicked a Mac
  (2026-07-15), so no test touches a physical disk or a FAT/exFAT image. Real sticks, SD cards, pulled cables, and a
  signed build have never been exercised end to end.
- **Impact**: the scenarios the work exists for (a Finder eject of an indexed exFAT stick, a cable pulled mid-move) are
  exactly the untested ones. One of them, a MOUNTED volume vanishing under a real pull, is also unverified at the
  DiskArbitration level (the spike only measured a disk whose volume was already unmounted).
- **Solution**: David runs this list on a signed release build, with Cmdr indexing each drive:
  1. An APFS USB stick and an exFAT one, each ejected from Finder.
  2. Two indexed sticks ejected together from Finder.
  3. Indexing turned on for a stick while Finder ejects it.
  4. An SD card.
  5. A two-partition drive (eject one partition; both must go, or the eject must say why not).
  6. A DMG opened from Finder, and an app launched from its DMG, each holding the drive.
  7. Preview holding a file on the drive; a terminal `cd`'d into the drive.
  8. A copy and a move to a stick with the cable pulled mid-file, then re-plugged (leftovers swept, originals intact).
  9. A move to a stick with the cable pulled right after the progress bar finishes (sources must still be there, or the
     destination must be complete).
  10. An overwrite on a stick pulled mid-file (the original survives, restored or kept as "(recovered)").

  What to read, since every refusal toast now names its holder: check the SENTENCE as well as the outcome. Preview
  should read "Preview is still using this drive…"; a DMG opened from Finder should send you to eject the image first; a
  terminal names the terminal APP (responsibility is inherited). "Something is still using this drive" means either
  nothing was nameable or every holder came back `Unclassified`; the `eject` log line tells them apart. ❗ A `warn`
  saying Cmdr itself held the drive is a bug, not a copy issue.
- **Size**: S (about an hour of David's time, plus fixes for whatever it finds).
- **Blocked on**: nothing. Needs David and real hardware; an agent can't run it.

## 2. Words for a refused eject whose holders are all unclassified

- **Problem**: when every holder the scan named is `HolderKind::Unclassified` (the holder budget ran out, or a
  signature wouldn't read), the toast falls through to the generic `errors.eject.unmountRefused` ("Something is still
  using this drive"), exactly like a scan that named nobody. The names and pids ARE on the wire in `HolderScan`.
- **Impact**: a person gets no hint where a hint exists. Rare in practice (the budget is 1.5 s and most holders
  classify), but it's the least helpful sentence in the set.
- **Solution**: a product decision first. Options: (a) keep today's fallback; (b) a new key along the lines of "Cmdr
  couldn't tell which app, but {count} processes are using this drive"; (c) name the executables anyway ("{names} are
  still using this drive"), accepting that an executable name can be cryptic (`mds_stores`). Whatever is picked lands
  in `wordUnmountRefusal` (`apps/desktop/src/lib/file-explorer/navigation/eject-error-messages.ts`) as a new arm before
  the fallback, with an `@key` description, a translator pass for the ten locales, and a test case in
  `eject-error-messages.test.ts`. ❌ Never word `Unclassified` as an app or a tool, and ❌ never say the drive is free
  unless the scan is `Complete` with nobody named.
- **Size**: S.
- **Blocked on**: a David decision on the copy.

## 3. Per-disk DiskArbitration sessions for the unmount approver

- **Problem**: the approver runs one DA session on one serial queue, and DA times each approval ask from when it QUEUED
  it (10 s), so asks queued behind each other share one time-based chain with a 7 s stop budget
  (`volumes/DETAILS.md` § "The unmount approver"). When several indexed drives are ejected together and that budget
  runs out, the later asks dissent with their stops detached. On a non-force request that's an honest refusal; under
  force (`diskutil unmount force`, `hdiutil detach -force`) DA ignores the dissent and unmounts under a live FSEvents
  watcher.
- **Impact**: the FSKit wedge exposure (kernel panic on macOS 26, observed 2026-07-15) remains for a multi-drive FORCE
  eject. The delete gates and the rebuild marker protect the index rows; nothing protects the kernel.
- **Solution**: one DA session per physical disk, each with a match dictionary for its own disk (created on
  `Appeared`, released on `Disappeared`), so one disk's slow stop never spends another disk's DA window. Keep the
  shared-deadline logic per session. Check first whether AppKit's own DA session filters by disk (unverified; it
  decides whether a slow `WillUnmount` observer anywhere delays other disks too): evidence and method in
  `docs/notes/diskarbitration-unmount-evidence-2026-09.md` § "`NSWorkspaceWillUnmountNotification`".
- **Size**: L.
- **Blocked on**: a trigger. Revisit when a log shows an ask answered past its chain deadline under force, or a wedge
  report involves a multi-drive eject. Until then the residual is documented and accepted.

## 4. Eject through DiskArbitration directly (the DA teardown swap)

- **Problem**: Cmdr's own eject tears a disk down with `diskutil eject`, which reports a refusal as stderr text and a
  nonzero exit. The holder scan (`proc_listpidspath`) sees same-uid processes only, so a root-owned holder (`backupd`,
  a system daemon) leaves a refusal that names nobody. And "unmounted but not powered down" can't be told apart from a
  clean eject, so it's a silent `Ok` (`file_system/volume/DETAILS.md` § "Eject").
- **Impact**: some refusals say "Something is still using this drive" when DA itself knows the dissenter's pid, and a
  drive left powered on reads as ejected.
- **Solution**: replace `diskutil eject` in the per-disk flight with DA calls on the flight's own session and serial
  queue: `DADiskUnmount(container, Whole)` for each synthesized APFS container, then `DADiskUnmount(physical_whole,
  Whole)` (a dissent stops the attempt), then a fresh mount-table gate (anything still listed → still mounted, never
  eject), then `DADiskEject(physical_whole)`. Map statuses typed, never by string: `0x0000C010` (EBUSY) and
  `0xF8DA0002` → busy, `0xF8DA0007` not mounted, `0xF8DA0006` not found, `0xF8DA0008` not permitted, `0xF8DA0009` not
  privileged; a dissent carries `DADissenterGetProcessID` (through `dlsym`), which sees root holders too. A failed
  `DADiskEject` after every volume left the table becomes a typed `UnmountedNotPoweredDown` (its wire mapping is a
  David decision). Bridge each request with a boxed `oneshot::Sender` reclaimed exactly once by the callback, awaited
  under a deadline, with `catch_unwind` in the callback; a daemon restart drops a request with no callback, which leaks
  one box and answers `TimedOut`. The retry loop (`settle_with_retries`) becomes generic over the outcome. The full
  earlier design, with the status table and the settle rules per stage, is in git: `docs/specs/eject-diskarbitration-plan.md`
  at `959670562`, § "The DA teardown (M5)" and § "Statuses".
- **Size**: L.
- **Blocked on**: a trigger. Revisit when the eject `warn` lines show refusals with no nameable holder often enough to
  matter, or a report shows `diskutil` answering a partial unmount the fail-closed check can't classify, or someone
  reports a drive still powered after Cmdr's eject.

## 5. Linux: an unmount from outside Cmdr stops the drive's index

- **Problem**: on Linux, Cmdr's own eject stops a drive's index through the `drive_release` gate before `umount` runs,
  but an unmount from anywhere else (a file manager, `umount`, a pulled cable) never stops it: the mount-table watcher
  (`apps/desktop/src-tauri/src/volumes_linux/watcher.rs`) unregisters the volume and emits `volume-unmounted`, and
  nothing tells the index. There's no pre-unmount hook on Linux at all.
- **Impact**: the index instance keeps its inotify watches and SQLite handles on a mount that's gone until the app
  quits. Correctness is covered: the delete gates refuse deletes from a gone drive and the persisted rebuild marker
  routes the next start to a rebuild. The cost is resources and log noise, not data.
- **Solution**: on the Linux watcher's unmount path, stop a `LocalExternal` volume's index through the `drive_release`
  gate, never resumed, the way macOS's `DidUnmount` cleanup does
  (`apps/desktop/src-tauri/src/volumes/watcher.rs`, `handle_volume_unmounted` → `stop_local_external_index_off_main`).
  Optionally
  a udisks2 D-Bus hook for a pre-unmount stop, which would be Linux's closest analogue to the approver.
- **Size**: M.
- **Blocked on**: Linux builds shipping (`docs/specs/later/linux-builds-plan.md`).
