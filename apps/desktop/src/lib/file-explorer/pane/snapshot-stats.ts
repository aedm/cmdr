/**
 * The footer figures for a search-results pane, folded out of the in-memory
 * snapshot.
 *
 * A normal pane asks the backend for `ListingStats`, but a snapshot pane has no
 * backend listing to ask about: its rows are a frontend-only array
 * (`lib/search/snapshot-store.svelte.ts`). The fold below produces the same
 * shape from those rows, so `SelectionInfo`, the context-menu header, and every
 * other `stats` reader work on a snapshot pane exactly as they do in a folder.
 *
 * It runs on up to `SNAPSHOT_ENTRIES_CAP` (10,000) rows per selection change,
 * which measures in the tens of microseconds — nothing worth caching.
 *
 * Two honesty notes:
 * - A `SearchResultEntry` carries no PHYSICAL size, so the on-disk totals mirror
 *   the logical ones. Leaving them at 0 would make the "on disk" size setting
 *   render `0 bytes` for a real selection, which is a worse lie than being a few
 *   blocks off.
 * - A folder row has no recursive size either, so it counts toward the row
 *   counts and contributes 0 bytes. A search hit is almost always a file, and a
 *   folder's subtree isn't part of what the user selected here anyway.
 */

import type { SearchResultEntry } from '$lib/ipc/bindings'
import type { ListingStats } from '../types'

/**
 * Folds `entries` (and the rows `selectedIndices` names) into the stats shape the
 * pane footer reads. An index past the end is ignored: a delete-sync can shrink the
 * snapshot a frame before the selection catches up.
 *
 * The four `selected*` fields are `null` for an empty selection, matching what the
 * backend returns when a stats call names no indices.
 */
export function computeSnapshotStats(
  entries: readonly SearchResultEntry[],
  selectedIndices: readonly number[],
): ListingStats {
  let totalFiles = 0
  let totalDirs = 0
  let totalSize = 0
  for (const entry of entries) {
    if (entry.isDirectory) totalDirs += 1
    else totalFiles += 1
    totalSize += entry.size ?? 0
  }

  if (selectedIndices.length === 0) {
    return {
      totalFiles,
      totalDirs,
      totalSize,
      totalPhysicalSize: totalSize,
      selectedFiles: null,
      selectedDirs: null,
      selectedSize: null,
      selectedPhysicalSize: null,
    }
  }

  let selectedFiles = 0
  let selectedDirs = 0
  let selectedSize = 0
  for (const index of selectedIndices) {
    const entry = entries[index] as SearchResultEntry | undefined
    if (!entry) continue
    if (entry.isDirectory) selectedDirs += 1
    else selectedFiles += 1
    selectedSize += entry.size ?? 0
  }

  return {
    totalFiles,
    totalDirs,
    totalSize,
    totalPhysicalSize: totalSize,
    selectedFiles,
    selectedDirs,
    selectedSize,
    selectedPhysicalSize: selectedSize,
  }
}
