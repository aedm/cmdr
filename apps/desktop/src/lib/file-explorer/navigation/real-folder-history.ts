/**
 * Finding the newest REAL folder a pane visited, skipping the `search-results://<id>`
 * snapshot paths in its history.
 *
 * A snapshot path names an in-memory, per-session result set rather than a folder, so
 * anything that needs a folder has to look past it: the Search dialog's "current folder"
 * scope can't search inside one (`$lib/search/searchable-folder.ts`), and tab persistence
 * must not write one to disk, since the id names nothing once the session ends
 * (`../pane/tab-operations.ts`).
 */
import { snapshotIdFromPanePath } from '$lib/search/snapshot-store.svelte'

/** Whether `path` belongs to a search-results snapshot pane rather than a folder. */
export function isSnapshotPath(path: string): boolean {
  return snapshotIdFromPanePath(path) !== null
}

/**
 * The newest entry naming a real folder in an oldest-first history stack, `null` when
 * every entry is a snapshot.
 *
 * Generic over the entry, with `pathOf` reading the path out, because the two callers
 * hold different shapes: a bare path list for the search scope, and whole `HistoryEntry`s
 * for persistence, which needs the folder's own `volumeId` too (a snapshot pane's is the
 * virtual `search-results`, which says nothing about where the rows live).
 */
export function latestRealFolder<T>(stack: readonly T[], pathOf: (entry: T) => string): T | null {
  for (let i = stack.length - 1; i >= 0; i--) {
    const entry = stack[i]
    if (!isSnapshotPath(pathOf(entry))) return entry
  }
  return null
}
