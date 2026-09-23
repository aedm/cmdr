# What's in the Rust heap, and the four changes that cut it (2026-09-23)

**What this settles:** how much of Cmdr's Rust heap is live data and how much is allocator slack, what the named live
data is, and what four changes bought. At idle the heap is roughly 55% live data and 45% mimalloc slack; the changes cut
live data by ~40 MiB at idle and ~430 MiB for the 10 minutes after an MCP search, and left the slack where it was.

It picks up where `mimalloc-purge-experiment-2026-09-22.md` stopped (its 0.46.1 baseline had ~548 MiB of heap with
nothing named on it). The allocator itself is weighed in `allocator-comparison-2026-09-23.md`. The census tool this
built is documented where it's used: `docs/tooling/memory-debugging.md` § "Live bytes vs allocator slack".

## Setup

- Release builds (`pnpm tauri build --no-bundle`, thin LTO) under an instance id of their own: own data dir, bundle id,
  MCP port, and WebKit dir, `CMDR_SECRET_STORE=file`, verified with `lsof` before each launch.
- Data: an APFS clone (`cp -c`) of prod's data dir, or `sqlite3 .backup` of each DB when prod was running; 6.6 M-entry
  root index; analytics, update checks, crash and error reports, and global shortcuts off; media indexing off, as in
  prod. Secrets and the crash report excluded.
- Differences from prod: no SMB credentials (the direct SMB upgrades fell back to kernel mounts), no user browsing, runs
  of 10–40 min instead of 25 h, and a host under heavy memory pressure from other agents' builds.

## Live versus slack

**`MIMALLOC_SHOW_STATS` and `mi_stats_print_out` can't answer it**: in v3 the per-thread counters merge only when a
thread collects or exits, and after a ~400 MiB arena was freed they still read 242–265 MiB live. The replacement is a
**page census**: `mi_heap_visit_blocks(NULL, false, …)` summing `used × block_size` (live) and `capacity × block_size`
(block space) per page, now shipped as `rustHeapCensus` in `memory_diagnostics`.

Readings (heap = tag 100 dirty plus swapped), verified on release builds, page census, 2026-09-23:

- Idle, ~10 min: heap 221–227 MiB, live 124–130 (64 of it the SQLite slab), block space ~139.
- Busier, ~9 min: heap 344, live 194, with a 44 MiB and two 9 MiB singletons in flight beside the slab.
- After one search-arena load and drop: heap 250, live 130; a second 1 GiB arena reservation now exists.
- `MIMALLOC_PURGE_DELAY=0` with `MIMALLOC_ARENA_PURGE_MULT=1`: heap 224, live 129, no change from defaults.
  `mi_collect(true)` returned at most 20 MiB. **The slack isn't purge-tunable**, which matches the purge note's prior.
- The same workload under the system allocator ran at ~1.15–1.2× live (zone resident against allocated, 12–14%
  fragmentation); under mimalloc ~1.75×.

On macOS mimalloc decommits with `MADV_FREE_REUSABLE`, and a standalone test confirmed `vmmap` DIRTY excludes reusable
pages, so the leftover isn't "free slices awaiting purge". The arena map showed 88–93 MiB of free, committed slices and
zero purge-pending. Which state it's in stays open; `allocator-comparison-2026-09-23.md` ties it to thread churn.

## Named live data (idle)

Attributed with the system allocator swapped in and `MallocStackLogging=1`, then `malloc_history -allBySize`, split by
the first frame above `libsystem_malloc` (totals matched `vmmap`'s zone "allocated" within 1 MiB):

1. **64 MiB, the SQLite page slab** (`crates/cmdr-fs/src/sqlite_util.rs`). Long-lived by design.
2. **~31 MiB, the media coverage importance-score cache**, 180,000 path `String`s plus a `HashMap<String, f64>`, built
   even with media indexing off because the settings `volume_state` poll read it unconditionally. Fixed below.
3. 5.9 MiB, search importance weights (by design). 5.8 MiB, font metrics.
4. **~6 MiB and growing, the agent wake inbox**: 26,000+ rows, most deferred, with nothing draining them while proactive
   mode is off. Fixed below.
5. 2.5 MiB, the index writer's channel buffer; 1.3–6.4 MiB of transient reconciler upserts; tails under 2 MiB.

Transient: the search arena, ~400 MiB live for 6.1–6.4 M entries, held 10 min after an MCP search. Beside the heap, not
in it: 34–36 MiB of system malloc from AppKit, WebKit setup, icon rendering (~10 MiB), and SF Symbol menu images (4
MiB).

## "Swapped" doesn't mean "built once and left"

Prod's 336 MiB of swapped heap looked like a clue to long-lived cold data. On a pressured machine it isn't: 500 MiB of a
freshly loaded search arena went to swap within three minutes, and heap pages generally swapped within 2–5 minutes.
Swapped means "untouched for minutes on a busy machine", nothing more.

## The four changes (landed 2026-09-23)

1. **The census** (`104a089e1`): `rustHeapCensus` in `memory_diagnostics`, from
   `crates/cmdr-fs/src/process_memory/heap_census.rs`. It walks without claiming pages, is bounded to a million pages,
   and ran 30+ times against live release instances, mid-search included, without incident.
2. **The score cache** (`a83b6a647`): not built while image indexing is off, released when it's switched off, and keyed
   on `hash_path` (a hashed `(u64, f64)` slot) when on: 147 B a folder before, under 32 B after.
3. **The MCP search arena** (`c1b3e82d6`): an agent search now arms the same 30 s idle drop the dialog uses, held off
   while any agent call is in flight so a burst shares one arena. The 10-minute backstop stays underneath.
4. **The wake inbox** (`de235afc7`): rows wait in `main.db`, and memory keeps only a count and the soonest deadline. A
   wake loads the inbox only while it runs.

Measured side by side, two release builds launched together from identical prod clones (base = census alone), MiB,
verified on release builds, `memory_diagnostics` census, 2026-09-23:

- t = 1 min: footprint 244.0 → 201.5, live 118.8 → 84.9.
- t = 9 min, idle: footprint 367.2 → 315.2, heap 292.3 → 254.1, live 138.9 → 100.2.
- Right after one MCP search: footprint 796.0 → 723.0.
- Search + 50 s: footprint **785.1 → 348.8**, live 528.9 → 100.1 (the arena dropped at 30 s instead of 10 min).
- t = 22 min, after the base's backstop fired: footprint 366.3 → 313.6.
- Slack stayed 144–185 MiB on both: these changes cut live data, not allocator slack.

## Reconciling prod, and what's still unexplained

Of prod's 611 MiB heap (0.46.1, ~25 h), minus the 63 MiB slab: ~70 MiB is named live data and ~100–120 MiB is slack at
the scale reproduced here, so **~360 MiB stayed unexplained**. It sits in prod-only state these runs couldn't reproduce:
direct SMB sessions, 25 h of browsing, 25 hourly importance recomputes, and repeated bursts. The census from a prod
build that has it is the reading that closes the gap (a follow-up in `README.md`).
