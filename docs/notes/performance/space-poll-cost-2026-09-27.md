# The space poller's free-space query: what it cost and what the cheap reading saves

The space poller read the boot volume's free space every 2 s with `NSURLVolumeAvailableCapacityForImportantUsageKey`,
all the time (the low-space check), plus each other local volume on screen. That query is the idle census's lever 1
(`idle-census-2026-09-27.md` § "The levers"). The poller now reads it through `volumes::live_volume_space`: the same
figure, anchored on an occasional important-usage query and moved by `statfs` in between. The mechanism lives in
`apps/desktop/src-tauri/src/volumes/DETAILS.md` § "Live volume space"; this note holds the evidence.

## Is `statfs` a trustworthy change detector?

A Swift probe sampled all three figures for `/` every 2 s for 5 min (149 samples), each through a fresh `URL` so
Foundation's per-object resource cache couldn't answer (verified on macOS 27.0, David's M3 MacBook Pro under sibling
agents' build load, 2026-09-27):

- **`statfs` free (`f_bavail × f_bsize`) equals `NSURLVolumeAvailableCapacityKey`** to the byte in 147 of 149 samples;
  the other two differed by 94 KB and 64 KB, writes landing between the two calls. The plain key is `statfs` behind an
  Objective-C call.
- **Purgeable space (important-usage free minus `statfs` free) held at 10.692 GB** for the whole run, while `statfs`
  free fell by 3.3 GB (345.32 → 341.98 GB) under builds. One sample read 10.682 GB and the next was back at 10.692.
- **Anchor plus delta tracks the real figure**: the first sample's important-usage figure plus how far `statfs` moved
  since stayed within **0.4 MB** of the real important-usage figure in 148 of 149 samples, 5 min and 3.3 GB later. The
  one outlier (10.1 MB) is the same sample as above, where a write landed inside the ~10 ms the important-usage call
  took after `statfs`.
- **Wall time per call**: important-usage median **9.9 ms** (p10 7.1, p90 22.2; the first call 20.6), the plain key
  0.08 ms, `statfs` 0.015 ms. Wall time, not CPU; the census's 6.5 ms is CPU (`idle-census-2026-09-27.md` lever 1).
- No Time Machine local snapshots existed on the machine (`tmutil listlocalsnapshots`), so the probe couldn't show the
  case where `statfs` misses a change: deleting a file a snapshot still holds leaves `statfs` free where it was while
  the important-usage figure grows. The design covers it without a measurement (a refresh after every write operation
  and at most every 60 s); creating a snapshot on David's machine to prove it wasn't worth the side effect.

So `statfs` catches every write and delete that frees or takes real blocks, and the part it can't see (purgeable) moves
rarely. That's why the design derives the figure instead of only using `statfs` to decide when to re-query: between
re-queries, a live reading is exact for writes rather than frozen at the last query.

## Before and after, idle

### Method

- **Binaries**: release builds (`pnpm tauri build --no-bundle --target aarch64-apple-darwin`) of `59854810f` (base) and
  of the branch (after), run bare with their own `CMDR_INSTANCE_ID`, `CMDR_DATA_DIR`, and `CMDR_SECRET_STORE=file`.
  Isolation checked with `lsof` each round: no file under prod's data dir open, no TCP listener.
- **Data**: a fresh data dir per round, cloned from one pristine `settings.json`: indexing, media indexing, network
  discovery (`network.enabled`), direct SMB, MCP, analytics, crash and error reports, update checks, Ask Cmdr, and the
  global shortcut off. The window open on the default pane (`~`, on the boot volume), low-space warning on (default).
  So the poller reads one volume, the boot one, every 2 s.
- **Protocol**: four interleaved rounds (base, after, base, after, …), each a fresh launch; 60 s to settle, then a 240 s
  idle window.
- **Instrument**: per-thread CPU with names from `proc_pidinfo(PROC_PIDTHREADINFO)` at both ends of the window, summed
  by `pth_name`, plus `ps -o time` for the process and for the `deleted` daemon (the census recipe,
  `idle-census-2026-09-27.md` § "A better CPU instrument").
- **Machine**: David's M3 Max MacBook Pro, macOS 27.0, load average 2–23 from sibling agents' builds.

### Results

Verified on release builds, `proc_pidinfo` per-thread CPU deltas over 240 s, 2026-09-27. % of one core:

- **Whole process, base**: 1.64, 2.85, 1.94, and 1.77 s per 240 s. Median **0.77%** (0.68–1.19%).
- **Whole process, after**: 0.40, 0.32, 0.37, and 0.32 s. Median **0.14%** (0.13–0.17%).
- **`tokio-rt-worker`, where the poll runs**: base 1,322, 2,540, 1,668, and 1,467 ms, median **0.65%**, 88–90% of it
  system time; after 121, 62, 95, and 156 ms, median **0.045%**. **Saving: ~0.61% of a core**, 93% of the workers'
  time, and every after round sat below every base round.
- **Per poll, base**: 11–21 ms of CPU (the worker time over 120 polls), about twice the census's 6.5 ms. The census
  measured one key in a standalone Swift loop; the app reads two keys in two `getResourceValue` calls on a busy
  machine. Which of those accounts for the gap isn't isolated.
- **What's left after**: the notify-rs debouncer and FSEvents loops (the pane's watch on `~`, 80–320 ms) are now the
  largest item. Of the workers' ~100 ms, the four 60 s anchor refreshes account for roughly half at base's per-poll
  cost; stretching `MAX_ANCHOR_AGE` to 5 min would save ~0.02% more, bought with staler purgeable space.
- **The `deleted` daemon**: 3.07, 0.47, 0.00, and 0.06 s per round under base against 2.12, 2.70, 0.00, and 0.00 under
  after. No relation to the poll shows up; its CPU is driven by other clients, and this read can't separate Cmdr's
  share. The census's "plus work in `deleted`" stays unconfirmed.

In the targets' terms: with indexing and network off, the main process went from ~0.8% of a core to ~0.14% at idle on
this machine, the space poll having been most of what was left. The census's indexing-on figure (2.2%) is dominated by
the index writer and live-event loop, which this doesn't touch.
