/**
 * Asking the index for EVERY row a search found, so a pane shows all of them.
 *
 * The dialog runs with `limit: 30`, which is what its list can usefully render, and a
 * snapshot pane holds up to [`SNAPSHOT_ENTRIES_CAP`]. So the rows the dialog is holding
 * are the wrong set to promote: a user who searched 112 JPGs to delete them got 30.
 * Both paths that fill a pane come through here, with the same query the run used and
 * the ceiling raised:
 *
 * - **"Open in pane"** (`snapshot-promotion.ts`), before the snapshot is built, so the
 *   pane opens complete.
 * - **A handed-off walk ending** (`walk-handoff.svelte.ts`). A walk streams at the
 *   dialog's limit and can't be widened mid-flight, but everything it walks lands in
 *   the index as it goes, so once it ends the index can answer for the whole scope.
 *
 * **It never shrinks a pane.** An answer smaller than what the pane already holds is
 * dropped rather than applied: an index that hasn't caught up with a just-finished walk
 * would otherwise take rows off the screen that the user can see and act on.
 *
 * **It never blocks the open.** Every failure here leaves the caller with the rows it
 * already had, which is exactly today's behavior; the alternative is a dialog that
 * refuses to open a pane because a second query failed.
 */

import { searchFiles, type SearchQuery, type SearchResultEntry } from '$lib/tauri-commands'
import { getAppLogger } from '$lib/logging/logger'
import { replaceSnapshotEntries, getSnapshot, SNAPSHOT_ENTRIES_CAP } from './snapshot-store.svelte'
import { resortSnapshotIfSorted } from './snapshot-sort.svelte'

const log = getAppLogger('search')

/** One full answer: the rows, the count behind them, and the volume they live on. */
export interface FullResultSet {
  entries: SearchResultEntry[]
  totalCount: number
  /** `SearchResult.targetVolumeId`, or `null` when the answer didn't name one. */
  volumeId: string | null
}

/** The query a run used, asked again for as many rows as a pane can hold. */
export function fullRowQuery(query: SearchQuery): SearchQuery {
  return { ...query, limit: SNAPSHOT_ENTRIES_CAP }
}

/**
 * Every row the index has for `query`, or `null` when the ask failed. `query` is taken
 * as it stands: pass it through [`fullRowQuery`] first.
 */
export async function fetchAllRows(query: SearchQuery): Promise<FullResultSet | null> {
  try {
    const result = await searchFiles(query)
    return { entries: result.entries, totalCount: result.totalCount, volumeId: result.targetVolumeId ?? null }
  } catch (err) {
    log.warn('Filling a search-results pane from the index failed: {error}', { error: err })
    return null
  }
}

/**
 * Tops a snapshot up from the index, and answers whether the pane grew.
 *
 * For the pane whose walk just ended: the rows it streamed stopped at the dialog's
 * limit, and the index now holds what the walk wrote. A snapshot that has since gone
 * away, an ask that failed, and an answer no bigger than what's on screen are all
 * no-ops.
 */
export async function fillSnapshotFromIndex(snapshotId: string, query: SearchQuery): Promise<boolean> {
  const before = getSnapshot(snapshotId)
  if (!before) return false
  const full = await fetchAllRows(fullRowQuery(query))
  if (!full) return false
  const current = getSnapshot(snapshotId)
  if (!current || full.entries.length <= current.entries.length) return false
  if (!replaceSnapshotEntries(snapshotId, full.entries, full.totalCount)) return false
  // A sorted pane holds its rows until the new set has a place in that order; an
  // unsorted one already shows them (`snapshot-store::replaceSnapshotEntries`).
  await resortSnapshotIfSorted(snapshotId)
  return true
}
