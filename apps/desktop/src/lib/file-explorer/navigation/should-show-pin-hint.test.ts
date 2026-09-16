/**
 * When the Network group has grown long enough to be worth a word about
 * unpinning.
 */

import { describe, it, expect } from 'vitest'
import { shouldShowPinHint, PIN_HINT_AT } from './should-show-pin-hint'

const quiet = { pinnedCount: 0, seen: false }

describe('shouldShowPinHint', () => {
  it('says nothing while the group is still short', () => {
    expect(shouldShowPinHint({ ...quiet, pinnedCount: PIN_HINT_AT - 1 })).toBe(false)
  })

  it('speaks up the moment the fifth server is pinned', () => {
    expect(shouldShowPinHint({ ...quiet, pinnedCount: PIN_HINT_AT })).toBe(true)
  })

  /**
   * A person who already had a long list before the hint existed is exactly who
   * it is for, so the rule is "at least five", ❌ not "the fifth one just
   * landed": nothing records what the count was last launch.
   */
  it('speaks up for a list that was already long', () => {
    expect(shouldShowPinHint({ ...quiet, pinnedCount: 12 })).toBe(true)
  })

  it('stays quiet forever once the user has said Got it', () => {
    expect(shouldShowPinHint({ ...quiet, pinnedCount: 40, seen: true })).toBe(false)
  })
})
