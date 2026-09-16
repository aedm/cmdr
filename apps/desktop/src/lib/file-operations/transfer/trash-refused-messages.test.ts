/**
 * What a refused trash says, and when it offers Full Disk Access as the next step.
 *
 * The offer is the delicate part: too eager and Cmdr blames a permission for every
 * failure until people stop reading it; too shy and the one user who needed the hint
 * gets "try again" under a folded disclosure, which is the report that started this.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { _setLocaleForTests } from '$lib/intl/locale'
import type { TrashRefusalKind } from '$lib/ipc/bindings'

// `vi.hoisted`, because the mock factories below are hoisted above this file's imports
// and run while another module is still loading: a plain `const` is still in its TDZ then.
const { isMacOS, fdaIsMissing } = vi.hoisted(() => ({
  isMacOS: vi.fn<() => boolean>(() => true),
  fdaIsMissing: vi.fn<() => boolean>(() => false),
}))

vi.mock('$lib/shortcuts/key-capture', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/shortcuts/key-capture')>()),
  isMacOS: () => isMacOS(),
}))
vi.mock('$lib/onboarding/fda-status.svelte', () => ({
  fdaIsMissing: () => fdaIsMissing(),
}))

import { getUserFriendlyMessage, mayBeAPermissionGrantAway } from './transfer-error-messages'

const ALL_REASONS: TrashRefusalKind[] = ['notPermitted', 'noTrashForVolume', 'other']

function refused(reason: TrashRefusalKind, itemCount = 7) {
  return getUserFriendlyMessage({ type: 'trash_refused', itemCount, reason, message: 'os words' }, 'trash')
}

describe('a refused trash', () => {
  beforeEach(() => {
    _setLocaleForTests('en-US')
    isMacOS.mockReturnValue(true)
    fdaIsMissing.mockReturnValue(false)
  })

  afterEach(() => {
    _setLocaleForTests(null)
  })

  it('words every reason distinctly, so the dialog never repeats itself', () => {
    const messages = ALL_REASONS.map((r) => refused(r).message)
    expect(new Set(messages).size).toBe(ALL_REASONS.length)
    const suggestions = ALL_REASONS.map((r) => refused(r).suggestion)
    expect(new Set(suggestions).size).toBe(ALL_REASONS.length)
  })

  it('names how many items it could not take', () => {
    expect(refused('notPermitted', 7).message).toContain('7')
  })

  // Retrying a permission refusal produces the identical refusal. The old wording said
  // "Try again. If the problem persists, check the technical details below."
  it('never tells the user to try again', () => {
    for (const reason of ALL_REASONS) {
      expect(refused(reason).suggestion.toLowerCase()).not.toContain('try again')
    }
  })

  it('offers a permanent delete for the two reasons where that is the way through', () => {
    expect(refused('notPermitted').suggestion).toContain('Shift+F8')
    expect(refused('noTrashForVolume').suggestion).toContain('Shift+F8')
  })

  describe('the Full Disk Access line', () => {
    it('stays away while the grant is there', () => {
      fdaIsMissing.mockReturnValue(false)
      for (const reason of ALL_REASONS) {
        expect(refused(reason).suggestion).not.toContain('full disk access')
      }
    })

    it('joins a permission-shaped refusal when the grant is missing', () => {
      fdaIsMissing.mockReturnValue(true)
      expect(refused('notPermitted').suggestion).toContain('full disk access')
      expect(refused('noTrashForVolume').suggestion).toContain('full disk access')
    })

    // A refusal Cmdr couldn't classify is not evidence about permissions. Offering the
    // grant there is how the hint becomes noise people learn to skip.
    it('stays away from an unclassified refusal even when the grant is missing', () => {
      fdaIsMissing.mockReturnValue(true)
      expect(refused('other').suggestion).not.toContain('full disk access')
    })

    it('stays away off macOS, where the permission does not exist', () => {
      fdaIsMissing.mockReturnValue(true)
      isMacOS.mockReturnValue(false)
      expect(refused('notPermitted').suggestion).not.toContain('full disk access')
    })

    // Walks every variant, so a reason added later can't silently default into the offer.
    it('is decided per reason, with the unclassified one opted out', () => {
      expect(ALL_REASONS.filter(mayBeAPermissionGrantAway)).toEqual(['notPermitted', 'noTrashForVolume'])
    })
  })
})
