/**
 * Tests for `compare-directories.ts` (⇧F2). They pin:
 * - both panes' listings and the hidden-files setting go to the backend,
 * - each pane's selection becomes the backend's rows plus its own `..` offset,
 * - the toasts: what was selected, no differences, a pane with no folder,
 * - an answer for panes that moved on meanwhile is dropped.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'

const { ipc, addToast } = vi.hoisted(() => ({
  ipc: { compareDirectories: vi.fn() },
  addToast: vi.fn(),
}))

vi.mock('$lib/tauri-commands', () => ({ compareDirectories: ipc.compareDirectories }))
vi.mock('$lib/ui/toast', () => ({ addToast }))
vi.mock('$lib/intl/messages.svelte', () => ({
  tString: (key: string, args?: Record<string, unknown>) => (args ? `${key} ${JSON.stringify(args)}` : key),
}))
vi.mock('$lib/logging/logger', () => ({
  getAppLogger: () => ({ warn: vi.fn(), info: vi.fn(), error: vi.fn(), debug: vi.fn() }),
}))

import { compareDirectories } from './compare-directories'
import type { FilePaneAPI } from './types'

function paneRef(listingId: string, hasParent: boolean) {
  const ref = {
    listingId,
    getListingId: vi.fn(() => ref.listingId),
    hasParentEntry: vi.fn(() => hasParent),
    setSelectedIndices: vi.fn(),
  }
  return ref
}

function deps(left: ReturnType<typeof paneRef> | undefined, right: ReturnType<typeof paneRef> | undefined) {
  return {
    getPaneRef: (pane: 'left' | 'right') => (pane === 'left' ? left : right) as unknown as FilePaneAPI | undefined,
    getShowHiddenFiles: () => true,
  }
}

describe('compareDirectories', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('selects the backend rows in each pane, with each pane’s own parent offset', async () => {
    ipc.compareDirectories.mockResolvedValue({ left: [0, 2], right: [1] })
    const left = paneRef('L', true)
    const right = paneRef('R', false)

    await compareDirectories(deps(left, right), 'newerAndMissing')

    expect(ipc.compareDirectories).toHaveBeenCalledWith('L', true, 'R', true, 'newerAndMissing')
    expect(left.setSelectedIndices).toHaveBeenCalledWith([1, 3])
    expect(right.setSelectedIndices).toHaveBeenCalledWith([1])
    expect(addToast.mock.calls[0][0]).toContain('fileExplorer.compareDirectories.marked')
    expect(addToast.mock.calls[0][0]).toContain('"leftCount":2')
  })

  it('clears both selections and says so when nothing differs', async () => {
    ipc.compareDirectories.mockResolvedValue({ left: [], right: [] })
    const left = paneRef('L', true)
    const right = paneRef('R', true)

    await compareDirectories(deps(left, right), 'missing')

    expect(left.setSelectedIndices).toHaveBeenCalledWith([])
    expect(right.setSelectedIndices).toHaveBeenCalledWith([])
    expect(addToast).toHaveBeenCalledWith('fileExplorer.compareDirectories.noDifferences', { level: 'info' })
  })

  it('asks for two folders when a pane shows no listing', async () => {
    await compareDirectories(deps(paneRef('L', false), paneRef('', false)), 'newerAndMissing')

    expect(ipc.compareDirectories).not.toHaveBeenCalled()
    expect(addToast).toHaveBeenCalledWith('fileExplorer.compareDirectories.needsTwoFolders', { level: 'info' })
  })

  it('drops the answer when a pane moved to another folder meanwhile', async () => {
    const left = paneRef('L', false)
    const right = paneRef('R', false)
    ipc.compareDirectories.mockImplementation(() => {
      left.listingId = 'L2'
      return Promise.resolve({ left: [0], right: [0] })
    })

    await compareDirectories(deps(left, right), 'newerAndMissing')

    expect(left.setSelectedIndices).not.toHaveBeenCalled()
    expect(right.setSelectedIndices).not.toHaveBeenCalled()
  })

  it('says so when the comparison ran out of time', async () => {
    ipc.compareDirectories.mockRejectedValue({ type: 'timedOut' })

    await compareDirectories(deps(paneRef('L', false), paneRef('R', false)), 'newerAndMissing')

    expect(addToast).toHaveBeenCalledWith('fileExplorer.compareDirectories.couldNotFinish', { level: 'warn' })
  })

  it('drops the answer when hidden files were toggled meanwhile', async () => {
    let showHidden = true
    const left = paneRef('L', false)
    const right = paneRef('R', false)
    ipc.compareDirectories.mockImplementation(() => {
      showHidden = false
      return Promise.resolve({ left: [0], right: [] })
    })

    await compareDirectories(
      {
        getPaneRef: (pane) => (pane === 'left' ? left : right) as unknown as FilePaneAPI,
        getShowHiddenFiles: () => showHidden,
      },
      'newerAndMissing',
    )

    expect(left.setSelectedIndices).not.toHaveBeenCalled()
  })

  it('stays quiet and selects nothing when a listing is gone', async () => {
    ipc.compareDirectories.mockRejectedValue({ type: 'gone' })
    const left = paneRef('L', false)

    await compareDirectories(deps(left, paneRef('R', false)), 'newerAndMissing')

    expect(left.setSelectedIndices).not.toHaveBeenCalled()
    expect(addToast).not.toHaveBeenCalled()
  })
})
