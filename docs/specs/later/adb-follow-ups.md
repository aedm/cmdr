# ADB follow-ups

Android over ADB is shipped: `crates/cmdr-adb` (the wire, the `Volume` answers, the error policy, and its own deferrals
in § "Known gaps and follow-ups": `crates/cmdr-adb/DETAILS.md`), the app side
(`apps/desktop/src-tauri/src/adb/DETAILS.md`, including the wireless-pairing non-goal), and the frontend
(`apps/desktop/src/lib/adb/DETAILS.md`). What's open is below; each item stands alone.

## 1. Finish the real-device pass

- **Problem**: the backend was built against the in-repo fake ADB server. Part of it has since met a real phone
  (listing, `df`, and `stat` on a Pixel 9 Pro XL with Android 17, 2026-09-10, anchored in `crates/cmdr-adb/DETAILS.md`),
  but several cases have never been observed on hardware: the authorize prompt, an `unauthorized` → `device` transition
  mid-session, a 2 GB `RECV` and `SEND`, a `/data` listing on a non-rooted phone (expected: `PermissionDenied` carrying
  the path), `test -w /` exiting 1, and a drive-index walk of a real `/sdcard` (how long it takes, whether browsing
  stays responsive under the eight-listing ceiling, and an unplug mid-walk).
- **Impact**: a fake server agrees with whatever the crate believes about framing and state transitions, so the first
  real device is where a wrong belief surfaces. Until this runs, those paths are "works against our own mock". Item 2 is
  blocked on it.
- **Solution**: an Android phone with USB debugging on, plugged into the Mac, and the cases above walked by hand. Record
  what comes back in `crates/cmdr-adb/DETAILS.md` with the usual evidence anchor (device, Android version, date), and
  drop the "Real-device pass pending" bullet there once it's all observed. The E2E spec
  (`apps/desktop/test/e2e-playwright/adb.spec.ts`) cites this item for the by-hand part.
- **Size**: S, an afternoon with a phone on the desk. Blocked on David and hardware.

## 2. Measure `sendrecv_v2` compression before enabling it

- **Problem**: the sync service's v2 packets can carry brotli, lz4, or zstd, and the crate sends none of them.
- **Impact**: unknown, which is the point. The DEVICE does the compressing, so a phone with a busy CPU may transfer a
  compressible tree slower with a flag on than off, while a USB 2 cable with an idle CPU may go much faster. Guessing
  either way is how a backend gets a throughput regression nobody can attribute.
- **Solution**: on real hardware, time the three algorithms against a compressible tree and an already-compressed one
  (photos), enable whatever wins (or nothing), and write the numbers into `docs/notes/`, linked from
  `crates/cmdr-adb/DETAILS.md` § "Known gaps and follow-ups".
- **Size**: S, half a day of measuring; the flag itself is a few lines. Blocked on item 1.

## 3. One switcher row per phone, not one per protocol

- **Problem**: a phone plugged into a Mac with platform-tools installed is visible to Cmdr TWICE, over MTP and over ADB,
  so the switcher shows two rows for one object on the desk. "Pixel 9" versus "Pixel 9 (ADB)" asks the user to pick a
  protocol before they have a question, and it's not a choice anyone outside this repo can make. The shipped stopgap is
  a label: `apps/desktop/src/lib/adb/adb-volume-label.ts` adds "(ADB)" only when an MTP row shares the name.
- **Impact**: every Android developer (the audience ADB exists for) sees a doubled phone. And it's a pattern every
  future device backend would copy.
- **Solution** (decided): the switcher shows **one row per physical device**, matched by serial, which both sides
  already have (`cmdr_fs::volume::mtp_ids::device_id_for` prefers it; `cmdr_fs::volume::adb_volume_id` is minted from
  it). MTP is the default face, since it needs no developer mode and covers what most people want; ADB is a mode the row
  switches into, from its context menu and from a pane-header control, "Show the full filesystem". A phone seen by only
  one protocol is simply that row, unlabelled. It costs:
  - A cross-provider identity pass in `apps/desktop/src-tauri/src/device_volumes.rs` that folds entries by serial before
    handing the listing out. The provider registry is the right seam; what it lacks is two providers answering for one
    device.
  - A per-row active protocol the pane remembers, plus the header control that switches it.
  - Deleting `adb-volume-label.ts`, the `deviceVolumeLabel(volume, volumes)` call in
    `apps/desktop/src/lib/file-explorer/navigation/VolumeChooserMenu.svelte`, and the `adb.volumeLabelWithSuffix` key
    with its translations, in the same commit.

  What it doesn't change: **readiness stays per protocol** (a phone can be `ready` over MTP and
  `waiting_for_authorization` over ADB at once, and the merged row has to say which face it describes;
  `apps/desktop/src/lib/adb/DETAILS.md` § "What each readiness makes of a row" is the rule it inherits), and **the
  pane's connect seam** (`device-connect.svelte.ts` keys on the volume's own id and path, so a merged row still hands
  the pane one of the two ids).

- **Size**: L, the largest single frontend item left from the ADB work. Nothing else waits on it.
