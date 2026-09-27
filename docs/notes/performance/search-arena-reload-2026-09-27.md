# The search arena's reload after a walk, and what it did and didn't cost (2026-09-27)

**What this settles:** whether the full arena reload behind `search/CLAUDE.md`'s one exception is what leaves the heap
316–387 MiB after a burst of searches (`idle-census-2026-09-27.md` § "Follow-up #2", lever 2). **Verdict: it drove the
burst PEAK, not the settled footprint.** Every search after a walk re-read all ~6 M rows while the old arena was still
held, which cost 1.2–2.1 s per search and ~700 MiB of peak. The catch-up that replaced it removes both. The footprint
the heap settles at afterwards is the same with the reload, with the fix, and with no reload at all; most of it comes
from listing the two big folders. The mechanism lives in `apps/desktop/src-tauri/src/search/DETAILS.md` § "Decision 12";
this note is the evidence. Raw census rows: `search-arena-reload-2026-09-27.csv` (one row per round, condition, and
sample point; MiB).

## What the reload was

A live search that walks writes rows behind the warm arena and sets the walk mark. The next search's coverage answer
then can't be honored against that arena, so `arena_for_coverage` called `reload_volume`: remove the arena from the map,
load all rows again. The caller still held its own `Arc` to the old arena through the whole load, so the old arena, the
load's eight segments, and the merge destination were all alive at once.

