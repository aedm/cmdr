/**
 * Keeps a pane's last settled listing on screen during a short navigation.
 * Fast directory loads never replace useful rows with a transient loading frame;
 * a load that lasts 100 ms earns the normal loading screen.
 */

export const LISTING_LOADING_DELAY_MS = 100

export interface ListingPresentationDeps {
  getListingId: () => string
  getTotalCount: () => number
  getLoading: () => boolean
}

export interface ListingPresentation {
  /** Listing id the list view should render while a load is in flight. */
  readonly listingId: string
  /** Row count paired with `listingId`. */
  readonly totalCount: number
  /** Whether the loading screen has outlasted the grace period. */
  readonly showLoading: boolean
}

export function createListingPresentation(deps: ListingPresentationDeps): ListingPresentation {
  let settledListingId = $state('')
  let settledTotalCount = $state(0)
  let showLoading = $state(deps.getLoading())

  $effect(() => {
    const loading = deps.getLoading()
    let delayTimer: ReturnType<typeof setTimeout> | undefined

    if (!loading) {
      const listingId = deps.getListingId()
      if (listingId) {
        settledListingId = listingId
        settledTotalCount = deps.getTotalCount()
      }
      showLoading = false
    } else if (!settledListingId) {
      // Startup has no useful rows to preserve.
      showLoading = true
    } else {
      showLoading = false
      delayTimer = setTimeout(() => {
        showLoading = true
      }, LISTING_LOADING_DELAY_MS)
    }

    return () => {
      if (delayTimer !== undefined) clearTimeout(delayTimer)
    }
  })

  return {
    get listingId() {
      return deps.getLoading() ? settledListingId : deps.getListingId()
    },
    get totalCount() {
      return deps.getLoading() ? settledTotalCount : deps.getTotalCount()
    },
    get showLoading() {
      return showLoading
    },
  }
}
