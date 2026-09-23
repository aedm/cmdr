import type { FilePaneAPI } from './types'

/** Throttled refresh: fires immediately on first relevant event, then skips for the cooldown period. */
export function throttledRefresh(
  shouldRefresh: boolean,
  throttleUntil: number,
  setThrottle: (v: number) => void,
  paneRef: FilePaneAPI | undefined,
  cooldownMs: number,
) {
  if (!shouldRefresh) return
  const now = Date.now()
  if (now < throttleUntil) return
  setThrottle(now + cooldownMs)
  paneRef?.refreshIndexSizes()
}

/**
 * Creates the handler for `listing-index-sizes-changed`: the backend already worked out which open
 * listings an index update touched (`src-tauri/src/listing_index_sizes/`), so this only finds the pane
 * showing that listing and refreshes it, at most once per cooldown.
 */
export function createIndexEventHandler(deps: { getPaneRef: (pane: 'left' | 'right') => FilePaneAPI | undefined }) {
  const cooldownMs = 2000
  let leftThrottleUntil = 0
  let rightThrottleUntil = 0

  return function handleListingIndexSizesChanged(listingId: string) {
    const left = deps.getPaneRef('left')
    const right = deps.getPaneRef('right')
    throttledRefresh(
      left?.getListingId() === listingId,
      leftThrottleUntil,
      (v) => (leftThrottleUntil = v),
      left,
      cooldownMs,
    )
    throttledRefresh(
      right?.getListingId() === listingId,
      rightThrottleUntil,
      (v) => (rightThrottleUntil = v),
      right,
      cooldownMs,
    )
  }
}
