/**
 * Pins the pane's loading grace period: a navigation that settles inside 100 ms
 * keeps the previous directory visible, while a genuinely slow one earns the
 * loading screen.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { flushSync } from 'svelte'

import { createListingPresentation, LISTING_LOADING_DELAY_MS } from './listing-presentation.svelte'

describe('createListingPresentation', () => {
  let dispose: (() => void) | undefined

  function create(initial: { listingId: string; totalCount: number; loading: boolean }) {
    let listingId = $state(initial.listingId)
    let totalCount = $state(initial.totalCount)
    let loading = $state(initial.loading)
    let presentation!: ReturnType<typeof createListingPresentation>

    dispose = $effect.root(() => {
      presentation = createListingPresentation({
        getListingId: () => listingId,
        getTotalCount: () => totalCount,
        getLoading: () => loading,
      })
    })
    flushSync()

    return {
      presentation,
      set: (next: Partial<typeof initial>) => {
        if (next.listingId !== undefined) listingId = next.listingId
        if (next.totalCount !== undefined) totalCount = next.totalCount
        if (next.loading !== undefined) loading = next.loading
        flushSync()
      },
    }
  }

  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    dispose?.()
    dispose = undefined
    vi.useRealTimers()
  })

  it('shows loading immediately when the pane has no settled listing yet', () => {
    const { presentation } = create({ listingId: '', totalCount: 0, loading: true })

    expect(presentation.showLoading).toBe(true)
  })

  it('keeps the settled listing visible when the next load finishes inside the grace period', () => {
    const { presentation, set } = create({ listingId: 'old', totalCount: 12, loading: false })

    set({ listingId: 'new', totalCount: 0, loading: true })
    expect(presentation.showLoading).toBe(false)
    expect(presentation.listingId).toBe('old')
    expect(presentation.totalCount).toBe(12)

    vi.advanceTimersByTime(LISTING_LOADING_DELAY_MS - 1)
    set({ totalCount: 7, loading: false })

    expect(presentation.showLoading).toBe(false)
    expect(presentation.listingId).toBe('new')
    expect(presentation.totalCount).toBe(7)
  })

  it('reveals loading after 100 ms and cancels the timer when the load settles', () => {
    const { presentation, set } = create({ listingId: 'old', totalCount: 12, loading: false })

    set({ listingId: 'new', totalCount: 0, loading: true })
    vi.advanceTimersByTime(LISTING_LOADING_DELAY_MS)
    flushSync()
    expect(presentation.showLoading).toBe(true)

    set({ totalCount: 7, loading: false })
    expect(presentation.showLoading).toBe(false)
  })
})
