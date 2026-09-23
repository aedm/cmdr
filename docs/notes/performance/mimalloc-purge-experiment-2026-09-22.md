# Can mimalloc option tuning halve Cmdr's RAM? A runnable protocol

The question: Cmdr 0.46.1 sits at a 708 MiB footprint, 87% of it the Rust heap, and **~548 MiB of that heap has nothing
named on it** once SQLite's slab is subtracted. The hypothesis this note tests is that a large share of it is pages
mimalloc holds but could return to the OS, so purge tuning would drop the number a user sees for little throughput cost.

**Read the verdict below before spending a day on the protocol.** The source says the prior is low, and the protocol is
shaped as a two-condition falsification rather than an option sweep because of it.

Traps, the `vmmap` recipe, and past investigations: `docs/tooling/memory-debugging.md`. Read it first.

## Verdict: possible, but the hypothesis is already weakened by the source

Environment-variable tuning **does work** on the shipped build. Four things had to be true, and all four are:

- **Which mimalloc.** `mimalloc = "0.1"` resolves to `mimalloc` 0.1.52 → `libmimalloc-sys` 0.1.49 (`Cargo.lock`). That
  crate vendors both v2 and v3 and **builds v3 unless the `v2` feature is set** (`build.rs` lines 8-12); nothing in the
  workspace sets it (`apps/desktop/src-tauri/Cargo.toml:198`, `crates/cmdr-fs/Cargo.toml:111`). So the C library is
  **mimalloc v3.3.2** (`c_src/mimalloc/v3/include/mimalloc.h:11`). Confirmed against the shipped binary: it contains the
  v3-only option names `minimal_purge_size` and `page_commit_on_demand`, which v2 doesn't have.
- **Env reading is compiled in.** `MI_NO_GETENV` is never defined by `build.rs`, so `mi_option_init`
  (`v3/src/options.c:633`) reads each option from `mimalloc_<option_name>`, case-insensitively, via a manual walk of
  `environ` (`v3/src/prim/unix/prim.c:824`). `MIMALLOC_PURGE_DELAY` and friends therefore work.
- **A GUI app can be given the env.** `launchctl setenv NAME VALUE` propagates to apps launched afterwards by Finder or
  `open` (verified on macOS 27.0 / 26A428 by launching a probe bundle through `open` and dumping its `environ`,
  2026-09-22). This is the launch path the protocol uses, because it keeps TCC/Full Disk Access, the data dir, and
  bundle identity **identical** between conditions. A Terminal launch does not: the TCC responsible process becomes
  Terminal, so FDA differs and the index behaves differently.
- **`MIMALLOC_SHOW_STATS=1` is half-useful, and not mixable with the A/B.** Release builds compile with `MI_DEBUG=0`
  (`build.rs`), which makes `MI_STAT` 0 (`v3/include/mimalloc/types.h:81-87`), which compiles out the live-bytes section
  of the stats dump. Verified against the shipped binary: it carries the labels `touched`, `reserved`, `committed`,
  `purged`, `arenas`, `mmaps`, and **not** `binned` or `malloc req`. On top of that, mimalloc prints to stderr at exit,
  and a launchd-started GUI app's stderr goes nowhere (Cmdr's logger writes its own file chain; it does not redirect the
  process's stderr, `apps/desktop/src-tauri/src/logging/dispatch.rs:156`). Capturing it needs a Terminal launch, which
  breaks comparability. **Treat it as a separate optional diagnostic, never as an A/B condition.**

**Why the prior is low anyway.** mimalloc v3 on macOS is already configured about as aggressively as its options allow:

- Decommit uses `madvise(MADV_FREE_REUSABLE)`, chosen specifically because it does immediate RSS accounting
  (`v3/src/prim/unix/prim.c:493-502`). Not `MADV_FREE`, not a no-op.
- `purge_delay` defaults to **1000 ms** and `purge_decommits` to **1** (`v3/src/options.c:127,135`). Purging is on, and
  it is decommit rather than reset.
