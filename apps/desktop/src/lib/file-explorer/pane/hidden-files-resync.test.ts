/**
 * Tests for `hidden-files-resync.ts`, keeping a pane consistent after the
 * hidden-files toggle changes how many rows the listing has. They pin:
 * - the backend hears the new setting before anything is read back,
 * - the new total is published before any cursor math runs,
 * - the cursor follows the file it was on, with the `..` row offset applied,
 * - a cursor left past the end is clamped, and only then,
 * - a file that just became hidden falls back to the clamp,
 * - an empty listing puts the cursor at 0 rather than -1,
 * - a resync the pane has moved on from (a new listing, or a newer toggle) ends
 *   quietly and writes nothing, while a failure on the live listing still rejects.
 */
import { describe, it, expect, vi, beforeEach, type Mock } from 'vitest'

const { ipc } = vi.hoisted<{
  ipc: { getTotalCount: Mock; findFileIndex: Mock; setListingIncludeHidden: Mock }
}>(() => ({
  ipc: { getTotalCount: vi.fn(), findFileIndex: vi.fn(), setListingIncludeHidden: vi.fn() },
}))

vi.mock('$lib/tauri-commands', () => ({
  getTotalCount: ipc.getTotalCount,
  findFileIndex: ipc.findFileIndex,
  setListingIncludeHidden: ipc.setListingIncludeHidden,
}))

import { createHiddenFilesResync } from './hidden-files-resync'

