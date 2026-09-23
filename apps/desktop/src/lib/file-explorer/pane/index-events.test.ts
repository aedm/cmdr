import { describe, expect, it, vi } from 'vitest'
import { throttledRefresh, createIndexEventHandler } from './index-events'
import type { FilePaneAPI } from './types'

/* eslint-disable @typescript-eslint/unbound-method -- vi.fn() mocks have no this binding */
describe('throttledRefresh', () => {
  it('fires immediately when not throttled', () => {
    const paneRef = { refreshIndexSizes: vi.fn() } as unknown as FilePaneAPI
    const setThrottle = vi.fn()
    throttledRefresh(true, 0, setThrottle, paneRef, 2000)
    expect(paneRef.refreshIndexSizes).toHaveBeenCalled()
    expect(setThrottle).toHaveBeenCalled()
  })

  it('skips when shouldRefresh is false', () => {
    const paneRef = { refreshIndexSizes: vi.fn() } as unknown as FilePaneAPI
    throttledRefresh(false, 0, vi.fn(), paneRef, 2000)
    expect(paneRef.refreshIndexSizes).not.toHaveBeenCalled()
  })

  it('skips when within cooldown period', () => {
    const paneRef = { refreshIndexSizes: vi.fn() } as unknown as FilePaneAPI
    const futureTime = Date.now() + 10_000
    throttledRefresh(true, futureTime, vi.fn(), paneRef, 2000)
    expect(paneRef.refreshIndexSizes).not.toHaveBeenCalled()
  })

  it('handles undefined paneRef gracefully', () => {
    expect(() => {
      throttledRefresh(true, 0, vi.fn(), undefined, 2000)
    }).not.toThrow()
  })
})

describe('createIndexEventHandler', () => {
  function panes() {
    const left = { getListingId: () => 'listing-left', refreshIndexSizes: vi.fn() }
    const right = { getListingId: () => 'listing-right', refreshIndexSizes: vi.fn() }
    const handler = createIndexEventHandler({
      getPaneRef: (pane) => (pane === 'left' ? left : right) as unknown as FilePaneAPI,
    })
    return { left, right, handler }
  }

  it('refreshes only the pane showing the listing', () => {
    const { left, right, handler } = panes()
    handler('listing-right')
    expect(right.refreshIndexSizes).toHaveBeenCalledTimes(1)
    expect(left.refreshIndexSizes).not.toHaveBeenCalled()
  })

  it('ignores a listing no pane shows', () => {
    const { left, right, handler } = panes()
    handler('listing-gone')
    expect(left.refreshIndexSizes).not.toHaveBeenCalled()
    expect(right.refreshIndexSizes).not.toHaveBeenCalled()
  })

  it('respects the throttle between calls', () => {
    const { left, handler } = panes()
    handler('listing-left')
    handler('listing-left')
    expect(left.refreshIndexSizes).toHaveBeenCalledTimes(1)
  })
})