- `minimal_purge_size` defaults to 0, which resolves to the OS page size, 16 KiB on Apple Silicon (`v3/src/os.c:55-65`).
  There's no coarse granularity hiding small free runs.

So there is no off switch that is currently off. Whatever the 611 MiB is, mimalloc has had a second to give it back and
hasn't, which means it believes those pages are in use. That is what makes this worth two conditions rather than six.

## Baseline (2026-09-22, Cmdr 0.46.1, macOS 27.0 / 26A428)

Prod instance, pid 69181, launched 2026-09-21 20:22, sampled after **1,505 minutes** (~25 h) of real use. `vmmap` and
`footprint -s` agree within a megabyte:

- Physical footprint **708 MiB**, peak **829 MiB**
- Rust heap (`IOAccelerator`, dirty + swapped) **611 MiB** = 275 dirty + 336 swapped, across 64 regions → **87% of the
  footprint**
- System malloc, every `Malloc *` row together **66 MiB** (`Malloc Small` 60 MiB of it), beside the heap, not in it → 9%
- `Malloc Large` **absent**: the CLIP towers are not loaded
- Everything else ~31 MiB: `Stack` 2.8 MiB, `__DATA*` ~7 MiB, `WebKit Malloc` 3 MiB, and the rest in the noise

⚠️ **The `MALLOC_SMALL` / `MALLOC_LARGE` spelling in older recipes matches nothing on macOS 27**, where the tags are
`Malloc Small` and `Malloc Large`. A grep for the old spelling reads as a clean zero, which is how a first pass at this
baseline lost the whole 66 MiB. `mem-sample.sh` matches on a prefix for exactly this reason.

### What the Rust heap is, and what's left over

Subtracting the one component we can name:

- **SQLite's shared page-cache slab: 63 MiB.** One process-wide slab via `SQLITE_CONFIG_PAGECACHE`
  (`crates/cmdr-fs/src/sqlite_util.rs:27-60,116-155`), verified live as 15,363 slots × 4,368 B. It's a leaked Rust
  allocation, so it sits **inside** the mimalloc total. Count it as fully resident rather than partly: the const's own
  docs record that the slab runs permanently full, with nine idle write connections holding 63 of the 64 MiB
  (release-build probe, 2026-08-22). Page memory no longer scales with connection count, so the old "132 connections ×
  16 MB" line is retired (`docs/notes/performance/thread-and-connection-inventory-2026-09-22.md`).
- ⚠️ The `SQLite Page Cache` VM tag in `vmmap` is **32 KB** and is **not** this slab. Anyone reading that row concludes
  SQLite costs nothing.

**611 − 63 = ~548 MiB of Rust heap with nothing named on it: 77% of the whole footprint.** That is the number this
experiment is really aimed at, and the reason a negative result still matters: it removes the allocator from the list
and leaves 548 MiB that has to be attributed some other way.

⚠️ **Only the slab comes off the heap.** The 66 MiB of `Malloc *` above sits BESIDE the Rust heap in the system zones,
not inside it, so it is never part of this subtraction. The two numbers landing one megabyte apart is a coincidence of
this particular reading, and mixing them up rebuilds the same both-sides-are-one-heap error the `IOAccelerator` trap is
made of. Everything here is in `vmmap`'s units, where `M` means MiB; rendering the slab as "66 MB" decimal is what made
the collision look real.

## The tool

