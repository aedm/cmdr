/**
 * The listing accessors' one refusal: the backend no longer holds the listing a
 * read named (`ListingLookupError::Gone`).
 *
 * Every listing-read wrapper in `file-listing.ts` throws through
 * `throwListingLookupError`, which first tells whoever listens, so the pane showing
 * that listing can re-list instead of serving stale rows to every later command.
 * The listener that acts on it is `file-explorer/pane/listing-liveness.ts`.
 */
import type { ListingLookupError } from '$lib/ipc/bindings'
import { TypedFailure } from '$lib/ipc/typed-failure'

/** A listing read the backend couldn't answer because the listing isn't cached any more. */
class ListingLookupFailure extends TypedFailure<ListingLookupError> {
  constructor(failure: ListingLookupError) {
    super(failure, `Listing ${failure.listingId} isn't cached`)
  }
}

const listeners = new Set<(listingId: string) => void>()

/** Hears the id of every listing a read found gone. Returns the unsubscribe. */
export function onListingGone(listener: (listingId: string) => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

/** Throws a listing accessor's typed refusal, first telling every `onListingGone` listener. */
export function throwListingLookupError(error: ListingLookupError): never {
  for (const listener of listeners) listener(error.listingId)
  throw new ListingLookupFailure(error)
}
