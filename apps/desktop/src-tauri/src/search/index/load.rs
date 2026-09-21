//! Reading a volume's index database into a [`SearchIndex`].
//!
//! The cost here is I/O latency, not compute, so the read is split across rowid
//! ranges rather than made a tighter loop. Why, and what the split costs in transient
//! memory: `search/DETAILS.md` § "Loading in parallel".

use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

use super::{OptU64, SearchEntry, SearchIndex};
use crate::index_host::index;
use crate::pluralize::pluralize_with;
use cmdr_index::ReadPool;
use cmdr_index::store::{IndexStore, ROOT_ID};

/// Rows between cancellation checks during load.
const CANCEL_CHECK_INTERVAL: usize = 100_000;

/// Most workers a parallel load will use.
///
/// The bottleneck is the SSD, not the CPU: one thread issuing serial 4 KiB `pread`s
/// against a cold index never builds enough queue depth to reach the drive's real
/// throughput (measured: ~14-26 MB/s effective against a drive that does GB/s,
/// `ERR-S76V3`, 2026-09-19). Eight readers is where queue depth stops being the
/// limit; more would only multiply the transient merge cost below.
const MAX_LOAD_WORKERS: usize = 8;

/// Below this many rows a load stays single-threaded: splitting a small index costs
/// more in connections and segment merging than the scan itself takes.
const PARALLEL_LOAD_THRESHOLD: usize = 50_000;

/// How many ranges the last load split into. Test-only, and the reason it exists is
/// that a test asserting a parallel load matches a serial one proves nothing if the
/// fixture quietly took the single-worker path.
#[cfg(test)]
static LAST_LOAD_WORKERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

// ── Index loading ────────────────────────────────────────────────────

/// One worker's contiguous slice of the arena.
///
/// `name_offset` on these rows is relative to **this segment's** `names`, and gets
/// rebased onto the merged arena in [`merge_segments`].
struct Segment {
    names: String,
    entries: Vec<SearchEntry>,
}

/// How many rows to reserve for, without a full `COUNT(*)`.
///
/// **Gotcha**: `SELECT COUNT(*)` is not the cheap b-tree count it looks like. SQLite
/// serves it by fully traversing the smallest index (`idx_inode` here), which is warm-
/// cache cheap and cold-cache seconds of I/O, all to size a `Vec`. The aggregator has
/// already counted the same rows into `dir_stats` for the scan root, so one indexed
/// row read answers it. `COUNT(*)` stays as the fallback for an index whose root
/// rollup hasn't been written yet (a first scan still running).
fn row_count_estimate(conn: &rusqlite::Connection) -> usize {
    if let Ok(Some(stats)) = IndexStore::get_dir_stats_by_id(conn, ROOT_ID) {
        let total = stats.recursive_file_count.saturating_add(stats.recursive_dir_count);
        if total > 0 {
            return usize::try_from(total).unwrap_or(usize::MAX);
        }
    }
    conn.query_row("SELECT COUNT(*) FROM entries", [], |r| r.get::<_, i64>(0))
        .map(|n| n.max(0) as usize)
        .unwrap_or(0)
}

/// Read the rows with `lo <= id < hi` into a segment.
///
/// `ORDER BY id` is free (`id` is the rowid, so this is the table's own scan order)
/// and says out loud that [`SearchIndex::index_of_id`]'s binary search depends on it.
fn load_range(
    pool: &ReadPool,
    lo: i64,
    hi: Option<i64>,
    reserve_rows: usize,
    cancel: &AtomicBool,
) -> Result<Segment, String> {
    pool.with_conn(|conn: &rusqlite::Connection| {
        const AVG_NAME_BYTES: usize = 20;
        const NAMES_ARENA_CEILING: usize = 512 * 1024 * 1024; // 512 MiB

        let mut names = String::with_capacity(reserve_rows.saturating_mul(AVG_NAME_BYTES).min(NAMES_ARENA_CEILING));
        let mut entries = Vec::with_capacity(reserve_rows);

        let sql = match hi {
            Some(_) => {
                "SELECT id, parent_id, name, is_directory, logical_size, modified_at \
                 FROM entries WHERE id >= ?1 AND id < ?2 ORDER BY id"
            }
            None => {
                "SELECT id, parent_id, name, is_directory, logical_size, modified_at \
                 FROM entries WHERE id >= ?1 ORDER BY id"
            }
        };
        let mut stmt = conn.prepare(sql).map_err(|e| format!("Prepare failed: {e}"))?;
        let mut rows = match hi {
            Some(hi) => stmt.query(rusqlite::params![lo, hi]),
            None => stmt.query(rusqlite::params![lo]),
        }
        .map_err(|e| format!("Query failed: {e}"))?;

        let mut row_count = 0usize;
        while let Some(row) = rows.next().map_err(|e| format!("Row read failed: {e}"))? {
            if row_count.is_multiple_of(CANCEL_CHECK_INTERVAL) && cancel.load(Ordering::Relaxed) {
                return Err("Load cancelled".to_string());
            }

            let id: i64 = row.get(0).map_err(|e| format!("{e}"))?;
            let parent_id: i64 = row.get(1).map_err(|e| format!("{e}"))?;
            // Borrow directly from SQLite's internal buffer via ValueRef: zero heap allocations.
            let name_ref = row.get_ref(2).map_err(|e| format!("{e}"))?;
            let name_str = name_ref.as_str().map_err(|e| format!("{e}"))?;
            let name_offset = names.len() as u32;
            let name_len = name_str.len() as u16;
            names.push_str(name_str);
            let is_directory: bool = row.get(3).map_err(|e| format!("{e}"))?;
            let logical_size: Option<u64> = row.get(4).map_err(|e| format!("{e}"))?;
            let modified_at: Option<u64> = row.get(5).map_err(|e| format!("{e}"))?;
            entries.push(SearchEntry {
                id,
                parent_id,
                name_offset,
                name_len,
                is_directory,
                size: OptU64::new(logical_size),
                modified_at: OptU64::new(modified_at),
            });
            row_count += 1;
        }

        Ok(Segment { names, entries })
    })?
}

