# Sealed subtrees: follow-ups

A single directory can hold a pathological number of files: Google Drive's `fetch_temp` held 1.14M empty files and
caused a 7-minute, 1 GB cold-start stall. The guard that bounds that stall is built: two pure teeth in
`crates/cmdr-index/src/indexing/watch/event_loop/verify_guard.rs`, documented (incident, constant, honest costs, and the
SQL census that measures such directories) in `crates/cmdr-index/src/indexing/reconcile/DETAILS.md` § "Bounding
verification cost (the two teeth)". The measurement spikes are in `docs/notes/reanchor-cost-spike.md` and
`docs/notes/churn-observability-spike.md`; the churn instrumentation they used is `indexing/watch/churn_monitor.rs`
(read-only, off unless `CMDR_CHURN_SPIKE` is set). What's open is whether to go further and "seal" such subtrees.

⚠️ `file.rs:NN` references here date from 2026-07-20 and many have drifted; resolve the named SYMBOLS with
`codegraph_search` and re-derive any number before quoting it.

## 1. Measure the residual cost of pathological directories now that verification is guarded

- **Problem**: Sealing (item 2) is a large change to the `dir_stats` ledger, the part of indexing with four documented
  size-corruption incidents. The verification guard already removed the measured incident, and nobody has checked
  whether what's left still hurts. The guard fixes the STALL, not the class: it doesn't reclaim the search index's RAM
  (`fetch_temp` was ~16% of the 6.95M rows search loads), it guards only `verify_affected_dirs` (a shallow
  `MustScanSubDirs` still re-walks via `start_scan`, and `reconcile_subtree` still diffs on a deep anchor), and a first
  scan or any `clear_index` still fully indexes 1.14M empty files.
- **Impact**: Decides whether item 2 is ever built. "The guard alone was the whole fix" is a live answer.
- **Solution**: Run the SQL census from the reconcile `DETAILS.md` section above on a real machine (2026-07-21: 29
  directories at ≥ 10,000 children, all lower bounds), read the guard's own counters (`verifyDeclinedDirs`,
  `verifyTruncatedDirs` on `cmdr://indexing?volume=<id>`), and observe cold-start time, first-scan time, and search-index
  RAM attributable to those directories. ❌ Don't build the `huge_dirs_seen` walker counter the original plan asked
  for: it reads zero on an established machine, and the census answers the same question with no code.
- **Size**: S. Not blocked. Ends in a David decision: build item 2 or close it.

## 2. Seal pathological high-churn subtrees: keep the aggregate, drop the per-file tail

- **Problem**: Re-syncing a directory costs O(children), not O(events), because the per-child events were dropped and
  all you can do is `readdir` and diff. So throttling can't fix a 1M-entry directory; it converts a continuous trickle
  into a periodic stall. It's a recurring class, not a Google Drive quirk: one machine had four unbounded scratch
  directories from four vendors (`fetch_temp` 1.4M, WebKit `Resource` 119k, Chrome `Cache_Data` 107k, cargo's
  `target/debug`). Excluding them isn't an option: exclusion breaks size truth for every ancestor up to `~` (harmless
  for 0-byte `fetch_temp`, a silent 50+ GB lie for `target`), and today everything inside the home folder is truthful.
- **Impact**: Bounds the index's rows, RAM, and resync cost for the worst directories while keeping every ancestor's
  size honest.
- **Solution**: A sealed subtree keeps its `dir_stats` aggregate (so size truth reaches `~`) but stores per-file rows
  only for files ≥ 64 KB, capped at the largest 10,000 per sealed subtree. Sealing is a mode, not a deletion: FSEvents
  keep flowing (to measure churn and credit the aggregate between re-anchors); only per-file rows and per-child diffing
  stop. Sustained quiet unseals it. Design notes below.
- **Size**: XL (one landing unit; every phase touches the single-writer `dir_stats` ledger, so no parallel agents).
  **Blocked** on item 1, and on David decisions 1–3 below.

### Design notes

**A sealed dir is a recompute boundary.** The ledger invariant `dir_stats ≡ recompute-from-entries` becomes "…with
sealed dirs as opaque leaf constants". Otherwise `repair_dir_stats_upward` recomputes a sealed dir from its surviving
rows and wipes the aggregate on the next ancestor walk. Sites:

- `aggregator/mod.rs::compute_bottom_up`: every full, partial, subtree, and backfill aggregate funnels through it.
- `writer/repair.rs::recompute_dir_stats_from_children`, including its two sub-recomputes
  (`recompute_recursive_has_symlinks`, `IndexStore::recompute_min_subtree_epoch`), or a sealed dir loses a true symlink
  flag and gets its epoch recomputed from survivors.
- `writer/delta.rs::propagate_min_subtree_epoch`, an independent up-walker.
- `stress_test_helpers::check_db_consistency`, the oracle, which fails on any sealed tree otherwise.
- **Delete deltas, a different category**: `handle_delete_subtree_by_id` computes its negative delta from
  `IndexStore::get_subtree_totals_by_id`, a recursive CTE over `entries` rows. Deleting a sealed dir or any ancestor
  credits back only the surviving rows' bytes, inflating every ancestor to `~` permanently. The move path already reads
  the stored aggregate; deletes must do the same for a sealed root.
- Thread the sealed set as a REQUIRED parameter (like `listed_epochs`), and pass the sealed VALUES
  (`HashMap<i64, DirStatsById>`, read before the pass), not just ids: `compute_bottom_up` has no connection, so knowing
  an id is sealed doesn't tell it what to write. This is what makes `AggSource::Maps` safe: a fresh full scan's maps
  never saw the collapsed files.

