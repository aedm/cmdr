/**
 * Compare directories (⇧F2), Total Commander's "Compare directories": mark, in
 * each pane, the files the other pane lacks plus the copies that are newer (or,
 * per mode, differ in size). Equal files and the older copy end up unmarked, so
 * F5 afterwards brings the other side up to date.
 *
 * The comparison is the backend's (`src-tauri/src/file_system/listing/compare.rs`),
 * read off both cached listings at once; this replaces each pane's selection with
 * the answer (adding the `..` offset) and says what happened in a toast.
 */

import { compareDirectories as compareDirectoriesIpc, type CompareDirectoriesMode } from '$lib/tauri-commands'
import { addToast } from '$lib/ui/toast'
import { tString } from '$lib/intl/messages.svelte'
import { formatNumber } from '$lib/file-explorer/selection/selection-info-utils'
import { getAppLogger } from '$lib/logging/logger'
import type { FilePaneAPI } from './types'

const log = getAppLogger('fileExplorer')

export interface CompareDirectoriesDeps {
  getPaneRef: (pane: 'left' | 'right') => FilePaneAPI | undefined
  getShowHiddenFiles: () => boolean
}

export async function compareDirectories(deps: CompareDirectoriesDeps, mode: CompareDirectoriesMode): Promise<void> {
  const left = deps.getPaneRef('left')
  const right = deps.getPaneRef('right')
  const leftListingId = left?.getListingId() ?? ''
  const rightListingId = right?.getListingId() ?? ''
  // A network hub or a search-results snapshot has no folder listing to compare.
  if (!left || !right || leftListingId === '' || rightListingId === '') {
    addToast(tString('fileExplorer.compareDirectories.needsTwoFolders'), { level: 'info' })
    return
  }

  const includeHidden = deps.getShowHiddenFiles()
  let result
  try {
    result = await compareDirectoriesIpc(leftListingId, includeHidden, rightListingId, includeHidden, mode)
  } catch (e) {
    const reason = (e as { type?: string }).type
    log.warn("compare directories couldn't run: {reason}", { reason: reason ?? String(e) })
    // A listing that went away mid-compare (the user navigated) has nothing to mark;
    // a comparison that ran out of time or broke tells the user.
    if (reason === 'timedOut' || reason === 'internal') {
      addToast(tString('fileExplorer.compareDirectories.couldNotFinish'), { level: 'warn' })
    }
    return
  }
  // The panes moved on, or hidden files were toggled, while we compared: these rows
  // describe a row space the panes no longer show.
  if (
    left.getListingId() !== leftListingId ||
    right.getListingId() !== rightListingId ||
    deps.getShowHiddenFiles() !== includeHidden
  )
    return

  const withParentOffset = (pane: FilePaneAPI, rows: number[]): number[] =>
    pane.hasParentEntry() ? rows.map((row) => row + 1) : rows
  left.setSelectedIndices(withParentOffset(left, result.left))
  right.setSelectedIndices(withParentOffset(right, result.right))

  const leftCount = result.left.length
  const rightCount = result.right.length
  if (leftCount === 0 && rightCount === 0) {
    addToast(tString('fileExplorer.compareDirectories.noDifferences'), { level: 'info' })
    return
  }
  addToast(
    tString('fileExplorer.compareDirectories.marked', {
      leftCount,
      leftCountText: formatNumber(leftCount),
      rightCount,
      rightCountText: formatNumber(rightCount),
    }),
    { level: 'info' },
  )
}