/// Concatenate segments in range order, rebasing each one's `name_offset`.
///
/// ⚠️ **This is the load's memory peak**: the destination is reserved at full size
/// while the segments still hold the same bytes, so a 320 MB arena transiently costs
/// ~640 MB. It falls monotonically as each segment drops at the end of its iteration.
/// Removing the peak entirely needs the mapped arena (GitHub #114), not a bigger loop.
fn merge_segments(segments: Vec<Segment>) -> (String, Vec<SearchEntry>) {
    let total_rows: usize = segments.iter().map(|s| s.entries.len()).sum();
    let total_names: usize = segments.iter().map(|s| s.names.len()).sum();

    let mut names = String::with_capacity(total_names);
    let mut entries = Vec::with_capacity(total_rows);
    for mut segment in segments {
        let base = names.len() as u32;
        names.push_str(&segment.names);
        for entry in &mut segment.entries {
            entry.name_offset += base;
        }
        entries.append(&mut segment.entries);
    }
    debug_assert!(
        entries.windows(2).all(|w| w[0].id < w[1].id),
        "the arena must stay strictly ascending by id: index_of_id binary-searches it"
    );
    (names, entries)
}

/// Split `[min_id, max_id]` into at most `workers` contiguous rowid ranges.
///
/// Ranges are by id, not by row, so a sparse id space (heavy churn deletes rows and
/// never reuses their rowids) gives uneven segments. That costs some parallelism and
/// nothing else: the merge is ordered by range, so the result is identical either way.
fn rowid_ranges(min_id: i64, max_id: i64, workers: usize) -> Vec<(i64, Option<i64>)> {
    if workers <= 1 || max_id <= min_id {
        return vec![(min_id, None)];
    }
    let span = (max_id - min_id + 1) as u128;
    let step = (span / workers as u128).max(1) as i64;
    let mut ranges = Vec::with_capacity(workers);
    let mut lo = min_id;
    for _ in 0..workers - 1 {
        let hi = lo.saturating_add(step);
        if hi > max_id {
            break;
        }
        ranges.push((lo, Some(hi)));
        lo = hi;
    }
    ranges.push((lo, None)); // the last range is open-ended, so a row past max_id can't be lost
    ranges
}

