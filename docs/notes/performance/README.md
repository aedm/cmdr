# CPU, RAM, and idle cost: start here

The hub for everything Cmdr knows about its own resource use: idle CPU, memory, wakeups, file descriptors, and thread
churn. It holds the current measured state, the rules every measurement here has paid for, what's fixed and where, and
**the one ranked list of open follow-ups**. The notes beside it hold the evidence; this page points at them.

Pure throughput benchmarks (scan, copy, search latency) stay in `docs/notes/README.md`.

## Read order

1. This page, all of it. The methodology rules below have each cost at least one wrong answer.
2. `docs/tooling/memory-debugging.md`: how to measure memory (the `memory_diagnostics` MCP tool first, `vmmap` second),
   the `IOAccelerator` trap, fingerprinting a block by region size, and attributing allocations.
3. `idle-cpu-attribution-2026-08-03.md`: how idle CPU was mis-attributed four times and what method held.
4. The dated note for the area you're in, from the index at the bottom.

## Current measured state

**The targets** every measurement here is judged against (set by David in a comment on the idle-cost issue, 2026-09-21):

- **Steady-state RAM of 200–300 MB**, never above 300 MB while nothing is indexing, searching, or transferring.
- **Idle CPU under 1%**.

**Memory, prod v0.46.1 after ~25 h** (verified with `vmmap` and `footprint -s` on the `/Applications` build, 2026-09-22;
breakdown in `mimalloc-purge-experiment-2026-09-22.md` § "Baseline"):

- Footprint **708 MiB** (peak 829).
- Rust heap (mimalloc, tag 100, which `vmmap` calls `IOAccelerator`) **611 MiB**: 275 dirty plus 336 swapped. The SQLite
  page slab (63 MiB) is inside it.
- System malloc (`Malloc *`) **66 MiB**, beside the heap, not in it.
- CLIP not loaded (no `Malloc Large`).
- A later census, on a release build of current `main` over a clone of prod's data, read **101 MiB live and 144 MiB
  slack in a 246 MiB heap** at 22 min (verified with `rustHeapCensus`, 2026-09-23). Current `main` includes the four
  heap changes in `rust-heap-attribution-2026-09-23.md`, so expect the next prod reading to be lower than 708.

**Idle CPU, prod** (verified with per-thread cumulative CPU from `ps -M` and CPU-time deltas, 2026-09-17):

- Main process ~2.5% of a core, WebContent ~1.7%, GPU helper ~0.1%.
- **No hotspot**: about 30 threads at 0.05–0.22% each. The cost is diffuse and wakeup-driven, so there's no single fix
  left; each remaining item below shaves a thread or two.
- WebContent's share has since dropped: a median 1.50% → 0.36% on a pane on `~` with indexing on
  (`webcontent-idle-fixes-2026-09-23.md`).

**Idle frontend**: `webcontent-idle-cost-2026-09-23.md` (before) and `webcontent-idle-fixes-2026-09-23.md` (after).

## Methodology rules

Rules canonical elsewhere are one line here plus the pointer; the rest are canonical here.

- **`sample` can't attribute diffuse idle cost.** In a 180 s sample of an idle prod app, 99.97% of leaf samples were
  parked. Use per-thread cumulative CPU (`ps -M <pid>`) and CPU-time deltas over minutes (`ps -o time`). Never rank work
  off one `sample` window, never count a syscall leaf as CPU, never infer CPU from log volume
  (`idle-cpu-attribution-2026-08-03.md` § "The rules this leaves behind").
- **`top`'s `IDLEW` column is unreliable on macOS 27**: it read static across intervals. Don't use it for wakeups.
- **`IOAccelerator` in `vmmap` is the Rust heap** (mimalloc tags its arenas 100), and `Malloc *` is NOT Cmdr's heap
  (`docs/tooling/memory-debugging.md` § "The trap").
- **The same tag is spelled three ways**: `vmmap` says `Malloc Small`, older notes say `MALLOC_SMALL`, and
  `memory_diagnostics` reports `VM_MEMORY_*` names. Match on the tag NUMBER (`docs/tooling/memory-debugging.md`).
- **`vmmap`'s `SQLite Page Cache` row (32 KB) is not the page slab.** The slab is a leaked Rust allocation inside the
  heap; `memory_diagnostics` names it as `sqlitePageCache`.
