# Idle-cost follow-ups

Cmdr's idle bill got two structural fixes, both documented beside the code: each CLIP tower loads on demand
(`crates/cmdr-index/src/media_index/clip/DETAILS.md` § "What holding the towers costs"), and a storm of one-shot rescan
anchors costs one sweep a day (`crates/cmdr-index/src/indexing/reconcile/reconciler/rescan/DETAILS.md` §
"Anchor-cardinality routing"). Before measuring anything below, read `docs/notes/idle-cpu-attribution-2026-08-03.md`:
four hypotheses about where idle CPU goes were refuted by measurement, and each left a rule (never rank work off one
`sample` window, never report a share as CPU when the leaf frame is a syscall, never infer CPU from log volume).

## 1. Take a fresh idle-cost baseline on a quiet machine

- **Problem**: Every idle-cost number the team ranks work against comes from one v0.37.0 measurement on 2026-08-03, on a
  machine that was also running six cargo builds: 110 minutes of CPU over 9.1 hours, at a 1.78 GB footprint. Since then
  the sync-status poll, the live tick's folder-score re-read, the SQLite page-cache slab, the CLIP text tower, and the
  rescan drain have all changed.
- **Impact**: Nobody knows where the idle bill stands, so nobody can say which of the items below are worth doing.
- **Solution**: Run a real prod build idle on David's laptop with no builds running, measure CPU and footprint over
  several hours following the attribution note's rules, and record it as a dated `docs/notes/` note. Re-rank the other
  items against it.
- **Size**: S (half a day, mostly waiting). Blocked on David: it needs his laptop and a quiet stretch.

## 2. Decide whether an idle CLIP tower should unload itself

- **Problem**: Once loaded, a CLIP tower stays resident for the process lifetime. The text tower is about 80% of that
  bill (`clip/DETAILS.md` § "What holding the towers costs").
- **Impact**: Unloading a tower nobody has used in N minutes would free that memory for users who search rarely, but a
  cold first query costs 677 ms against 8–10 ms warm (`clip/DETAILS.md` § "The query path"). An idle unload charges that
  on every reload instead of once per launch.
- **Solution**: If David accepts ~677 ms on the first semantic search after an idle gap, add an idle timer per tower in
  `clip/towers.rs` that drops it after N minutes without an `encode`, and reuse the existing on-demand load.
- **Size**: S to build. Blocked on a David decision (the latency trade), ideally informed by item 1.

## 3. Reconsider CLIP's `MLComputeUnits` (about 400 MB at stake)

- **Problem**: The towers load with `MLComputeUnits::All` (`clip/macos.rs`). `CPUOnly` and `CPUAndNeuralEngine` measure
  11.8 MB against `All`'s ~410 MB, because the GPU path materializes every weight matrix instead of reading the mmap'd
  `weight.bin`.
- **Impact**: Roughly 400 MB for every user who has used semantic search, if the non-GPU path keeps enrichment
  throughput and query latency acceptable. Nobody has measured that side.
- **Solution**: Measure enrichment throughput and query latency on `All` against `CPUAndNeuralEngine` on the same corpus
  (the `CMDR_CLIP_COMPUTE_UNITS` override in `clip/macos.rs` exists for this), then decide. ❌ Don't change it on the
  memory number alone; `media_index/clip/CLAUDE.md` carries that as an invariant.
- **Size**: M (the measurement is the work). Blocked on the throughput measurement, then a David decision.

## 4. Try an fp16 CLIP text tower

- **Problem**: The text tower ships in full precision (~184–246 MB resident). 8-bit palettization was rejected because
  its Core ML inference comes out all-NaN (`clip/install.rs`); fp16 sits between the two and was never tried.
- **Impact**: Would roughly halve what the text tower costs a user who searches, if quality holds.
- **Solution**: A spike: convert the text tower to fp16 with `apps/desktop/scripts/convert-clip-model/`, check it's
  NaN-free and that its embeddings keep cosine ≥0.99 against the full-precision tower on a query set (the same gate the
  image tower's palettization used), then measure resident size. Shipping it changes `CLIP_MODEL_ID`, which re-embeds.
- **Size**: M. Not blocked.

## 5. Set the rescan-storm threshold from a real week of data

- **Problem**: `HIGH_CARDINALITY_ANCHORS` is 256 (`indexing/reconcile/reconciler/rescan/cardinality.rs`), picked with no
  distribution behind it. The only anchor-cardinality data in the repo is from David's machine on 2026-07-19..23 while
  it ran six cargo builds: 5,876 distinct anchors across a sampled day, 1,595 in the worst single window.
- **Impact**: Too low and ordinary churn falls into the once-a-day sweep, so the index goes staler than it needs to; too
  high and a storm still costs one subtree walk per anchor.
- **Solution**: Collect a week of the INFO line `churn.rs` already emits on a quiet machine, write the distribution to
  `docs/notes/`, and re-set the constant from it (a one-line change).
- **Size**: S (half a day of analysis after a week of collection). Blocked on the week passing on David's machine.

## 6. May the rescan walk skip `SYSTEM_DIR_EXCLUDES`?

- **Problem**: `SYSTEM_DIR_EXCLUDES` (`indexing/scanner/exclusions.rs`) is a name denylist (`target`, `node_modules`,
  `Caches`, and so on) already read by search, the importance scorer, and the folder-size tooltip. Its docstring says
  the SCANNER must never read it, because skipping at walk time stamps coverage on parents whose `dir_stats` come out
  short. That reason doesn't obviously transfer to RE-walking already-indexed ground, where the aggregates exist and
  skipping costs staleness, not a wrong count.
- **Impact**: Rescans could skip the heaviest churn (build output, caches) that nobody browses. Nothing depends on the
  answer: the shipped rescan router bounds a rate and never needs to know which folders.
- **Solution**: Decide; if yes, have the rescan walk consult the list and document the carve-out beside the docstring.
- **Size**: S to build. Blocked on a David decision.

## 7. Drop the `ORDER BY` from `ImportanceIndex::above_threshold` if nobody needs it

- **Problem**: `above_threshold` sorts every scored folder (`importance/read/mod.rs`, `ORDER BY score DESC, path ASC`),
  but its main consumer (`media_index`'s cached `coverage::importance_scores`) builds a `HashMap` and never reads the
  order. The public API's ordering is asserted by a test, and other callers exist.
- **Impact**: Slight: a sort per call over up to ~90k folders, and `cache_size` also sets SQLite's sorter budget
  (`crates/cmdr-fs/src/sqlite_util.rs`).
- **Solution**: Audit the callers; if none needs the order, split an unordered variant for the map-building consumer (or
  drop the sort and the ordering assertion).
- **Size**: S. A small API decision, not blocked.

## 8. Should `derive-default-justified` cover IPC DTOs?

- **Problem**: The check scans only `file_system/` in the app and all of `cmdr-fs`
  (`scripts/check/checks/desktop-rust-derive-default-justified.go`, `deriveDefaultTrees`), so a `Default` derive on an
  IPC DTO goes unchallenged while its `cmdr-fs` twin needs a `DEFAULT-OK` line. The check's comment argues the fault
  class is "a type that carries a fact about a file" and that widening buys nothing, but a DTO twin of a `cmdr-fs` type
  carries the same fact.
- **Impact**: Small: a zero value on a file-fact DTO could slip past review.
- **Solution**: Look at the DTOs that mirror `cmdr-fs` types; if any carries a file fact, widen the scan to them (not
  the whole workspace), else record why they're out in the comment.
- **Size**: S. Not blocked.
