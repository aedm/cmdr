# Importance follow-ups

The folder-importance subsystem is shipped: `crates/cmdr-index/src/importance/` scores every folder on a Local or SMB
volume from index rows alone, and its `CLAUDE.md` routes to five area doc pairs (`scorer/`, `store/`, `scheduler/`,
`read/`, `evals/`). The lifecycle bus it rides is in `crates/cmdr-index/src/indexing/DETAILS.md`. Three things are open.

## 1. Tune the importance weights against a real home directory

- **Problem**: The scorer's coefficients are untuned defaults. The whole tuning loop is built (`importance/evals/`: the
  scenario format, hard and soft constraint tiers, and a corpus importer that derives signals through production code;
  plus the `importance-tune`, `importance-snapshot`, `importance-measure`, and `importance-diff` bins in
  `crates/index-query/`), but real corpus dumps are gitignored, so CI runs with zero corpus files and proves only that
  the shape holds.
- **Impact**: Every consumer inherits the ranking: folder-summary gating, event-bundle interest, and media enrichment
  order. A bad weight quietly spends effort on the wrong folders everywhere.
- **Solution**: Follow `docs/guides/importance-evals.md` on David's machine: dump an anonymized snapshot of his home,
  write constraints for folders he knows matter or don't, and iterate the weights with `importance-tune`'s `explain`
  breakdowns. If a change genuinely improves the aggregate, raise `SOFT_SCORE_FLOOR` in the same commit
  (`evals/CLAUDE.md`).
- **Size**: M. Blocked on David: it needs his own home directory and his judgment on what matters.

## 2. Measure what the Spotlight last-used sampler costs on a real home

- **Problem**: `importance/last_used.rs` samples `kMDItemLastUsedDate` for at most `SAMPLE_CAP` (500) folders per pass.
  Both the cap and the sampling strategy are guesses (`importance/DETAILS.md` § "Sampled `kMDItemLastUsedDate`").
- **Impact**: Probably small. Sampling runs only where the volume says `last_used_available`, so SMB never pays it and
  the cost stays on the boot disk. But an unmeasured cap could be either wasting seconds per pass or starving the
  recency signal.
- **Solution**: Run `importance-measure` against a real Spotlight index at a few cap values, record the sampling phase's
  wall-clock share in `docs/notes/`, and pick the cap from the data. ⚠️ This is separate from the shipped first-run
  recency signal (`apps/desktop/src-tauri/src/priority/DETAILS.md` § "The recency signal"), which asks Spotlight at
  launch before any index exists; don't merge the two.
- **Size**: S. Not blocked.

## 3. Let the memory watchdog and shutdown stop an importance recompute

- **Problem**: Every other long walk in the crate runs under a `CancellationToken` rooted at the volume. An importance
  pass runs under nothing and registers no stop hook, so `stop_all_indexing` (the memory watchdog's emergency stop and
  the shutdown path) doesn't reach it, and a running pass walks the whole index to the end.
- **Impact**: Low today: a full pass takes 5.5–6.4 s on real 391k- and 611k-folder indexes (measured 2026-07-29), and an
  incremental one takes microseconds. It becomes real if a pass grows to minutes or a watchdog stop visibly fails to
  free memory.
- **Solution**: Thread a child of the volume's token into the pass and poll it in `recompute_folders` (the
  `TODO(importance)` in `importance/scheduler/recompute.rs`), and register a stop hook. ❌ Don't add a second
  cancellation primitive. Fix shape: `importance/scheduler/DETAILS.md` § "A pass can't be stopped".
- **Size**: S. Not blocked; do it when the trigger above fires, or alongside other scheduler work.