- **mimalloc's own stats (`MI_STAT`, `MIMALLOC_SHOW_STATS`) are unreliable for live bytes.** Use `rustHeapCensus`
  (`docs/tooling/memory-debugging.md` § "Live bytes vs allocator slack").
- **Swapped doesn't mean "built once and left"**: on a pressured machine, heap pages swap within 2–5 minutes
  (`rust-heap-attribution-2026-09-23.md`).
- **Release builds only for memory work.** Debug builds allocate differently and their absolute numbers mean nothing off
  that machine; quote a debug-build win as a ratio.
- **David's dev machine is an outlier**: 16.2 M FS events in 25 h from worktrees and cargo builds, with the index writer
  alone at ~20% of process CPU. Its numbers are a heavy case, not a typical user's; say which one you have.
- **Identify WebContent with a busy loop** in the webview (or a burst of pane navigations) and see which pid gains the
  CPU. The GPU helper is the one spawned in the same second.
- **Interleave A/B runs and report medians with their spread.** Machine load moves every number here; a single pair
  proves nothing. Every A/B is a fresh launch (`docs/tooling/memory-debugging.md` § "Rules for A/B experiments").
- **Isolated instances only**: before launch, verify the data dir, bundle id, and ports are the instance's own (`lsof`),
  and clone prod data with `cp -c`, including every `-wal` and `-shm` (or `sqlite3 .backup` while prod runs). Exclude
  secrets and the crash report. Never touch the running prod app beyond read-only reads.
- **Check the peer before calling a socket leaked.** The MCP "leak" was live clients with keep-alive pools
  (`mcp-connection-leak-2026-09-22.md`); the real SMB leak showed as sockets whose peer had hung up or that outlived
  their owner (`smb2-socket-lifetime-2026-09-23.md`).
- **Re-verify the PID and the recipe before trusting either.** A stale PID and a stale recipe (the old `MALLOC_SMALL`
  spelling, which reads as a clean zero) each misled this investigation once.

## What's fixed, and where it's documented

- **Idle frontend**: seven fixes (scanning tooltips, Size-column width hold, disk-space emits, backend routing of folder
  sizes, refresh only on a shown change, the hourglass delay, free-space precision):
  `webcontent-idle-fixes-2026-09-23.md`.
- **Hidden-entry diffs**: a change to entries a pane doesn't show (dotfiles in `~` with hidden files off) no longer
  reaches it, and diff indices are the pane's rows: `hidden-entry-diffs-2026-09-23.md`.
- **Rust heap**: the score cache, the MCP search arena's 30 s drop, the wake inbox paged to `main.db`, and the heap
  census: `rust-heap-attribution-2026-09-23.md`.
- **SMB sockets**: fixed in `smb2` 0.24.1 (Cmdr ships 0.25.0): `smb2-socket-lifetime-2026-09-23.md`.
- **mDNS log storm**: `vendor/mdns-sd` stops a multicast-join retry every 5 s on machines with a VM bridge:
  `docs/notes/mdns-sd-multicast-join-retry-loop.md`.
- **`memory_diagnostics` over MCP**, release builds included: `docs/tooling/memory-debugging.md`.
- **Each CLIP tower loads on demand**, so enrichment never pays for the text tower:
  `crates/cmdr-index/src/media_index/clip/DETAILS.md` § "What holding the towers costs". **A rescan-anchor storm costs
  one sweep a day**: `crates/cmdr-index/src/indexing/reconcile/reconciler/rescan/DETAILS.md` § "Anchor-cardinality
  routing".
- **Earlier**: the importance treadmill, the page slab, the per-row INSERT re-parse, and the runaway coverage walk, each
  in its dated note below.

## Open follow-ups

The single ranked list. Where an issue exists, it's the place to track the work; the rest have none yet. Ranked by
expected payoff over effort.

1. **Pool the index walker threads, then re-compare allocators.**
   `crates/cmdr-index/src/indexing/scanner/walker/engine.rs` spawns a watchdog plus workers per walk, and
   `reconcile/reconciler/rescan/mod.rs` spawns `rescan-subtree` per subtree reconcile: ~29,000 threads in 40 min, which
   likely strands mimalloc pages. Then re-run v3 against the system allocator at idle
   (`allocator-comparison-2026-09-23.md`). Status: not started. Clear win on churn alone.
2. **Diagnose the ~500 MiB still live 13 min after search bursts**, under every allocator. Measured on a base from
   before the MCP arena's 30 s drop, so re-check on current `main` first; it may be gone. Status: not started.
