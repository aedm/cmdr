import { describe, expect, it, vi } from 'vitest'
import { createIndexEventHandler } from './index-events'
import type { ListingIndexSizesChanged } from '$lib/tauri-commands'
import type { FilePaneAPI } from './types'

 
describe('createIndexEventHandler', () => {
  function panes() {
    const left = { getListingId: () => 'listing-left', applyIndexSizes: vi.fn() }
    const right = { getListingId: () => 'listing-right', applyIndexSizes: vi.fn() }
    const handler = createIndexEventHandler({
      getPaneRef: (pane) => (pane === 'left' ? left : right) as unknown as FilePaneAPI,
    })
    return { left, right, handler }
  }

  function change(listingId: string): ListingIndexSizesChanged {
    return { listingId, full: false, folders: [], currentDirChanged: true, currentDir: null }
  }

  it('hands the change to the pane showing the listing, and only that one', () => {
    const { left, right, handler } = panes()
    handler(change('listing-right'))
    expect(right.applyIndexSizes).toHaveBeenCalledWith(change('listing-right'))
    expect(left.applyIndexSizes).not.toHaveBeenCalled()
  })

  it('ignores a listing no pane shows', () => {
    const { left, right, handler } = panes()
    handler(change('listing-gone'))
    expect(left.applyIndexSizes).not.toHaveBeenCalled()
    expect(right.applyIndexSizes).not.toHaveBeenCalled()
  })

  it('applies every change: the backend already paced them', () => {
    const { left, handler } = panes()
    handler(change('listing-left'))
    handler(change('listing-left'))
    expect(left.applyIndexSizes).toHaveBeenCalledTimes(2)
  })
})
