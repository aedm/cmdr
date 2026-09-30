/**
 * Every connection state the dot can carry says something of its own.
 *
 * The failure this catches used to ship: five states shared two sentences, so a
 * signed-out SFTP server hovered as "Using system connection", which is about
 * SMB's kernel mount and neither true nor actionable. A `Record` keyed on the
 * union makes a missing sentence a compile error; this makes a BORROWED one a
 * test failure.
 */
import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { _setLocaleForTests } from '$lib/intl/locale'
import { getConnectionTooltip } from './connection-tooltips'
import type { ConnectionState } from '../types'

beforeAll(() => {
  _setLocaleForTests('en-US')
})
afterAll(() => {
  _setLocaleForTests(null)
})

/**
 * One key per `ConnectionState`. ❗ A `Record` over the union, so adding a state
 * without listing it here fails to typecheck; an array literal typed
 * `ConnectionState[]` would stay green with the new state missing.
 */
const COVERED: Record<ConnectionState, true> = {
  direct: true,
  os_mount: true,
  disconnected: true,
  needs_sign_in: true,
  needs_host_key_approval: true,
  saved: true,
}
const STATES = Object.keys(COVERED) as ConnectionState[]

describe('getConnectionTooltip', () => {
  it('gives every state a sentence, and no two states the same one', () => {
    const said = STATES.map((state) => getConnectionTooltip(state))
    for (const [i, sentence] of said.entries()) {
      expect(sentence, `${STATES[i]} has no words`).not.toBe('')
      // A missing catalog key renders as the key itself.
      expect(sentence, `${STATES[i]} renders its key`).not.toContain('fileExplorer.')
    }
    expect(new Set(said).size).toBe(STATES.length)
  })

  it('words the three server states for what the user does next', () => {
    // ❗ The wording contract, not just its presence: `disconnected` is Cmdr's
    // job (the backoff loop owns it), the other two are the user's.
    expect(getConnectionTooltip('disconnected')).toContain('Cmdr')
    expect(getConnectionTooltip('needs_sign_in')).toContain('sign in')
    expect(getConnectionTooltip('saved')).toContain('connect')
  })
})