`apps/desktop/scripts/mem-sample.sh` takes one reading and appends a CSV row. It lives in the repo rather than a
scratchpad because it outlives this experiment: it's the `vmmap` recipe from `docs/tooling/memory-debugging.md` with the
tag-column trap handled (it matches the trimmed tag column, so `IOAccelerator (reserved)` can't be counted as heap), and
it records the mimalloc env that was actually set on every row, so a sample can't be misfiled under the wrong condition.

```
./apps/desktop/scripts/mem-sample.sh baseline          # one sample
./apps/desktop/scripts/mem-sample.sh --watch A 15      # every 15 min until Ctrl-C
./apps/desktop/scripts/mem-sample.sh --show            # the CSV so far
./apps/desktop/scripts/mem-sample.sh --env             # what the next launch will inherit
```

It samples the `/Applications` build specifically, since a `cargo run` release build is a different binary with a
different data dir (`CMDR_MEM_PID` overrides). CSV path: `$CMDR_MEM_CSV`, default `~/cmdr-mem-experiment.csv`.

## The protocol

Three conditions, and **you may only need two**. Each needs a fresh launch (`docs/tooling/memory-debugging.md` § Rules
for A/B experiments), and roughly 3 hours of ordinary use: enough for the startup and indexing bursts to run and the
process to reach its shape. Start `--watch` right after each launch and leave it in a terminal tab.

Before condition A, quit Cmdr and let the indexes settle. Keep the workload roughly matched across conditions: same kind
of browsing, same volumes, at least one big-directory visit and one NAS visit each.

**Condition A — defaults.**

```
launchctl unsetenv MIMALLOC_PURGE_DELAY; launchctl unsetenv MIMALLOC_ARENA_PURGE_MULT
launchctl unsetenv MIMALLOC_PAGE_FULL_RETAIN; launchctl unsetenv MIMALLOC_PAGE_RECLAIM_ON_FREE
launchctl unsetenv MIMALLOC_GENERIC_COLLECT; launchctl unsetenv MIMALLOC_ARENA_RESERVE
./apps/desktop/scripts/mem-sample.sh --env             # must print "(none set)"
# quit Cmdr, relaunch it from Finder / Spotlight
./apps/desktop/scripts/mem-sample.sh --watch A 15
```

**Condition B — purging off. This is the ceiling probe, and it's the point of the whole day.**

```
launchctl setenv MIMALLOC_PURGE_DELAY -1
./apps/desktop/scripts/mem-sample.sh --env
# quit Cmdr, relaunch from Finder
./apps/desktop/scripts/mem-sample.sh --watch B 15
```

B measures how much work purging is currently doing. It's deliberately the _wrong_ direction: if switching purging off
entirely doesn't make the number meaningfully worse, then purging is already returning close to nothing, and no amount
of tuning in the good direction can return more. **That kills the whole option class in one condition.**

**Condition C — hold nothing. Only run this if B − A ≥ 50 MiB on peak footprint.**

```
launchctl setenv MIMALLOC_PURGE_DELAY 0
launchctl setenv MIMALLOC_PAGE_FULL_RETAIN 0
launchctl setenv MIMALLOC_PAGE_RECLAIM_ON_FREE 1
launchctl setenv MIMALLOC_GENERIC_COLLECT 1000
launchctl setenv MIMALLOC_ARENA_RESERVE 131072          # 128 MiB arenas instead of 1 GiB
./apps/desktop/scripts/mem-sample.sh --env
# quit Cmdr, relaunch from Finder
./apps/desktop/scripts/mem-sample.sh --watch C 15
```

Every "return more, hold less" knob at once. If the combination doesn't move the number, no subset will, so there's no
reason to test them one at a time first. Bisect only if C wins.

**Afterwards**, always `launchctl unsetenv` every variable you set. They persist for the login session and would
silently apply to the next launch, and to every other app.

### Reading the result

Compare **peak footprint** at matched uptimes (the CSV's `uptimeMin` column), not a single late sample. Also watch
`rustHeapTotalMb`, which is the part any of this could possibly affect.

- **≥ 100 MiB off peak footprint**: a real win, worth pursuing into a shipped default.
- **50–100 MiB**: inconclusive. Repeat A and the winner once more before believing it.
- **< 50 MiB**: noise. Run-to-run variance on this workload is real, and the doc's rule is to trust only large deltas.

A footprint change with no matching `rustHeapTotalMb` change means something else moved and the condition had nothing to
do with it.

## What this can and cannot tell us

**It can** falsify one hypothesis class: "mimalloc is hoarding returnable pages." That's worth a day precisely because a
clean negative redirects the whole effort.

**It cannot tell us what the unattributed ~548 MiB contains**, and that is the actual question. mimalloc's committed
bytes (what `get_memory_diagnostics` reports as `rustHeapCommittedBytes`) is live allocations _plus_ free lists _plus_
arena slack, and the code says so: "mimalloc exposes no cheap process-wide 'bytes in use', so committed is the number
that tracks the Rust heap" (`crates/cmdr-fs/src/process_memory/mod.rs:241-243`). Nothing in the tree can currently split
live data from slack.

**If the number does NOT move**, two hypotheses survive and this experiment can't tell them apart:

1. **The heap is live data.** Index caches, the 64 MiB SQLite page slab (`crates/cmdr-fs/src/sqlite_util.rs`), in-memory
   structures. `docs/notes/performance/idle-memory-profile-2026-07-28.md` is the cost catalog, but read
   `docs/notes/performance/thread-and-connection-inventory-2026-09-22.md` first: it measured this same instance and
   **retires the "connection count × page cache" line**, since page memory stopped tracking connection count when the
   shared slab landed. It also names a live duplication worth pricing here (one SMB share reached at three addresses
   becomes three volumes, each with its own threads, database, and scan).
2. **The heap is fragmented.** Live objects scattered such that mostly-free mimalloc pages stay dirty. mimalloc cannot
   purge a page holding one live object, so this looks exactly like hoarding and is immune to every purge option.

Telling 1 from 2 needs **live bytes**, and the cheapest route is a local build with mimalloc's full stats compiled in.
The `cc` crate honours `CFLAGS`, so `CFLAGS="-DMI_STAT=1" cargo build --release` (a build-only change, nothing shipped)
plus `MIMALLOC_SHOW_STATS=1` and a Terminal launch would print the `binned` / `total` / `malloc req` lines the release
build compiles out. Not comparable to the A/B runs (dev build, different FDA, different data dir), but decisive for the
live-vs-slack split, which is the question that matters. **Do this before, or instead of, condition C.**

**If the number DOES move**, we'd be trading throughput for it, and the trade needs measuring:

- `PURGE_DELAY=0` makes every freed run issue an immediate `madvise` syscall. On directory listing and indexing, which
  are the allocation-heavy paths, that's a per-free syscall in the hot loop.
- `PAGE_FULL_RETAIN=0` and `GENERIC_COLLECT=1000` increase page churn and collection frequency.
- `ARENA_RESERVE=128 MiB` means 8× more `mmap` calls for the same heap growth.

Detect it by timing the same work under each condition: a big-directory listing, and a full index run on a known volume.
`RUSTY_COMMANDER_BENCHMARK=1` turns on the microsecond event timeline (`apps/desktop/src-tauri/src/benchmark.rs`). A
throughput cost that only shows up under heavy churn is exactly the kind that wouldn't appear in a 3-hour browse, so
don't ship a default off this experiment alone.

## Open: the diagnostic has no way in on a release build

`get_memory_diagnostics` (`apps/desktop/src-tauri/src/commands/memory_diagnostics.rs:192`) is registered as an IPC
command (`apps/desktop/src-tauri/src/ipc.rs:844`) and has a generated binding
(`apps/desktop/src/lib/ipc/bindings.ts:4470`), but **nothing calls it** outside its own tests. On a shipped release
there is no way to invoke it, which is a shame, because it's the one reading that spans both allocators and names the
SQLite slab.

Exposing it as an MCP tool is small: the registry is a single `mcp_tools!` table entry plus a JSON schema and a handler
that awaits the command (`apps/desktop/src-tauri/src/mcp/tool_registry/table.rs`), and it's a read-only,
`TokenGate::Open`-class tool carrying only byte counts and fixed tag names (the command's module docs make the privacy
argument). It would make every future memory question answerable against the running prod app instead of through
`vmmap`.

It is **not** part of this experiment: it changes the shipped app's runtime surface, which this effort explicitly
doesn't do. It's a separate small piece of work, and it's worth doing.
