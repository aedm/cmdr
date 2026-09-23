# DB-first directory listings

Serve a directory listing from the volume's SQLite index instead of `readdir` + `stat`, so the first paint is a query
rather than a filesystem walk. Background verification on each navigation keeps the index honest.

**Status, re-derived from the tree 2026-09-23**: not started. Nothing in `file_system/listing/` reads the index for
entries. Its prerequisite, the per-navigation verifier, is built and in production
(`crates/cmdr-index/src/indexing/reconcile/verifier.rs`).

Read `crates/cmdr-index/src/indexing/CLAUDE.md` and `crates/cmdr-index/src/indexing/reconcile/CLAUDE.md` before planning
any of this.

## Why it might still be worth doing

`readdir` + `stat` costs a walk per navigation; an indexed lookup on the same directory is one tree walk to an entry id
plus one child query. For rapid keyboard navigation through large directories that difference is the whole feel of the
app.

⚠️ **There is no trustworthy motivating measurement.** The only number ("2–50 ms per directory", March 2026) is
unanchored: no build mode recorded, no directory sizes, and the listing path has been rewritten since.
`docs/notes/listing-wedge-impact-2026-08-22.md` is the reason to be careful with any older listing number: it measured
release builds against debug builds on the same probe and found release roughly two orders of magnitude faster. ❌ Don't
schedule this work off the March figure. Measure a release build first, at several directory sizes, and decide whether
there is a user-visible win left to win.

## What the index already gives a DB-first path

- **The per-navigation verifier.** `crates/cmdr-index/src/indexing/reconcile/` holds three resync mechanisms (the
  event-triggered reconciler, the full rescan-in-place, and `verifier.rs`'s per-navigation `read_dir` diff), each with
  guardrails its `CLAUDE.md` states as must-knows. It's the safety net a DB-first paint leans on.
- **Logical and physical sizes, separately.** `EntryRow` carries `logical_size` and `physical_size`; `DirStatsById`
  carries `recursive_logical_size` and `recursive_physical_size`. A DB-first listing can show file bytes where the user
  expects them.
- **Honest recursive sizes.** `listed_epoch` per directory and `min_subtree_epoch` rolled up through `dir_stats` answer
  "is this total exact or a lower bound", which the frontend already renders. A DB-first path inherits this.

## Constraints from today's index

- **The `entries` table is id-keyed, not path-keyed.** There is no `parent_path` or `path` column. The shape is
  `store::resolve_path(conn, path) -> Option<i64>` (one tree walk), then `IndexStore::list_children(parent_id)`.
  `EntryRow` (`indexing/store/mod.rs`) carries `id`, `parent_id`, `name`, `is_directory`, `is_symlink`, `logical_size`,
  `physical_size`, `modified_at`, and `inode`.
- **The index is per-volume.** Reads go through `get_read_pool_for(volume_id)`, paths map into the volume's own index
  path space via `routing::index_read_path`, and a volume with no registered index answers `None`. Any DB-first check is
  a per-volume question before it is a per-directory one.
- **`enrich_entries_with_index_on_volume` already does most of the plumbing.** It resolves the parent path to an id
  once, lists the child directory `(id, name)` pairs, and batch-fetches `dir_stats` by integer id: two indexed queries
  for a whole listing. A DB-first read is that same resolve-then-list, taken one step further to build entries instead
  of enriching them. ❌ Don't build a second path-resolution route beside it.
- **`FileEntry` carries fields beyond the index columns**: `is_archive`, `inode`, `physical_size`, `tags`,
  `recursive_has_symlinks`, `recursive_size_complete`, and `recursive_size_stale`. Most are derivable or already
  enriched: `icon_id` and `is_archive` are pure functions of the name and the directory flag, `inode` and both sizes are
  columns, `tags` are deferred on every path already (`file_system/listing/DETAILS.md` § "Finder tags"), and the
  recursive fields come from the same `dir_stats` batch enrichment does today.
- **❗ `created` is a sort column, and the index does not store it.** `SortColumn` is
  `'name' | 'extension' | 'size' | 'modified' | 'created'`. A DB-first listing with `created_at: None` on every entry
  would silently produce a wrong sort for anyone who chose that column. This blocks the switch: either backfill
  `created_at` into the index, or fall back to `readdir` for a listing sorted by `created`. `permissions`, `owner`,
  `group`, `added_at`, and `opened_at` are undisplayed and can take defaults.
- **Post-replay verification** lives in `indexing/watch/event_loop/verification.rs` (`verify_affected_dirs`).

## What to build

**The DB-first read path.** For an indexed directory on a volume with a registered index, build the listing from
`list_children(parent_id)` instead of the backend's `list_directory`, then enrich, sort, cache, and return exactly as
today. Fall back to `readdir` per-directory whenever the answer isn't available.

**The readiness predicate.** Two questions have to be separated:

- _Has this volume finished a first full scan?_ `IndexStatus::scan_completed_at`. Before it is set, a directory can
  legitimately hold only some of its children, and a DB-first paint would jump when verification corrected it.
- _Has this specific directory ever been listed?_ The `listed_epoch` column on `entries` answers exactly this and
  distinguishes "genuinely empty" from "never walked". `0` means never listed. ⚠️ The verifier already keys off this
  pairing and its `CLAUDE.md` states the rule in both halves; keep the predicates in lock-step rather than inventing a
  third one.

**The cache-update path when verification finds a diff.** Compare against the CURRENT cache rather than the original DB
snapshot, so a change the per-directory watcher already applied noops instead of being processed twice.

**Streaming.** `list_directory_start_streaming` needs the same fork: if the DB answers, populate the cache and emit
`listing-complete` without progress events, since a sub-millisecond read has nothing to report.

**Then measure.** Benchmark against `readdir` at 100, 1,000, 10,000, and 100,000 entries on RELEASE builds, and check
for lock contention between DB-first reads and the writer during a concurrent scan.

## The watcher-dedup follow-up, and why it isn't a clean trade

The tempting follow-up: once DB-first is active for a volume, stop starting a per-directory `notify` watcher for
directories on it, since the volume-level watcher covers them and 300 ms of FSEvents batching is imperceptible.

The per-directory watcher also carries Finder tags forward across a re-stat (`caching::carry_forward_tags`) and drives
the in-place listing diffs several features read. Anyone picking this up has to inventory what depends on that watcher
before removing it.
