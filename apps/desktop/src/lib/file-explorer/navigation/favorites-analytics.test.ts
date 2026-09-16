/**
 * The two favorites events, as they go over the wire.
 *
 * The event NAMES are the contract `analytics-event-catalog` pins against
 * `src-tauri/src/analytics/DETAILS.md`, and the props are what a dashboard
 * groups by — neither is checked by a type once it leaves here, so both are
 * pinned as literals.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'

const trackEvent = vi.fn()
vi.mock('$lib/tauri-commands', () => ({
  trackEvent: (...args: unknown[]) => {
    trackEvent(...(args as []))
    return Promise.resolve()
  },
}))

import { reportFavoriteOpened, reportFavoritesMenuOpened } from './favorites-analytics'

beforeEach(() => trackEvent.mockReset())

describe('reportFavoriteOpened', () => {
  it('sends both props, so `via` is never missing from a row', () => {
    reportFavoriteOpened({ surface: 'favorites_menu', via: 'digit' })
    expect(trackEvent).toHaveBeenCalledWith('favorite_opened', { surface: 'favorites_menu', via: 'digit' })
  })

  it('pairs the command surface with the command via', () => {
    // The arm where the command IS the interaction: no row to point at, so no
    // digit / keyboard / pointer answer exists. The payload union is what makes
    // any other pairing a compile error.
    reportFavoriteOpened({ surface: 'command', via: 'command' })
    expect(trackEvent).toHaveBeenCalledWith('favorite_opened', { surface: 'command', via: 'command' })
  })
})

describe('reportFavoritesMenuOpened', () => {
  it('names both ways the menu comes up', () => {
    reportFavoritesMenuOpened({ trigger: 'command' })
    reportFavoritesMenuOpened({ trigger: 'switcher_row' })
    expect(trackEvent).toHaveBeenNthCalledWith(1, 'favorites_menu_opened', { trigger: 'command' })
    expect(trackEvent).toHaveBeenNthCalledWith(2, 'favorites_menu_opened', { trigger: 'switcher_row' })
  })
})
