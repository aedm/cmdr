/** Unit tests for the consent gate state module (refresh / accept / revoke, fail-closed, the held revoke). */

import { describe, it, expect, vi, beforeEach } from 'vitest'

import type { AskCmdrConsentStatus } from '$lib/tauri-commands'

const { statusMock, acceptMock, revokeMock, pendingChangedMock, settingsMock, order } = vi.hoisted(() => {
  const order: string[] = []
  // An annotation, not an `as`: the lint auto-fix strips an assertion it thinks is unnecessary.
  const values: Record<string, unknown> = {}
  return {
    order,
    statusMock: vi.fn<() => Promise<AskCmdrConsentStatus>>(),
    acceptMock: vi.fn<() => Promise<void>>(),
    revokeMock: vi.fn<() => Promise<void>>(),
    pendingChangedMock: vi.fn<() => Promise<void>>(() => Promise.resolve()),
    settingsMock: {
      values,
      forceSave: vi.fn<() => Promise<boolean>>(() => Promise.resolve(true)),
    },
  }
})

vi.mock('$lib/tauri-commands', () => ({
  askCmdrConsentStatus: () => statusMock(),
  acceptAskCmdrConsent: () => {
    order.push('accept')
    return acceptMock()
  },
  revokeAskCmdrConsent: () => revokeMock(),
  askCmdrConsentRevokePendingChanged: () => pendingChangedMock(),
}))
vi.mock('$lib/settings', () => ({
  getSetting: (id: string): unknown => settingsMock.values[id] ?? false,
  setSetting: (id: string, value: unknown) => {
    order.push(`set ${id}=${String(value)}`)
    settingsMock.values[id] = value
  },
  forceSave: () => {
    order.push('save')
    return settingsMock.forceSave()
  },
}))
vi.mock('$lib/logging/logger', () => ({
  getAppLogger: () => ({ warn: vi.fn(), info: vi.fn(), debug: vi.fn(), error: vi.fn() }),
}))

import {
  consentState,
  refreshConsent,
  acceptConsent,
  revokeConsent,
  declineConsent,
  holdConsentRevoke,
  settleHeldConsentRevoke,
} from './ask-cmdr-consent.svelte'

const HELD = 'askCmdr.consentRevokePending'

beforeEach(() => {
  // Reset, not clear: a `mockRejectedValueOnce` a test didn't consume must not leak into the next.
  vi.resetAllMocks()
  order.length = 0
  settingsMock.values = {}
  settingsMock.forceSave.mockResolvedValue(true)
  pendingChangedMock.mockResolvedValue(undefined)
  consentState.accepted = null
  consentState.acceptedAt = null
  consentState.needsReconsent = false
})

const notAccepted: AskCmdrConsentStatus = {
  accepted: false,
  currentVersion: 1,
  acceptedVersion: null,
  acceptedAt: null,
}
const accepted: AskCmdrConsentStatus = {
  accepted: true,
  currentVersion: 1,
  acceptedVersion: 1,
  acceptedAt: 1_760_000_100,
}

