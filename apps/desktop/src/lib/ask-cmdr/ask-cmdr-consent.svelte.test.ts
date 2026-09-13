/** Unit tests for the consent gate state module (refresh / accept / revoke, fail-closed). */

import { describe, it, expect, vi, beforeEach } from 'vitest'

import type { AskCmdrConsentStatus } from '$lib/tauri-commands'

const { statusMock, acceptMock, revokeMock } = vi.hoisted(() => ({
  statusMock: vi.fn<() => Promise<AskCmdrConsentStatus>>(),
  acceptMock: vi.fn<() => Promise<void>>(),
  revokeMock: vi.fn<() => Promise<void>>(),
}))

vi.mock('$lib/tauri-commands', () => ({
  askCmdrConsentStatus: () => statusMock(),
  acceptAskCmdrConsent: () => acceptMock(),
  revokeAskCmdrConsent: () => revokeMock(),
}))
vi.mock('$lib/logging/logger', () => ({
  getAppLogger: () => ({ warn: vi.fn(), info: vi.fn(), debug: vi.fn(), error: vi.fn() }),
}))

import { consentState, refreshConsent, acceptConsent, revokeConsent } from './ask-cmdr-consent.svelte'

beforeEach(() => {
  vi.clearAllMocks()
  consentState.accepted = null
  consentState.acceptedAt = null
})

describe('refreshConsent', () => {
  it('applies an accepted status (accepted + timestamp)', async () => {
    statusMock.mockResolvedValue({ accepted: true, currentVersion: 1, acceptedVersion: 1, acceptedAt: 1_760_000_000 })
    await refreshConsent()
    expect(consentState.accepted).toBe(true)
    expect(consentState.acceptedAt).toBe(1_760_000_000)
  })

  it('clears the timestamp when not accepted', async () => {
    statusMock.mockResolvedValue({ accepted: false, currentVersion: 1, acceptedVersion: null, acceptedAt: null })
    await refreshConsent()
    expect(consentState.accepted).toBe(false)
    expect(consentState.acceptedAt).toBeNull()
  })

  it('fails CLOSED when the status read throws', async () => {
    statusMock.mockRejectedValue(new Error('nope'))
    await refreshConsent()
    expect(consentState.accepted).toBe(false)
    expect(consentState.acceptedAt).toBeNull()
  })
})

const notAccepted: AskCmdrConsentStatus = { accepted: false, currentVersion: 1, acceptedVersion: null, acceptedAt: null }
const accepted: AskCmdrConsentStatus = { accepted: true, currentVersion: 1, acceptedVersion: 1, acceptedAt: 1_760_000_100 }

describe('acceptConsent', () => {
  it('records consent, refreshes, and answers done', async () => {
    acceptMock.mockResolvedValue(undefined)
    statusMock.mockResolvedValue(accepted)
    const result = await acceptConsent()
    expect(acceptMock).toHaveBeenCalledOnce()
    expect(result).toBe('done')
    expect(consentState.accepted).toBe(true)
  })

  it('answers notSaved when the store refuses the write, and the gate stays shut', async () => {
    acceptMock.mockRejectedValue(new Error('database is locked'))
    statusMock.mockResolvedValue(notAccepted)
    expect(await acceptConsent()).toBe('notSaved')
    expect(consentState.accepted).toBe(false)
  })

  it('answers notSaved when the write went through but the store still reads not accepted', async () => {
    // A store that never opened takes the write as a no-op and reads back "not accepted".
    acceptMock.mockResolvedValue(undefined)
    statusMock.mockResolvedValue(notAccepted)
    expect(await acceptConsent()).toBe('notSaved')
  })
})

describe('revokeConsent', () => {
  it('clears consent, refreshes, and answers done', async () => {
    revokeMock.mockResolvedValue(undefined)
    statusMock.mockResolvedValue(notAccepted)
    expect(await revokeConsent()).toBe('done')
    expect(revokeMock).toHaveBeenCalledOnce()
    expect(consentState.accepted).toBe(false)
  })

  it('answers notSaved when the store refuses, and re-reads so the status stays honest', async () => {
    // Pre-fix this swallowed the refusal: a "turn off" or a wizard "no AI" pick silently left
    // consent recorded, and no caller could tell.
    revokeMock.mockRejectedValue(new Error('disk I/O error'))
    statusMock.mockResolvedValue(accepted)
    expect(await revokeConsent()).toBe('notSaved')
    expect(consentState.accepted).toBe(true)
  })
})
