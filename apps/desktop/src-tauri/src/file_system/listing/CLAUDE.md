# File system listing module

Backend directory reading, caching, sorting, and streaming: 100k+ entries, non-blocking, with progress.

## Module map

- Read and serve: **reading.rs** disk I/O, **streaming.rs** async progress and cancellation (`ListingEventSink`),
  **operations.rs** the sync API, **cached_listing.rs** `CachedListing` + `LISTING_CACHE`, **caching.rs** patch helpers,
  **orphan_reaper.rs** the 6 h backstop, **mutation.rs**, **foreign_path.rs** stored spellings.
- Derive and emit: **diff.rs** `compute_diff`, **diff_emitter.rs** 50 ms coalescing, **visible_rows.rs** /
  **path_index.rs** the row and path maps, **sorting.rs** the one comparator, **collation.rs** the one name order, plus
  **listing_host.rs**, **brief_columns.rs**, **fuzzy_jump.rs**. `FileEntry` is `cmdr-fs`'s, as `listing::metadata`.

## Invariants and gotchas

- **Neither a row number nor a path indexes `entries`.** Rows drop hidden entries and in-flight scratch, so
  `CachedListing::rows` is the ONLY filter point, on READ; by-path callers go through `index_of_path` /
  `indices_of_paths`. ❗ A MUTATING caller resolves BEFORE `entries_mut`, which drops both maps. `entries` stays
  private: accessors that grew their own filter were each a row off.
- **A watcher diff updates the cache, then emits what the PANE shows** via `diff_emitter::enqueue_diff` (❌ never
  `app.emit`: no coalescing, flicker). Its index is a pane row (`CachedListing::pane_rows`), ❌ never an entry index:
  build with `DiffChange::for_pane` or `compute_diff(.., include_hidden)`. `DETAILS.md` § "Diffs speak the pane's
  rows".
- **Refreshes of ONE directory stay serialized** (`notify_full_refresh`), or an older read lands last.
- **`listing_overlays::decorate` folds in rows no volume holds**, between enrich and the sort, in all THREE read paths
  (`streaming.rs`, `operations.rs`, the watcher's full refresh); miss one and a refresh strips them.
- **A close notifies `crate::listing_lifecycle` AFTER the cache removal**, ❌ never before: an observer's detached arm
  reconciles against cache membership.
- **The orphan reaper keys on `last_accessed_ms`, not `created_at`**: every read accessor and cache patch bumps it, or
  it evicts a live pane. ❌ Never from `refresh_listing_index_sizes` (background work).
- **`read_directory_with_progress` holds a `priority::foreground` lease** for its whole body: ❌ never bind it to `_`.
- ❌ **The `select!` cancel arm must never `listing_task.abort()`**: returning detaches a safely-unwinding task,
  aborting wedges an MTP phone mid-round-trip.
- **FullRefresh goes through `caching::spawn_full_refresh`**: on a watcher's OS thread a bare `tokio::spawn` panics.
- **Sorting has ONE comparator**, `entry_comparator` over `SortableEntry` (`FileEntry` plus a search-results row). ❌
  Never add a second. A sort change invalidates the frontend's cached range, so bump `cacheGeneration`.
- **Names rank by Unicode collation (`collation.rs`), ❌ never code points or `to_lowercase`** (macOS holds NFC and NFD
  side by side). Both readings, live `compare` and prebuilt `key`, end with a raw-bytes tiebreak, else two spellings of
  one name tie and the watcher sees a phantom `Move`. ❌ Never persist a `NameKey`.
- **A listing's path is a `ListingPath`, built only by `ListingPath::on_volume`** (the volume's one spelling,
  `Volume::listing_path`). ❌ Never compare a raw path to it: MTP reports at `mtp://…` while a pane entered with Enter
  holds `/DCIM`, and a verbatim match drops every Cmdr-made delete on that pane.
- **Only a pane open or a Finder drop/paste resolves a foreign spelling** (`list_as_stored`, `stored_spellings`): ❌
  never a walker, scan, or refresh, where a miss means gone and a resolve returns a look-alike twin. `DETAILS.md` § "A
  pane path the volume stores another way".
- **New listing state hangs off a struct, not a `static`**; fixtures use `caching_test_support::TestListing`.
- **Finder tags are deferred**: `list_directory_core` never reads them, and every modify path calls
  `carry_forward_tags` BEFORE storing, else an mtime touch blanks a file's dots. ❌ Never route enrich through it.

Data flow, the caching lifecycle, row numbers, entries by path, the overlay step, sorting, collation, and the decisions
behind them: `DETAILS.md`. Read it before any non-trivial work here: editing, planning, reorganizing, or advising.
