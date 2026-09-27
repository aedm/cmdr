# Pooling the index walker's threads, and what it did to the allocator slack (2026-09-27)

**What this settles:** whether giving the index walker and the rescan drain long-lived threads cuts thread creation, and
whether that shrinks mimalloc's slack enough to change the allocator decision in `allocator-comparison-2026-09-23.md`.
**Pooling removes 97–99% of the walker's thread creations, and changes neither memory nor CPU measurably.** The slack
isn't thread churn: a round that created 19,713 threads against 905 ended within 1 MiB of it. So pooling doesn't move
the allocator tradeoff: the system allocator's idle saving survives unchanged. Raw numbers:
`walker-thread-pool-2026-09-27.csv` (one row per round, condition, and sample point; bytes).

## The change

- `cmdr_fs::utility_pool::UtilityPool`: a keep-alive pool of `Utility`-QoS threads (60 s keep-alive, unbounded, never
  queues a job behind a busy thread).
- The walker's workers are jobs on the process-wide `WALK_THREADS`, and its watchdog runs on the thread that called
  `walk`, which blocks anyway. Mechanism: `crates/cmdr-index/src/indexing/scanner/walker/DETAILS.md` § "The engine".
- The rescan drain's walks run on `RESCAN_THREADS`.

## Setup

- **Binaries**: release (`pnpm tauri build --no-bundle --target aarch64-apple-darwin`, thin LTO). `pre` is `e9fd713ad`
  (per-walk threads), `post` is the pooling commit. Both carry the same uncommitted diagnostic `main.rs`: the runtime
  allocator switch from the allocator note (`CMDR_EXP_ALLOC`, one relaxed load and a branch per call in every condition)
  and a thread census through libpthread's introspection hook (`pthread_introspection_hook_install`), which counts every
  thread the process creates and names each one as it terminates. That counts ALL threads, Rust or not, unlike the
  allocator note's first-allocation counter.
- **Four conditions, run concurrently each round** (same machine state, fresh launches, launch order rotated): A `pre`
  v3, B `post` v3, C `pre` system, D `post` system.
- **Data**: a `sqlite3 .backup` of prod's DBs (prod was running), taken fresh before each round so every round replays a
  gap of minutes, plus clones of the small JSON state; secrets, MCP files, the crash report, analytics, and the lock
  excluded. Analytics, update checks, crash and error reports, and the global shortcut off; each instance on its own
  data dir, instance id, and MCP port, verified with `lsof` every round (zero prod files open).
- **Protocol per round**: the allocator note's. Launch; sample at 5 and 10 min (idle); rescan the root index (5.4–5.8 M
  entries); three bursts (MCP search `*.pdf`, then navigation to a 200,000- and a 100,000-entry folder); sample at +5,
  +10, and +13 min (settled). Heap = tag 100 plus `Malloc *` dirty and swapped. Live = `rustHeapCensus.liveBytes` plus
  the system zones' in-use bytes (the system allocator: zone in-use alone).
- **Noise**: load average 3–90 from sibling agents. Five rounds, 27–31 min each.

## Thread creations

Per run (27–31 min; median [min–max] over five rounds):

- **All threads, whole run**: `pre` 2,952 [2,117–19,713], `post` 758 [642–905].
- **Walker threads** (`index-walk`, `index-walk-watchdog`, `rescan-subtree` that terminated): `pre` 2,357
  [1,543–18,948], `post` 49 [40–124]. The `post` ones are pooled threads aging out after a quiet minute.
- **Where the churn happens**: mostly after the rescan and the bursts, not at idle. In the first 10 idle minutes `pre`
  created 324–841 threads and `post` 133–239. Round 1's 17,000 walker threads came in the five minutes after its bursts.
- The rest of `post`'s creations (~25 a minute) aren't walkers; the census can't name them further, and they're the same
  in both builds.

## Memory

MiB, median [min–max]:

| reading                    | A `pre` v3       | B `post` v3      | C `pre` system   | D `post` system  |
| -------------------------- | ---------------- | ---------------- | ---------------- | ---------------- |
| idle footprint (10 min)    | 200 [188–257]    | 211 [183–256]    | 137 [135–175]    | 137 [131–156]    |
| idle heap ÷ live           | 1.71 [1.50–1.96] | 1.77 [1.53–2.00] | 1.19 [1.16–1.30] | 1.17 [1.15–1.19] |
| post-burst footprint       | 1065 [1001–1137] | 1090 [1034–1364] | 2563 [1819–3142] | 2459 [1865–3135] |
| settled footprint (13 min) | 364 [328–517]    | 357 [322–493]    | 221 [152–367]    | 269 [147–469]    |
| settled slack (heap−live)  | 208 [168–327]    | 201 [181–298]    | 71 [20–199]      | 112 [18–287]     |

Paired per round:

- **Pooling under v3 (B − A)**: idle footprint +7, +33, −12, −1, −5 (median −1); settled +1, +84, −24, +29, −41 (median
  +1). Round 1, the one with 19,713 against 905 thread creations, is the +1. **The slack didn't shrink.**
- **Pooling under the system allocator (D − C)**: idle median −2; settled +47, +103, +28, +56, −4 (median +47). Four of
  five rounds worse, but inside the ±95 MiB settled noise floor the allocator note measured. Not conclusive.
- **System against v3**: idle −60 (`pre`) and −78 (`post`) median; settled −122 and −88; right after the bursts
  **+1.2–1.5 GiB** worse, the same lazy-return transient the allocator note found.

## CPU

- Idle (5 → 10 min): 1.86% `pre` against 1.92% `post` of a core, v3; identical within noise under both allocators.
- Settling (burst → +13 min, where the churn is): B − A +0.53, +0.24, +0.22, +0.67, −0.17 percentage points. Round 1's
  17,000 extra thread creations bought no visible CPU either: at a few tens of microseconds each they're ~0.1% of a core
  over 13 minutes, below this machine's noise.

## What this means

- **The churn didn't cause the slack.** The allocator note's evidence was one low-churn round against four high-churn
  ones; five paired rounds here with a 3–20× churn difference show none. Whatever the ~75 MiB idle and ~200 MiB settled
  slack is, it survives with the walker's threads pooled.
- **Pooling doesn't move the allocator tradeoff.** The system allocator still saves 60–80 MiB at idle and ~90–120
  settled, and still costs a 1.2–1.5 GiB transient after a burst. Its search penalty is a separate question, and
  `search-loop-allocations-2026-09-27.md` reports it closed by removing the scan's per-row allocations, so that side of
  the allocator note's decision needs re-weighing on its own.
- **The pool stays** for what it does do: a stream of walks no longer costs a stream of `clone`s, 8 MiB stack mappings,
  and allocator thread setups, and the process's thread census reads cleanly. It's a hygiene win, not a memory one.

## Open

- Where the v3 slack lives. `idle-census-2026-09-27.md` places the settled slack outside any page's blocks, in free
  arena slices mimalloc keeps committed after a burst grows the heap, which fits these numbers: how many threads came
  and went didn't move it.
- Whether D − C's +47 MiB settled is real. It would take more rounds than this machine's noise allows in an afternoon.
