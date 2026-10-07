import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { flushSync } from 'svelte'
import { createQuickFilterController } from './quick-filter-controller.svelte'
import { initListingDiffSync } from './listing-diff-sync.svelte'
import { createListingUpdateQueue } from './listing-update-queue'
import { createSelectionState } from './selection-state.svelte'
import { createRenameState } from '../rename/rename-state.svelte'
import type { DirectoryDiff } from '../types'

const ipc = vi.hoisted(() => ({
  setListingNameFilter: vi.fn(),
  getTotalCount: vi.fn(),
  onDirectoryDiff: vi.fn(),
}))
vi.mock('$lib/tauri-commands', () => ({
  ...ipc,
  onDirectoryDeleted: () => Promise.resolve(() => {}),
  onListingRespelled: () => Promise.resolve(() => {}),
  onWriteSourceItemDone: () => Promise.resolve(() => {}),
}))

const settle = () => new Promise((resolve) => setTimeout(resolve, 0))
const filtered = { accepted: true, totalCount: 2, newCursorIndex: 1, newSelectedIndices: [1], sequence: 5 }
const added: DirectoryDiff = {
  listingId: 'listing',
  sequence: 6,
  changes: [
    {
      type: 'add',
      index: 0,
      entry: {
        name: 'a.pdf',
        path: '/a.pdf',
        isDirectory: false,
        isSymlink: false,
        permissions: 0o644,
        owner: 'user',
        group: 'staff',
        iconId: 'file',
        extendedMetadataLoaded: true,
      },
    },
  ],
}

describe('filter and watcher row-space ordering', () => {
  let dispose: () => void
  let emit: (diff: DirectoryDiff) => void
  beforeEach(() => {
    vi.clearAllMocks()
    ipc.onDirectoryDiff.mockImplementation((callback: typeof emit) => {
      emit = callback
      return Promise.resolve(() => {})
    })
    ipc.getTotalCount.mockResolvedValue(3)
  })
  afterEach(() => {
    dispose()
  })

  function wire() {
    let cursor = 4
    let count = 8
    let sequence = 0
    let generation = 0
    const runListingUpdate = createListingUpdateQueue()
    const selection = createSelectionState()
    selection.setSelectedIndices([4])
    const ctl = createQuickFilterController({
      runListingUpdate,
      getListingId: () => 'listing',
      getLoading: () => false,
      getHasBackendListing: () => true,
      getIncludeHidden: () => false,
      getHasParent: () => true,
      getCursorFilename: () => 'z.pdf',
      getSelectedIndices: () => selection.getSelectedIndices(),
      apply: (result) => {
        count = result.totalCount
        cursor = result.cursorIndex
        selection.setSelectedIndices(result.selectedIndices)
        sequence = Math.max(sequence, result.sequence ?? 0)
      },
    })
    dispose = $effect.root(() => {
      initListingDiffSync({
        runListingUpdate,
        selection,
        rename: createRenameState(),
        renameFlow: { pendingCursorName: null },
        getListingId: () => 'listing',
        getIncludeHidden: () => false,
        getHasParent: () => true,
        getCursorIndex: () => cursor,
        setCursorIndex: (index) => {
          cursor = index
          return Promise.resolve()
        },
        applyCursorIndex: (index) => {
          cursor = index
        },
        getCurrentPath: () => '/',
        getVolumePath: () => '/',
        getOperationSelectedNames: () => null,
        getLastSequence: () => sequence,
        setLastSequence: (value) => {
          sequence = value
        },
        getDiffGeneration: () => generation,
        bumpDiffGeneration: () => ++generation,
        setTotalCount: (value) => {
          count = value
        },
        bumpSoftRefreshTick: vi.fn(),
        scheduleColumnWidthRefetch: vi.fn(),
        fetchEntryUnderCursor: vi.fn(),
        fetchListingStats: vi.fn(),
        navigateToFallback: vi.fn(),
        adoptStoredPath: vi.fn(),
        bumpCacheGeneration: vi.fn(),
      })
    })
    flushSync()
    return { ctl, state: () => ({ cursor, count, selection: selection.getSelectedIndices(), sequence }) }
  }

  it('applies a new-row diff only after the filter response establishes those rows', async () => {
    const answer = Promise.withResolvers<typeof filtered>()
    ipc.setListingNameFilter.mockReturnValue(answer.promise)
    const pane = wire()
    pane.ctl.append('p')
    await settle()
    emit(added)
    await settle()
    expect(ipc.getTotalCount).not.toHaveBeenCalled()
    answer.resolve(filtered)
    await settle()
    expect(pane.state()).toEqual({ cursor: 3, count: 3, selection: [3], sequence: 6 })
  })

  it('finishes an already-running old-row diff before taking the filter selection snapshot', async () => {
    const count = Promise.withResolvers<number>()
    ipc.getTotalCount.mockReturnValueOnce(count.promise)
    ipc.setListingNameFilter.mockResolvedValue(filtered)
    const pane = wire()
    emit({ ...added, sequence: 4 })
    await settle()
    pane.ctl.append('p')
    await settle()
    expect(ipc.setListingNameFilter).not.toHaveBeenCalled()
    count.resolve(9)
    await settle()
    expect(ipc.setListingNameFilter.mock.calls[0][4]).toEqual([4])
    expect(pane.state()).toEqual({ cursor: 2, count: 2, selection: [2], sequence: 5 })
  })

  it('discards an old-row diff delivered while the filter response is pending', async () => {
    const answer = Promise.withResolvers<typeof filtered>()
    ipc.setListingNameFilter.mockReturnValue(answer.promise)
    const pane = wire()
    pane.ctl.append('p')
    await settle()
    emit({ ...added, sequence: 4 })
    answer.resolve(filtered)
    await settle()
    expect(ipc.getTotalCount).not.toHaveBeenCalled()
    expect(pane.state()).toEqual({ cursor: 2, count: 2, selection: [2], sequence: 5 })
  })

  it('does not restore the old count when a throttled refresh runs after the filter', async () => {
    vi.useFakeTimers()
    try {
      ipc.setListingNameFilter.mockResolvedValue(filtered)
      const pane = wire()
      emit({ ...added, sequence: 3 })
      await vi.advanceTimersByTimeAsync(0)
      emit({ ...added, sequence: 4 })
      await vi.advanceTimersByTimeAsync(0)
      pane.ctl.append('p')
      await vi.advanceTimersByTimeAsync(0)
      await vi.advanceTimersByTimeAsync(250)
      expect(pane.state()).toEqual({ cursor: 2, count: 2, selection: [2], sequence: 5 })
    } finally {
      vi.useRealTimers()
    }
  })

  it('keeps processing watcher updates after a rejected filter IPC', async () => {
    const answer = Promise.withResolvers<typeof filtered>()
    ipc.setListingNameFilter.mockReturnValue(answer.promise)
    const pane = wire()
    pane.ctl.append('p')
    emit(added)
    answer.reject({ type: 'ListingGone' })
    await settle()
    expect(ipc.getTotalCount).toHaveBeenCalledOnce()
    expect(pane.state()).toEqual({ cursor: 5, count: 3, selection: [5], sequence: 6 })
  })
})