3. **The search loop probably allocates per entry** (a no-match query slows under the system allocator). Unverified.
   Fixing it would speed search under any allocator. Status: not started.
4. **One SMB share reached three ways becomes three volumes.** `smb_volume_id` keys on the address as mounted, so the
   LAN IP, a VPN IP, and the mDNS name make three indexes, writers, and scans
   (`thread-and-connection-inventory-2026-09-22.md`). Fix with an alias-adoption layer, not `same_server*` as a key.
   Risk: a wrong match merges two indexes. Status: not started; worth a spec.
5. **Gate the mDNS browse on UI need, and keep the identity cache.** The staleness policy is David's call
   (recommendation: mount dedupe never trusts a stale cache). Status: waiting on David.
6. **Cmdr indexes developers' build output** (`target/`, `node_modules`, `.svelte-kit`, temp fixtures). Whether the
   rescan walk may read `SYSTEM_DIR_EXCLUDES` is David's call: issue #236.
7. **A stuck-loop watchdog at the log sink**, plus a `(target, level)` counter. Designed, ~2–2.5 days. Separately,
   third-party `log::error!` never reaches the error-report flow (Flow B). Status: not started.
8. **The CPU half of the diagnostics instrument**: per-thread CPU (`task_threads` plus `thread_info`) and wakeup
   counters (`TASK_POWER_INFO`) over MCP, next to `memory_diagnostics`. It would also log thread-count growth over a
   run. Status: not started.
9. **A sync-status pool thread wedged in a File Provider call** (seen in the prod log). The pool bounds the cost by
   design; what's open is which provider call never answers. Status: not started.
10. **Load only the active language's messages** (~10–25 MB of WebContent heap and 4 MB of startup parse): issue #134,
    David's decision.
11. **Upstream the `mdns-sd` fix.** The PR draft is in `docs/notes/mdns-sd-upstream-pr/pr-draft.md`, unsent. Add a guard
    that `mdns-sd` resolves from `vendor/`: a dependency bump past 0.20.x would silently drop the `[patch]`. Status:
    draft ready.
12. **The `bridge*` interface filter for mDNS**: parked (it false-positives on Thunderbolt Bridge).
13. **WebContent grows over days** (138 → 262 MB): take a Web Inspector heap snapshot on a long-running build before
    changing anything. Status: not started.
14. **The i18n screenshot run may lose drive-row coverage**, since the scanning tooltip body is no longer mounted until
    hovered. Check on the next capture run.
15. **After the next release**: run `memory_diagnostics` (it now includes `rustHeapCensus`) on a long-running prod to
    explain the rest of the heap (~360 MiB was unexplained at 0.46.1), and run the Cmdr acceptance check for the `smb2`
    socket fix (mount/unmount cycles leave no sockets). `smb2`'s own consumer suite wasn't run for 0.24.1.
16. **`SmbClient::close()` (LOGOFF)** was left out of `smb2` on purpose; it needs design calls
    (`smb2-socket-lifetime-2026-09-23.md` § "Left out on purpose").
17. **Reply to the #92 reporter**: David's.

Smaller or already filed, unranked:

- **Take a fresh idle baseline on a quiet machine**, issue #231: largely answered by the two baselines above; what's
  left is re-ranking the CLIP items against them.
