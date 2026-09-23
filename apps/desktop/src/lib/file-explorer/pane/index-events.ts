import type { ListingIndexSizesChanged } from '$lib/tauri-commands'
import type { FilePaneAPI } from './types'

/**
 * Creates the handler for `listing-index-sizes-changed`. The backend already decided everything:
 * which open listing an index update touched, whether any row's shown values moved, and how often
 * (`src-tauri/src/listing_index_sizes/`: at most one per listing per 2 s, none while the window is
 * hidden). So this only hands the change to the pane showing that listing.
 */
export function createIndexEventHandler(deps: { getPaneRef: (pane: 'left' | 'right') => FilePaneAPI | undefined }) {
  return function handleListingIndexSizesChanged(change: ListingIndexSizesChanged) {
    for (const pane of ['left', 'right'] as const) {
      const paneRef = deps.getPaneRef(pane)
      if (paneRef?.getListingId() === change.listingId) paneRef.applyIndexSizes(change)
    }
  }
}