describe('refreshConsent', () => {
  it('applies an accepted status (accepted + timestamp)', async () => {
    statusMock.mockResolvedValue({ accepted: true, currentVersion: 1, acceptedVersion: 1, acceptedAt: 1_760_000_000 })
    await refreshConsent()
    expect(consentState.accepted).toBe(true)
    expect(consentState.acceptedAt).toBe(1_760_000_000)
  })

  it('clears the timestamp when not accepted', async () => {
    statusMock.mockResolvedValue(notAccepted)
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

  it('makes no revoke attempt when no "no" is held', async () => {
    statusMock.mockResolvedValue(notAccepted)
    await refreshConsent()
    expect(revokeMock).not.toHaveBeenCalled()
  })

  it('retries a held revoke, and lets go of it once the store takes it', async () => {
    settingsMock.values[HELD] = true
    revokeMock.mockResolvedValue(undefined)
    statusMock.mockResolvedValue(notAccepted)

    await refreshConsent()

    expect(revokeMock).toHaveBeenCalledOnce()
    expect(settingsMock.values[HELD]).toBe(false)
    expect(settingsMock.forceSave).toHaveBeenCalled()
    expect(pendingChangedMock).toHaveBeenCalled()
  })

  it('keeps holding the "no" when the store refuses the retry too', async () => {
    settingsMock.values[HELD] = true
    revokeMock.mockRejectedValue(new Error('disk I/O error'))
    statusMock.mockResolvedValue(notAccepted)

    await refreshConsent()

    expect(revokeMock).toHaveBeenCalledOnce()
    expect(settingsMock.values[HELD]).toBe(true)
  })

  it('never reads a held "no" as someone to ask again about changed wording', async () => {
    // The backend answers not-accepted while a revoke is held, and the store's audit still
    // names the version they once accepted. That's a "no", not a paused opt-in.
    settingsMock.values[HELD] = true
    revokeMock.mockRejectedValue(new Error('disk I/O error'))
    statusMock.mockResolvedValue({ accepted: false, currentVersion: 1, acceptedVersion: 1, acceptedAt: null })

    await refreshConsent()

    expect(consentState.accepted).toBe(false)
    expect(consentState.needsReconsent).toBe(false)
  })
})

describe('settleHeldConsentRevoke', () => {
  it('is the same retry the launch runs, and does nothing without a held "no"', async () => {
    await settleHeldConsentRevoke()
    expect(revokeMock).not.toHaveBeenCalled()

    settingsMock.values[HELD] = true
    revokeMock.mockResolvedValue(undefined)
    await settleHeldConsentRevoke()
    expect(revokeMock).toHaveBeenCalledOnce()
    expect(settingsMock.values[HELD]).toBe(false)
  })
})

describe('holdConsentRevoke', () => {
  it('holds the "no" in settings, saves it now, and tells the backend gates', async () => {
    const saved = await holdConsentRevoke()

    expect(saved).toBe(true)
    expect(settingsMock.values[HELD]).toBe(true)
    // Saved BEFORE the backend is told: the gates read `settings.json` from disk.
    expect(order).toEqual([`set ${HELD}=true`, 'save'])
    expect(pendingChangedMock).toHaveBeenCalledOnce()
  })

  it("answers false when the settings file won't take it either", async () => {
    settingsMock.forceSave.mockResolvedValue(false)
    expect(await holdConsentRevoke()).toBe(false)
  })
})

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

  it('lets go of a held "no" before recording a deliberate yes', async () => {
    settingsMock.values[HELD] = true
    acceptMock.mockResolvedValue(undefined)
    statusMock.mockResolvedValue(accepted)

    await acceptConsent()

    expect(settingsMock.values[HELD]).toBe(false)
    expect(order.indexOf(`set ${HELD}=false`)).toBeLessThan(order.indexOf('accept'))
    expect(order.indexOf('save')).toBeLessThan(order.indexOf('accept'))
    expect(revokeMock).not.toHaveBeenCalled()
  })
})

describe('declineConsent', () => {
  it('turns Ask Cmdr off, with nothing held, when the store takes the first try', async () => {
    revokeMock.mockResolvedValue(undefined)
    statusMock.mockResolvedValue(notAccepted)

    expect(await declineConsent()).toBe('done')
    expect(revokeMock).toHaveBeenCalledOnce()
    expect(settingsMock.values[HELD]).toBeUndefined()
  })

  it('gives a refused "no" one more try before holding it', async () => {
    revokeMock.mockRejectedValueOnce(new Error('database is locked')).mockResolvedValue(undefined)
    statusMock.mockResolvedValue(notAccepted)

    expect(await declineConsent()).toBe('done')
    expect(revokeMock).toHaveBeenCalledTimes(2)
    expect(settingsMock.values[HELD]).toBeUndefined()
  })

  it('holds a "no" the store refuses twice, and answers done: it holds from the next check on', async () => {
    revokeMock.mockRejectedValue(new Error('disk I/O error'))
    statusMock.mockResolvedValue(notAccepted)

    expect(await declineConsent()).toBe('done')
    expect(settingsMock.values[HELD]).toBe(true)
    expect(pendingChangedMock).toHaveBeenCalled()
  })

  it('answers notSaved only when settings.json won\'t hold the "no" either', async () => {
    revokeMock.mockRejectedValue(new Error('disk I/O error'))
    settingsMock.forceSave.mockResolvedValue(false)
    statusMock.mockResolvedValue(accepted)

    expect(await declineConsent()).toBe('notSaved')
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