/// Load all entries from the index DB into an in-memory `SearchIndex`.
///
/// `name_folded` is NOT loaded: the search pattern is normalized instead
/// (NFD on macOS) to avoid ~5.1M extra String allocations and ~300 MB of memory.
///
/// The read is split across [`MAX_LOAD_WORKERS`] rowid ranges, each on its own
/// thread-local connection, because the cost is I/O latency rather than compute and
/// one thread cannot give the drive enough queue depth to answer faster.
pub(crate) fn load_search_index(pool: &ReadPool, cancel: &AtomicBool) -> Result<SearchIndex, String> {
    let t = std::time::Instant::now();
    let generation = index().search_generation();

    let (estimate, min_id, max_id) = pool.with_conn(|conn: &rusqlite::Connection| {
        let estimate = row_count_estimate(conn);
        // Both are O(1) on a rowid table: SQLite walks to the leftmost/rightmost leaf.
        let bounds: (Option<i64>, Option<i64>) = conn
            .query_row("SELECT MIN(id), MAX(id) FROM entries", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap_or((None, None));
        (estimate, bounds.0.unwrap_or(1), bounds.1.unwrap_or(0))
    })?;

    let workers = if estimate < PARALLEL_LOAD_THRESHOLD {
        1
    } else {
        std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(MAX_LOAD_WORKERS)
    };
    let ranges = rowid_ranges(min_id, max_id, workers);
    let reserve_per_worker = estimate.div_ceil(ranges.len().max(1));

    // `collect` on an indexed parallel iterator preserves order, which the merge
    // depends on: segment k holds a strictly lower rowid range than segment k+1, so
    // concatenating them in order keeps `entries` sorted by id.
    let segments: Vec<Segment> = if ranges.len() == 1 {
        vec![load_range(pool, ranges[0].0, ranges[0].1, estimate, cancel)?]
    } else {
        ranges
            .par_iter()
            .map(|&(lo, hi)| load_range(pool, lo, hi, reserve_per_worker, cancel))
            .collect::<Result<Vec<_>, String>>()?
    };

    #[cfg(test)]
    LAST_LOAD_WORKERS.store(ranges.len(), Ordering::Relaxed);

    let (names, entries) = merge_segments(segments);

    log::debug!(
        "Search index loaded: {}, generation {generation}, {} worker(s), took {:?}",
        pluralize_with(entries.len() as u64, "entry", "entries"),
        ranges.len(),
        t.elapsed()
    );
    Ok(SearchIndex {
        names,
        entries,
        generation,
    })
}

#[cfg(test)]
mod tests {
    use cmdr_index::ReadPool;
    use cmdr_index::store::{IndexStore, ROOT_ID};

    use super::*;

    // ── The parallel load ────────────────────────────────────────────

    /// Build an index with enough rows to cross [`PARALLEL_LOAD_THRESHOLD`], so the
    /// load really does split into ranges rather than quietly taking the single-worker
    /// path the rest of the suite exercises.
    fn store_with_rows(dir: &std::path::Path, rows: usize) -> std::path::PathBuf {
        let db_path = dir.join("parallel-index.db");
        let _store = IndexStore::open(&db_path).expect("failed to open store");
        let conn = IndexStore::open_write_connection(&db_path).unwrap();
        conn.execute_batch("BEGIN").unwrap();
        for i in 0..rows {
            // A mix of the shapes the sentinel encoding has to carry across a merge.
            let size = if i % 7 == 0 { None } else { Some(i as u64) };
            let modified = if i % 11 == 0 {
                None
            } else {
                Some(1_700_000_000 + i as u64)
            };
            IndexStore::insert_entry_v2(
                &conn,
                ROOT_ID,
                &format!("file-{i:07}.txt"),
                i % 13 == 0,
                false,
                size,
                size,
                modified,
                None,
            )
            .unwrap();
        }
        conn.execute_batch("COMMIT").unwrap();
        db_path
    }

    /// `index_of_id` is a binary search, so a load that returned rows out of order
    /// would answer wrong rather than slow. The parallel path merges per-range
    /// segments, which is exactly where the order could be lost.
    #[test]
    fn entries_are_sorted_by_id() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let db_path = store_with_rows(dir.path(), PARALLEL_LOAD_THRESHOLD + 5_000);

        let pool = ReadPool::new(db_path).unwrap();
        let index = load_search_index(&pool, &AtomicBool::new(false)).unwrap();

        assert!(
            index.entries.windows(2).all(|w| w[0].id < w[1].id),
            "the merged arena must stay strictly ascending by id"
        );
        for entry in &index.entries {
            assert_eq!(
                index.index_of_id(entry.id).map(|i| index.entries[i].id),
                Some(entry.id),
                "every row must be findable by its own id"
            );
        }
        assert_eq!(index.index_of_id(-1), None, "an absent id finds nothing");
    }

    /// The whole point of the segment merge is that it changes nothing. Every row's
    /// name, parent, kind, size, and mtime must come out of a multi-worker load
    /// exactly as they do out of a single-worker one.
    #[test]
    fn a_parallel_load_matches_a_single_worker_load_row_for_row() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let rows = PARALLEL_LOAD_THRESHOLD + 5_000;
        let db_path = store_with_rows(dir.path(), rows);
        let pool = ReadPool::new(db_path).unwrap();

        let parallel = load_search_index(&pool, &AtomicBool::new(false)).unwrap();
        // The same read, forced onto one range, is the reference.
        let reference = {
            let segment = load_range(&pool, 1, None, 0, &AtomicBool::new(false)).unwrap();
            let (names, entries) = merge_segments(vec![segment]);
            SearchIndex {
                names,
                entries,
                generation: 0,
            }
        };

        assert!(
            LAST_LOAD_WORKERS.load(Ordering::Relaxed) > 1,
            "the fixture must actually take the parallel path, or this test proves nothing"
        );
        assert!(
            parallel.entries.len() > rows,
            "the fixture should be past the threshold"
        );
        assert_eq!(parallel.entries.len(), reference.entries.len());
        for (a, b) in parallel.entries.iter().zip(reference.entries.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.parent_id, b.parent_id);
            assert_eq!(a.is_directory, b.is_directory);
            assert_eq!(a.size.get(), b.size.get(), "size, sentinel and all, on id {}", a.id);
            assert_eq!(a.modified_at.get(), b.modified_at.get(), "mtime on id {}", a.id);
            assert_eq!(
                parallel.name(a),
                reference.name(b),
                "the rebased name offset must still address the right bytes"
            );
        }
    }

    /// A rebased offset is the one thing the merge can silently corrupt: every row
    /// keeps its own `name_offset` until the segment it came from is concatenated, so
    /// an off-by-one base shifts a whole segment's names by a few bytes and search
    /// starts matching neighbours' names.
    #[test]
    fn merging_segments_rebases_every_name_offset() {
        let segments = vec![
            Segment {
                names: "alpha".to_string(),
                entries: vec![entry_named(1, 0, 5)],
            },
            Segment {
                names: "beta".to_string(),
                entries: vec![entry_named(2, 0, 4)],
            },
            Segment {
                names: "gamma".to_string(),
                entries: vec![entry_named(3, 0, 5)],
            },
        ];
        let (names, entries) = merge_segments(segments);
        let index = SearchIndex {
            names,
            entries,
            generation: 0,
        };
        let read: Vec<&str> = index.entries.iter().map(|e| index.name(e)).collect();
        assert_eq!(read, vec!["alpha", "beta", "gamma"]);
    }

    fn entry_named(id: i64, name_offset: u32, name_len: u16) -> SearchEntry {
        SearchEntry {
            id,
            parent_id: ROOT_ID,
            name_offset,
            name_len,
            is_directory: false,
            size: OptU64::NONE,
            modified_at: OptU64::NONE,
        }
    }

    /// Ranges must tile the id space with no gap and no overlap, or the merge either
    /// loses rows or double-counts them. The last one is open-ended on purpose: a row
    /// written after `MAX(id)` was read still has to land.
    #[test]
    fn rowid_ranges_tile_the_id_space() {
        let ranges = rowid_ranges(1, 1000, 4);
        assert_eq!(ranges.len(), 4);
        assert_eq!(ranges[0].0, 1);
        assert_eq!(ranges.last().unwrap().1, None, "the last range stays open-ended");
        for pair in ranges.windows(2) {
            assert_eq!(
                pair[0].1,
                Some(pair[1].0),
                "each range must start exactly where the previous one ended"
            );
        }

        assert_eq!(rowid_ranges(1, 1000, 1).len(), 1, "one worker means one range");
        assert_eq!(rowid_ranges(1, 1, 8).len(), 1, "a one-row index doesn't split");
    }

    /// A cancelled load has to return promptly rather than reading the rest of the
    /// drive into memory first. Every worker watches the same flag.
    #[test]
    fn a_cancelled_load_gives_up() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let db_path = store_with_rows(dir.path(), PARALLEL_LOAD_THRESHOLD + 5_000);
        let pool = ReadPool::new(db_path).unwrap();

        let cancelled = AtomicBool::new(true);
        let result = load_search_index(&pool, &cancelled);
        assert!(result.is_err(), "a pre-cancelled load must not build an arena");
    }

    /// The row estimate has to come off `dir_stats` rather than a full `COUNT(*)`
    /// traversal, and it has to stay right when that rollup hasn't been written.
    #[test]
    fn the_row_estimate_survives_a_missing_root_rollup() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let db_path = store_with_rows(dir.path(), 100);
        let pool = ReadPool::new(db_path).unwrap();

        // No `dir_stats` row for the scan root yet: a first scan still running.
        let estimate = pool.with_conn(row_count_estimate).unwrap();
        assert!(estimate >= 100, "the COUNT(*) fallback still answers, got {estimate}");

        // The arena is correct either way; the estimate only sizes the allocation.
        let index = load_search_index(&pool, &AtomicBool::new(false)).unwrap();
        assert_eq!(index.entries.len(), 101, "root sentinel plus the 100 rows");
    }
}
