# Debugging Cmdr's memory

Start here for any "Cmdr is using too much RAM" report. Read the trap section before you measure anything: getting it
wrong has cost multi-day investigations.

## The trap: `vmmap` reports Cmdr's Rust heap as `IOAccelerator`

`vmmap` names VM regions by their VM tag. macOS defines `VM_MEMORY_IOACCELERATOR = 100`
(`$(xcrun --show-sdk-path)/usr/include/mach/vm_statistics.h`), and **mimalloc tags every arena it `mmap`s with `os_tag`
= 100 by default**. Cmdr's Rust global allocator is mimalloc (`apps/desktop/src-tauri/src/main.rs`), so:

> **The `IOAccelerator` rows in Cmdr's `vmmap` / `footprint` output ARE the Rust heap** — not GPU memory, not WebKit,
> not the compositor. Arenas are reserved in 128 MB chunks, so the region COUNT grows in 128 MB steps.

The mirror-image trap: **`MALLOC_*` / `DefaultMallocZone` rows are NOT Cmdr's heap.** `malloc_zone_statistics` and
`malloc_get_all_zones` only see registered system zones, and mimalloc isn't one. A snapshot reading "malloc heap 1.6 GB"
while `phys_footprint` is 16.5 GB is not a contradiction — it means ~15 GB of Rust heap is invisible to that API.

Consequences worth internalising, because each one burned a day:

- A backend heap runaway **looks like a GPU/compositor leak**. If you find yourself bisecting CSS, layer promotion, DOM
  churn, or event volume because "the compositor is leaking", stop and re-read this section.
- Real WebKit compositor memory in Cmdr is small: measured at **35.6 MB in 10 allocations** during a climb where the
  process peaked at 646 MB. WebKit's helper processes (`com.apple.WebKit.GPU`, `…WebContent`) hold ~0 `IOAccelerator`;
  if the number is big and it's in the Cmdr process, it's Rust.
- "The balloon popped back on its own" is usually **mimalloc decommitting pages**, not macOS purging GPU surfaces. The
  arena regions stay mapped, so the region count doesn't drop even though dirty bytes collapse.

## How to measure

```
PID=$(pgrep -x Cmdr | head -1)
vmmap -summary "$PID" | grep -E "Physical footprint:|^IOAccelerator |^Malloc "
```

⚠️ **The tags are `Malloc Small` / `Malloc Large`, not `MALLOC_SMALL` / `MALLOC_LARGE`** (verified on macOS 27.0 /
26A428, 2026-09-22). The underscored spelling matches nothing and reads as a clean zero, which is how a baseline once
lost 66 MB of system malloc. The tag also contains a space, so `$4` is no longer the DIRTY column on those rows: slice
the tag by column (it ends at char 32) rather than splitting on whitespace.