- **CLIP**: should an idle tower unload itself (#233), the ~400 MB non-GPU compute-unit path (#232), and an fp16 text
  tower (#234). The costs they trade: `crates/cmdr-index/src/media_index/clip/DETAILS.md` § "What holding the towers
  costs".
- **Set the rescan-storm threshold from a week of data**: #235.
- **Drop the `ORDER BY` from `above_threshold` when nobody needs the order**: #237.
- **A running importance pass ignores the memory watchdog and shutdown**: #230. **Spotlight "last used" sampling cost**:
  #229.
- **The SMB importance DB outlives its index DB** after the index is removed
  (`thread-and-connection-inventory-2026-09-22.md` § "A smaller loose end").
- **The media live tick's `load_statuses`** reads every stored status on any tick that survives the filter; unmeasured
  (`live-tick-cost-2026-08-21.md`).
- **mimalloc's `os_tag` collides with `VM_MEMORY_IOACCELERATOR`**; a non-colliding tag would retire the trap, at the
  cost of invalidating every doc that explains it (`memory-runaway-rust-heap-2026-07-25.md` § "Still open").

## Retired: don't reopen

- **The MCP connection "leak"**: not a leak. Every socket had a live peer, and a killed peer's socket was reaped in
  under a second (`mcp-connection-leak-2026-09-22.md`).
- **The "thread leak"**: every thread is legitimate and bounded (`thread-and-connection-inventory-2026-09-22.md`).
- **"132 connections × 16 MB page cache"**: page memory is one process-wide slab (same note).
- **mimalloc purge tuning**: no effect on the slack (`rust-heap-attribution-2026-09-23.md`).
- **Capping tokio's blocking pool**: not where the thread churn is, and measured to change nothing
  (`allocator-comparison-2026-09-23.md`).
- **mimalloc v2**: no gain over v3 (same note).
- **The GPU-compositor theory of the memory runaway**: it was the Rust heap mislabeled
  (`memory-runaway-rust-heap-2026-07-25.md`).

## Notes index

Newest first.

- `hidden-entry-diffs-2026-09-23.md`: diffs skip rows the pane doesn't show; natural and controlled-churn A/B numbers.
- `webcontent-idle-fixes-2026-09-23.md`: the seven frontend idle fixes and their interleaved before/after numbers.
- `webcontent-idle-cost-2026-09-23.md`: where WebContent and the GPU helper spend idle CPU, why window state barely
  matters, and the frontend memory picture (including the eager translation catalogs).
- `rust-heap-attribution-2026-09-23.md`: live data against slack in the Rust heap, the named live consumers, the four
  changes and their measured effect, and the ~360 MiB still unexplained at 0.46.1.
- `allocator-comparison-2026-09-23.md`: v3 against v2 and the system allocator, the decision to keep v3, and the walker
  thread churn behind mimalloc's slack. Raw numbers: `allocator-comparison-2026-09-23.csv`.
- `smb2-socket-lifetime-2026-09-23.md`: the two `smb2` socket bugs behind the leftover SMB sockets, fixed in 0.24.1.
- `mimalloc-purge-experiment-2026-09-22.md`: what's tunable in mimalloc v3 (option names, `launchctl setenv`, why
  `MIMALLOC_SHOW_STATS` is half-useful), the 0.46.1 baseline, and a purge A/B protocol whose prior is low.
- `thread-and-connection-inventory-2026-09-22.md`: a verdict per worker thread and SQLite connection count, and the one
  real duplication (one SMB share at three addresses becomes three volumes). Read it before counting threads.
- `mcp-connection-leak-2026-09-22.md`: why the MCP server doesn't leak connections, what one costs, and how the two
  stuck SMB sockets were found.
- `live-tick-cost-2026-08-21.md`: what a media live tick costs (the coverage gate and the scoped walk), and why
  filtering the walk without fixing the gate would have left the floor in place.
- `idle-malloc-large-clip-towers-2026-08-21.md`: Core ML holding the CLIP towers costs 307–412 MB of `Malloc Large`, 80%
  of it the text tower; the region-size fingerprint method and the one `vmmap` line that confirms it.
- `importance-treadmill-2026-08-04.md`: the 60-second importance rescore treadmill, why raising `SCOPED_WALK_MAX_DIRS`
  is refuted, and the signals-not-score equality key.
- `idle-cpu-attribution-2026-08-03.md`: 110 minutes of idle CPU over 9.1 hours, four wrong answers, and the rules they
  left.
- `idle-memory-profile-2026-07-28.md`: a 2.5 GB idle footprint from SQLite page cache across many connections and the
  rescore treadmill, and the shared page slab it led to.
- `memory-runaway-rust-heap-2026-07-25.md`: the runaway up to 50 GB (a coverage walk materializing every image path),
  and the origin of the `IOAccelerator` trap.
- `idle-cpu-indexing-streamlining-2026-07.md`: issue #37's idle-CPU stack (the importance loop and the collated-key
  subtree clear) and what each fix bought.
- `high-memory-gpu-compositor-investigation-2026-07.md`: superseded (it read the Rust heap as GPU memory); kept for the
  measurement gotchas and the frontend throttles it landed.

Related elsewhere: `docs/notes/listing-row-fetch-quadratic-2026-08-22.md` (a main thread saturated by per-row IPC),
`docs/notes/search-arena-row-2026-08-06.md` (the search arena's size), and
`docs/notes/sync-status-pool-bench-2026-07-31.md` (the sync-status pool).
