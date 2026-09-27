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