`phys_footprint` is the honest total (what Activity Monitor's "Memory" shows and what jetsam keys on). `ps`/RSS lies
here — it keeps counting regions long after `phys_footprint` collapses. Read the DIRTY column (col 4), not VIRTUAL or
RESIDENT.

For a series rather than one reading, `apps/desktop/scripts/mem-sample.sh <label>` runs that recipe and appends a CSV
row (footprint, peak, Rust heap dirty/swapped, region count, uptime, and the mimalloc env the process inherited).
`--watch <label> [mins]` samples on a timer. It targets the `/Applications` build, so a dev build running beside it
can't be sampled by accident.

Per-line RAM in the app's own log: launch with `CMDR_LOG_RAM_USE=1` (see `logging.md`), which makes every log line carry
the current footprint — the cheapest way to correlate a climb with what the backend was doing.

## How to name an anonymous block (start here)

A tag total tells you the size. The **per-tag histogram of distinct region sizes** tells you the shape, and the shape is
what names things: macOS gives every allocation past its 127 KB large-zone threshold a VM region sized to the request,
so a repeated exact size is a fingerprint of whatever asked for those bytes.

Against a live app, any build, no app support needed:

```
PID=$(pgrep -x Cmdr | head -1)
vmmap "$PID" | awk '/^Malloc Large / && /\[/ { sub(/.*\[ */, ""); print $1 }' | sort | uniq -c | sort -rn | head -12
```

Same tag-spelling caveat as above, plus a column one: a detail line is
`<tag> <start>-<end> [ VSIZE RSIZE DIRTY SWAP] …`, and a tag with a space in it (`Malloc Large`) shifts every `$N` by
one. Cutting at the `[` instead of counting fields works for any tag. Swap in `IOAccelerator` to fingerprint the Rust
heap the same way.

Known fingerprints so far:

- **`96.5M`** (101,187,584 bytes) — the CLIP text tower's `49,408 × 512` fp32 token embedding. If it's there, the CLIP
  towers are loaded and cost 307–412 MB of `Malloc Large` plus 120–176 MB of `Malloc Small` for the process's whole
  life. Expect `4096K`, `3072K`, and `2304K` in the dozens beside it.
  `docs/notes/idle-malloc-large-clip-towers-2026-08-21.md`.
- **`128.0M` under `IOAccelerator`** — a mimalloc arena. Seven of them in a 25 h prod session (2026-09-22). The count is
  how many arenas the heap has grown to, and it only ever goes up: arenas stay mapped after mimalloc decommits the pages
  inside them, so a flat region count alongside collapsing dirty bytes is normal, not a leak.

From the app itself — any build, a shipped release included, which is deliberate because that's the only condition the
interesting numbers appear under — `get_memory_diagnostics(sizesPerTag)` returns the same histogram as structured data
plus the footprint and BOTH allocators' own accounting in one payload — the only reading that spans mimalloc and the
system zones at once. It also carries `sqlitePageCache`, the 64 MiB page slab every store's cached pages come from: that
slab is a leaked Rust allocation, so it hides inside the mimalloc total, and without this field you'd have to know to go
ask SQLite about it. It's an ordinary IPC command (`apps/desktop/src-tauri/src/commands/memory_diagnostics.rs`), macOS
only, and its module docs say how to read the payload.

## How to attribute (which code allocates)

```
MallocStackLogging=1 MallocStackLoggingNoCompact=1 <launch the app>
vmmap -fullStacks "$PID"        # allocation backtrace per VM region (confirms mimalloc vs anything else)
malloc_history "$PID" -allBySize # biggest live allocations with stacks
```

`malloc_history` only sees system-zone allocations, so mimalloc hides Rust allocations from it. To attribute a Rust heap
problem, temporarily comment out the `#[global_allocator]` in `main.rs` and rebuild: the growth reappears as `MALLOC_*`
and `malloc_history` can name the call sites. Revert afterwards.

## Rules for A/B experiments

- **Every A/B must be a fresh launch.** Startup bursts run until the backend settles; once settled the process is
  effectively immune, so toggling a lever mid-run measures nothing. (A 60 s hard-churn test on a settled instance
  produced zero growth.)
- Restart between conditions and compare peak `phys_footprint`, not a single sample.
- Run-to-run noise is real; trust large deltas and shape (climb-then-settle vs flat), not 10 % differences.

## Known-good ladder (dev, 2026-07-25)

Useful as a sanity baseline when re-testing: with the NAS index resumed, the app peaked at ~646 MB; suppressing the
media-coverage walk alone made the same build flat at ~155 MB. Details and the full investigation:
`docs/notes/memory-runaway-rust-heap-2026-07-25.md`.

## Past investigations

Read these before re-deriving anything; between them they cover every cause found so far.

- `docs/notes/idle-memory-profile-2026-07-28.md` — the STEADY-STATE costs (2.5 GB idle): SQLite page cache across many
  thread-local connections, and the importance rescore treadmill. Start here for "it's high but not climbing".
- `docs/notes/memory-runaway-rust-heap-2026-07-25.md` — the RUNAWAY (up to 50 GB): a walk that materialized every image
  path. Also the origin of the `IOAccelerator` trap above.
- `docs/notes/idle-malloc-large-clip-towers-2026-08-21.md` — the leading candidate for the 643 MB `MALLOC_LARGE` in that
  idle profile: Core ML holding the two CLIP towers, at a measured 307–412 MB, which nothing had been able to name
  because Core ML allocates through the SYSTEM allocator and so falls between both of Cmdr's allocator APIs. Also the
  origin of the region-histogram method above.
- `docs/notes/high-memory-gpu-compositor-investigation-2026-07.md` — superseded; its conclusion is wrong (it read the
  mislabel as GPU memory). Kept for the measurement methodology only.

## Before proposing an allocator setting

`docs/notes/mimalloc-purge-experiment-2026-09-22.md` is the source-read on what's tunable. The build is mimalloc **v3**
(`libmimalloc-sys` builds v3 unless the `v2` feature is set, and nothing sets it), so v2 option names from training data
are wrong. Env-var tuning works, and `launchctl setenv` is the way to get options into a Finder-launched app without
changing FDA or the data dir. `MIMALLOC_SHOW_STATS=1` on a release build prints no live-bytes section: `MI_DEBUG=0`
makes `MI_STAT` 0. And v3 on macOS already decommits with `MADV_FREE_REUSABLE` after 1 s at page granularity, so
"mimalloc is hoarding pages" is a weak starting hypothesis. The note carries the A/B protocol and the conditions.