**Every walker must respect seal state, or it resurrects the rows**: `local_reconcile` (the most likely way sealing gets
undone, since a rescan of a completed index takes this path), `reconciler::reconcile_subtree`, `scanner::scan_subtree`,
the guarded walker (which must also compute and write the sealed aggregate as a streaming sum), the per-navigation
`verifier.rs`, `verify_affected_dirs` (post-seal the DB-side probe can't fire, so only the disk-side cap stands guard),
and the live create path, which needs a row-less credit path re-anchored at the seal-root id (a new message shape, since
`PropagateDeltaById` keys off an entry id a collapsed file doesn't have). The seal-state consumers must land before
anything writes a seal.

**Seal state**: a dedicated table (a new table materializes on existing DBs with no `SCHEMA_VERSION` bump, because
`create_tables` runs `CREATE TABLE IF NOT EXISTS` on every open before the version check). Keyed per Decision 1. An
unresolvable seal row must be loud and fail-safe, ❌ never "recompute this dir from children" (don't inherit
`compute_partial_aggregates_sql`'s silent-skip idiom; here a skip is destructive). The path fallback is resolved with
`resolve_path_under(conn, ROOT_ID, path)` in the volume's `IndexPathSpace`.

**The size cut** (measured over 6,681,172 files, 2,373 GB): ≥ 64 KB keeps 99% of bytes in 7% of rows; ≥ 1 MB would
drop 130 GB of visible truth. On pathological subtrees the 10,000 CAP binds, not the threshold, so most of a sealed
subtree's bytes can sit in collapsed files and drift applies to the bulk of the aggregate. Sealing indexed rows is one
`DELETE … NOT IN (top 10,000)`; scanning an already-sealed subtree needs a bounded min-heap that buffers writes to the
end of the walk (and a shared heap across the parallel walk means hot-path contention).

**Where to seal**: the highest node whose subtree is uniformly churny (for `something/cache/{hex}/{hex}/{hex}.tmp`,
seal `cache`, never `something`). ⚠️ Churn share ALONE over-climbs on real data: it picked `~/Library/Containers` for
`fetch_temp` and `~/Library/Caches` for the WebKit cache, because the siblings happened to be quiet. Combine churn share
with a content ratio (entries and/or bytes below the candidate vs. below its parent). Hard stops, belt-and-braces only:
`~`, `~/Documents`, `~/Desktop`, `~/Downloads`, `~/Pictures`, `~/Library/Containers`, `~/Library/Caches`, any volume
root, plus a Linux counterpart or an explicit macOS-only note. `pick_seal_root`: pure and clock-injected.

**First scan, no seed list (settled)**: provisionally seal on SIZE at scan time; only churn confirms. A quiet 60k-file
photo library gets provisionally sealed, goes quiet, and unseals (~1.5× one subtree scan). No hardcoded path list: it
would hide classifier failures on the two cases we understand, and it contradicts the "the OS churn signal
self-identifies busy subtrees" principle. Classification is fast: `fetch_temp` separated within 10 s, `target` within
31 s.

**Drift and re-anchor**: deletes of collapsed files resolve to nothing, so the aggregate inflates monotonically on
exactly the churny dirs (DriveFS renames are delete+create pairs). The re-anchor is the primary correctness mechanism,
and it's a streaming `readdir` + sum with zero DB reads or writer messages. Measured (`reanchor-cost-spike.md`): 96–181 s
wall for 1.44M entries, flat 128 KiB, about a quarter of the verification pass it replaces. Conditions: cadence per
directory from its own walk cost (1.9 µs/entry at 100k, 80 µs at 1.43M); split into a cheap count pass (~17× cheaper,
hourly) and a byte pass (every 6–12 h); cap the walk and degrade to the approximate state when over budget.
`file_count_delta` drifts too, which matters for `expected_totals`. Unseal with hysteresis (seal fast, unseal slow,
cooldown); the constants are still unmeasured.

**The approximate state must land in this unit, not later**: post-seal `min_subtree_epoch > 0` reads as
`recursive_size_complete = true`, so `expected_totals::per_source_contribution` must return `None` for a sealed subtree,
or copy progress overshoots or parks at 100%.

**Non-issues and costs**: the search-index reload adds no new trigger and gets cheaper (fewer rows); the real cost is
the one-time delete/insert burst. A sealed subtree still raises `MustScanSubDirs` and burns a 60 s throttle drain slot
on a walk that now does nothing; worth measuring, not a correctness issue.

**Tests (test-first)**: table-driven seal-root selection (flat dir seals itself; `cache` not `something`; the quiet
photo library unseals; hard stops never selected); clock-injected seal/unseal state machine including thrash; creates
credit, deletes drift and the re-anchor corrects them; sealing leaves every ancestor byte-identical, is idempotent,
survives a writer restart, passes `check_db_consistency`; a full rescan or `local_reconcile` doesn't resurrect rows.

**Also update**: the `indexing` MCP surface, `docs/tooling/logging.md` and `index-query` (row count no longer matches the
aggregate, which reads as corruption), `docs/architecture.md`, and `enrichment.rs`'s integer-keyed batch path.

**Out of scope**: SMB/MTP (verification is root-only), a settings UI (Decision 5), replacing the existing throttles.

### Decisions for David

1. **Seal-state identity.** Recommendation: key by `entry_id` with path as a rebuild-after-`clear_index` fallback. Pure
   path keying loses the aggregate on any ancestor rename; pure id keying doesn't survive `clear_index`.
2. **Do directory rows survive a seal?** Recommendation: keep them, collapse only files. Dropping them breaks
   `resolve_path` under the seal root, and with it re-entry, delta targeting, `recursive_dir_count`, and unseal scoping.
   Cost: a dir-heavy tree reduces far less than flat `fetch_temp`.
3. **Unseal on navigate?** Recommendation: narrow it to "a sealed CHILD whose size is being displayed". Listings come
   from the filesystem, so a full unseal on `cd` would re-insert 1.14M rows on the user's path: the incident, on demand.
4. (Settled) No seed list, see above.
5. **Settings UI in v1?** Recommendation: no. A path/throttle ignorelist is a developer control, users find it only
   after the damage, and it cuts against the churn-signal principle. If control is wanted later, a right-click "index
   this folder less often" with three named choices beats a pattern table.

## 3. Show sealed folders as approximate, and say they're unsearchable

- **Problem**: Once item 2 ships, a sealed folder's size is permanently approximate and its collapsed files can't be
  found by search, and nothing in the UI says so.
- **Impact**: Without it, sealing silently lies in two places the user trusts: folder sizes and search results.
- **Solution**: A distinct third size state threaded through `DirStats`, `FileEntry`, the specta bindings,
  `full-list-utils.ts`, and `sorting.rs::known_dir_size`. ❌ Don't reuse the `recursive_size_pending` hourglass: it means
  "writes in flight right now", and a sealed folder would show a forever-spinning hourglass. Search disclosure needs a
  new field or a redesign: `uncovered_scopes` is populated per scope path and only when the whole volume is unindexed,
  so an unscoped root search yields nothing for a sealed subtree inside root. New strings need `@key` descriptions and a
  `bindings.ts` regeneration. Tests: component tier plus an IPC-contract test on the new `DirStats` field; no E2E (the
  Playwright lane can't produce a sealed folder without a dev-only seal hook).
- **Size**: M. **Blocked** on item 2 (the backend's approximate state, including `per_source_contribution`, lands there).