describe('resyncAfterHiddenFilesToggle', () => {
  let setTotalCount: Mock
  let setCursorIndex: Mock
  /** The listing the pane shows right now; a test moves the pane on by changing it. */
  let paneListingId: string
  let resyncAfterHiddenFilesToggle: ReturnType<typeof createHiddenFilesResync>

  beforeEach(() => {
    vi.clearAllMocks()
    setTotalCount = vi.fn()
    setCursorIndex = vi.fn().mockResolvedValue(undefined)
    ipc.getTotalCount.mockResolvedValue(10)
    ipc.findFileIndex.mockResolvedValue(null)
    ipc.setListingIncludeHidden.mockResolvedValue(undefined)
    paneListingId = 'listing-1'
    resyncAfterHiddenFilesToggle = createHiddenFilesResync(() => paneListingId)
  })

  function run(over: Partial<Parameters<ReturnType<typeof createHiddenFilesResync>>[0]> = {}) {
    return resyncAfterHiddenFilesToggle({
      listingId: 'listing-1',
      includeHidden: true,
      nameToFollow: undefined,
      cursorIndex: 0,
      getHasParent: () => false,
      setTotalCount,
      setCursorIndex,
      ...over,
    })
  }

  // The backend numbers `directory-diff` rows in the pane's row space and skips
  // rows the pane can't see, so it has to know the setting before the pane
  // re-reads anything in the new space.
  it('tells the backend the new setting before reading the count', async () => {
    const calls: string[] = []
    ipc.setListingIncludeHidden.mockImplementation(() => {
      calls.push('set')
      return Promise.resolve()
    })
    ipc.getTotalCount.mockImplementation(() => {
      calls.push('count')
      return Promise.resolve(10)
    })
    await run({ includeHidden: false })
    expect(ipc.setListingIncludeHidden).toHaveBeenCalledWith('listing-1', false)
    expect(calls).toEqual(['set', 'count'])
  })

  it('publishes the new total count', async () => {
    await run()
    expect(ipc.getTotalCount).toHaveBeenCalledWith('listing-1', true)
    expect(setTotalCount).toHaveBeenCalledWith(10)
  })

  it('keeps the cursor on the same file', async () => {
    ipc.findFileIndex.mockResolvedValue(4)
    await run({ nameToFollow: 'a.txt', cursorIndex: 7 })
    expect(ipc.findFileIndex).toHaveBeenCalledWith('listing-1', 'a.txt', true)
    expect(setCursorIndex).toHaveBeenCalledWith(4)
  })

  it('offsets the found index by the `..` row', async () => {
    ipc.findFileIndex.mockResolvedValue(4)
    await run({ nameToFollow: 'a.txt', getHasParent: () => true })
    expect(setCursorIndex).toHaveBeenCalledWith(5)
  })

  it('leaves a still-valid cursor alone when there is no file to follow', async () => {
    await run({ cursorIndex: 3 })
    expect(setCursorIndex).not.toHaveBeenCalled()
  })

  it('clamps a cursor left past the end', async () => {
    ipc.getTotalCount.mockResolvedValue(3)
    await run({ cursorIndex: 8 })
    expect(setCursorIndex).toHaveBeenCalledWith(2)
  })

  it('counts the `..` row when deciding whether the cursor still fits', async () => {
    ipc.getTotalCount.mockResolvedValue(3)
    await run({ cursorIndex: 3, getHasParent: () => true })
    expect(setCursorIndex).not.toHaveBeenCalled()
  })

  it('clamps when the followed file just became hidden', async () => {
    ipc.getTotalCount.mockResolvedValue(2)
    ipc.findFileIndex.mockResolvedValue(null)
    await run({ nameToFollow: 'hidden.txt', cursorIndex: 5 })
    expect(setCursorIndex).toHaveBeenCalledWith(1)
  })

  it('puts the cursor at 0 on an emptied listing', async () => {
    ipc.getTotalCount.mockResolvedValue(0)
    await run({ cursorIndex: 4 })
    expect(setCursorIndex).toHaveBeenCalledWith(0)
  })

  // A navigation ends the old listing in the same tick it clears the pane's id
  // (`listing-loader.ts`), so a read still in flight is answered "Listing not
  // found". The caller is a fire-and-forget `void`, so that rejection used to
  // escape the window as an unhandled one (MCP `select_volume` then `nav_to_path`).
  it('ends quietly when the pane moved to another listing and the read found the old one gone', async () => {
    ipc.getTotalCount.mockImplementation(() => {
      paneListingId = 'listing-2'
      return Promise.reject(new Error('Listing not found: listing-1'))
    })
    await expect(run({ nameToFollow: 'a.txt' })).resolves.toBeUndefined()
    expect(setTotalCount).not.toHaveBeenCalled()
    expect(setCursorIndex).not.toHaveBeenCalled()
  })

  // The old listing can also answer before it's torn down. Its count and cursor
  // describe a listing the pane no longer shows.
  it('writes nothing into a pane that moved on while the count was being read', async () => {
    ipc.getTotalCount.mockImplementation(() => {
      paneListingId = 'listing-2'
      return Promise.resolve(3)
    })
    await run({ cursorIndex: 8 })
    expect(setTotalCount).not.toHaveBeenCalled()
    expect(setCursorIndex).not.toHaveBeenCalled()
  })

  it('leaves the cursor alone when the pane moved on while the followed file was being looked up', async () => {
    ipc.findFileIndex.mockImplementation(() => {
      paneListingId = ''
      return Promise.resolve(4)
    })
    await run({ nameToFollow: 'a.txt' })
    expect(setCursorIndex).not.toHaveBeenCalled()
  })

  it('gives way to a newer toggle on the same listing', async () => {
    let answerFirstCount: (count: number) => void = () => {}
    ipc.getTotalCount.mockImplementationOnce(
      () =>
        new Promise<number>((resolve) => {
          answerFirstCount = resolve
        }),
    )
    const first = run({ includeHidden: true })
    await vi.waitFor(() => {
      expect(ipc.getTotalCount).toHaveBeenCalledTimes(1)
    })
    await run({ includeHidden: false })
    expect(setTotalCount).toHaveBeenCalledExactlyOnceWith(10)

    answerFirstCount(99)
    await first
    expect(setTotalCount).toHaveBeenCalledExactlyOnceWith(10)
  })

  it('still rejects when a read fails on the listing the pane is showing', async () => {
    ipc.getTotalCount.mockRejectedValue(new Error('Failed to acquire cache lock'))
    await expect(run()).rejects.toThrow('Failed to acquire cache lock')
  })
})