It fired far more often than "after a walk" suggests. On this machine the first search walked 1,500–2,100 frontier roots
(sibling agents' worktrees, caches, build output), and then nearly every later search walked a few more roots that wrote
nothing: `Cover: 0 entries over 3 frontier roots` in the log, repairs of `+0 -0 ~0`, and a reload anyway, because the
mark is set when a walk starts. In rounds 1 and 3 both later searches reloaded; in rounds 2, 5, and 6, one of them.

## The fix

`volumes::catch_up_volume` replaces the reload. It reads only the rows with an id past the arena's highest and appends
them, in place when nobody else holds the arena and it has room (the loader now keeps 1/128 of the rows spare). The
measured catch-ups: 2,421 rows in 0.8 ms, 59,581 rows in 53 ms, 92,181 in 50 ms, and 0 rows when a walk wrote nothing,
against reloads of 1.2–2.3 s. Why appending created rows is enough, and what it leaves stale: `search/DETAILS.md` §
"Decision 12".

## Method

- **Binaries**: two release builds (`pnpm tauri build --no-bundle --target aarch64-apple-darwin`, thin LTO). "Before" is
  `main` (`59854810f`) plus an experiment switch at the reload site, `CMDR_EXP_RELOAD`: `held` is `main`'s behavior
  unchanged (condition H), `dropped` drops the caller's handle before the reload (X), and `skip` serves the stale arena
  with no reload (K, incorrect on purpose: the ceiling of any fix). "After" is the fix commit (F).
- **Data**: `sqlite3 .backup` of prod's databases (prod was running) plus APFS clones of the rest, secrets and the crash
  report excluded. 5.4–6.3 M-row root index. Analytics, update checks, crash and error reports, the global shortcut,
  `network.directSmbConnection`, and network discovery off. Each instance on its own data dir, MCP port, and instance
  id; `lsof` showed no prod file open, one loopback listener, and no port-445 socket, every round.
- **Protocol**: three instances launched together per round, launch order rotated, a fresh clone each. Idle 8 min, then
  three bursts back to back, each an MCP search for `*.pdf` (`maxWaitSeconds` 10) followed by navigations to the
  200,000-entry folder, `~`, the 100,000-entry folder, and `~` (the allocator note's protocol, as in the census).
  `memory_diagnostics` with `rustHeapCensus` before, right after the third burst, and at +1, +5, and +15 min. The burst
  peak is the maximum `ri_phys_footprint` from `proc_pid_rusage`, polled every 50 ms across the bursts.
- **Machine**: David's dev Mac, the heavy case, with sibling agents building and running their own instances. Load
  average 2.5–51 during the runs.
- **Round 4 is set aside**: a sibling's new tree made the first two searches of every instance walk ~7,500 frontier
  roots each and hit the 10 s wait, so the walks, not the reload, set its peaks (2.0–2.2 GB in all three conditions).
  Its settled numbers agree with the other rounds and are listed below.

## Results

MiB, verified on release builds of `59854810f` (H, K, X) and the fix commit (F), `memory_diagnostics` with
`rustHeapCensus` plus a `proc_pid_rusage` poller, 2026-09-27. Medians over the clean rounds (H: 1, 2, 3, 5, 6; F: 2, 3,
5, 6; K: 1, 2, 3) with [min–max]:

- **Burst peak**: H **1,946** [1,761–2,176]; F **1,334** [1,300–1,366]; K 1,391 [1,382–1,414]; X (round 1 only) 1,963.
  The fix takes ~610 MiB off the peak, and lands on the no-reload ceiling. Dropping the caller's handle before a full
  reload (X) saved only ~210.
- **The search after a walk**: H **1,427 ms** [1,221–1,677], a full reload in front of the answer; F **107 ms**
  [87–182].
- **Settled footprint**, before → +1 / +5 / +15 min: H 236 → **429 / 408 / 408**; F 243 → **418 / 412 / 405**. Rise at
  +15 min: H 172 [150–191], F 166 [141–166]. Round 4 (set aside): K 435, F 416, H 426 at +15.
- **Heap census at +15 min**: live H 101 [94–124], F 102 [95–124], the same as before the bursts plus ~10; slack H 243
  [214–271], F 237 [221–266]. mimalloc arenas of 128 MiB: H 23 [22–23]; F 15 [14–23]; K 15 [14–23].

So the reload is what grew the heap to 22–23 arenas and what set the peak, and removing it keeps the heap to ~15 arenas
in three rounds out of four. It isn't what the heap settles at: the settled footprint and slack are the same with the
reload, with the fix, and with no reload at all (K).

## What the settled footprint is made of

Round 5 ran a nav-only instance (N: the same bursts without the search, fix binary) beside a full one (F):

- N: footprint 237 before, peak 904, **338 at +15 min** (+101). Live 93 → 91; slack 98 → 188.
- F: 265 before, peak 1,366, **430 at +15 min** (+165). Live 98 → 103; slack 115 → 266.
- S (search-only, round 6, fix binary): 217 before, peak 1,198, **615 at +1 min** (the arena dropped at +30 s but its
  memory hadn't been released yet), **277 at +5 and +15 min** (+60). Slack 74 → 466 → 128.
- F in the same round: 239 before, **380 at +15 min** (+141).

So the search's own memory comes back, within five minutes, once it runs alone. Listing the two big folders is what
leaves ~100 MiB behind for good, and together the two leave more than either alone. None of it involves the reload.

In every condition the retained memory sits outside any page's blocks (`blockSpaceBytes` moves by 5–35 MiB while slack
moves by 90–150), which matches the census's open question about which state mimalloc's retained slices are in. It isn't the walker's thread churn: pooling those threads left the slack where
it was (`walker-thread-pool-2026-09-27.md`).

## Still open

- **The background refresh still rebuilds with the old arena held.** `volumes::get_loaded` starts a full rebuild when a
  warm root arena is 30 s or more behind the writer, and the old arena stays in the map until the new one swaps in: the
  same three-copies peak, now on a timer. Seen in round 4 (a 2.0 s load 30 s into the session). It only runs while
  someone keeps searching past 30 s, and it's what carries deletions into a warm arena, so it isn't simply replaceable
  by a catch-up. Options: catch up on each refresh and rebuild only every few minutes, or rebuild without keeping the
  old arena reachable (serve the answer that triggered it first).
- **The slack after listings**: find out why a big listing's memory stays, and why a search's is released when it runs
  alone but not beside a listing. A forced `mi_collect` after the idle drop (the symbol isn't exported from the release
  binary, so it needs a small build change) would tell unpurged free slices from pages mimalloc still counts as in use.
