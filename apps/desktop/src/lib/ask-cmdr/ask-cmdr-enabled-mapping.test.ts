/**
 * The one-time `askCmdr.enabled` mapping for installs that predate the switch (`lib/ask-cmdr/DETAILS.md` § Gates, cost, and settings).
 * Someone who accepted Ask Cmdr's old opt-in keeps it on; someone who saw it and didn't, or
 * held a "no", stays off. It runs at every main-window launch but writes at most once.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest'
import type { LegacyAskCmdrOptIn } from '$lib/tauri-commands'

const { legacyMock, settings, explicit } = vi.hoisted(() => {
  // An annotation, not an `as`: the lint auto-fix strips an assertion it thinks is unnecessary.
  const settings: Record<string, unknown> = {}
  return { legacyMock: vi.fn<() => Promise<LegacyAskCmdrOptIn>>(), settings, explicit: new Set<string>() }
})

vi.mock('$lib/tauri-commands', () => ({
  askCmdrLegacyOptIn: () => legacyMock(),
}))
vi.mock('$lib/settings', () => ({
  getSetting: (id: string): unknown => settings[id],
  setSetting: vi.fn((id: string, value: unknown) => {
    settings[id] = value
    explicit.add(id)
  }),
  isExplicitlySet: (id: string) => explicit.has(id),
}))
vi.mock('$lib/logging/logger', () => ({
  getAppLogger: () => ({ warn: vi.fn(), info: vi.fn(), debug: vi.fn(), error: vi.fn() }),
}))

import { mapLegacyAskCmdrOptIn } from './ask-cmdr-enabled-mapping'
import { setSetting } from '$lib/settings'

beforeEach(() => {
  vi.clearAllMocks()
  for (const key of Object.keys(settings)) Reflect.deleteProperty(settings, key)
  explicit.clear()
  settings['onboarding.completed'] = true
})

describe('mapLegacyAskCmdrOptIn', () => {
  it('keeps Ask Cmdr on for someone who accepted the old opt-in', async () => {
    legacyMock.mockResolvedValue('recorded')
    await mapLegacyAskCmdrOptIn()
    expect(setSetting).toHaveBeenCalledExactlyOnceWith('askCmdr.enabled', true)
  })

  it('keeps it off for someone who saw the opt-in and never took it, or held a "no"', async () => {
    legacyMock.mockResolvedValue('notRecorded')
    await mapLegacyAskCmdrOptIn()
    expect(setSetting).toHaveBeenCalledExactlyOnceWith('askCmdr.enabled', false)
  })

  it('writes nothing when the store is unreadable, so the next launch asks again', async () => {
    legacyMock.mockResolvedValue('storeUnavailable')
    await mapLegacyAskCmdrOptIn()
    expect(setSetting).not.toHaveBeenCalled()
  })

  it('writes nothing when the call itself breaks', async () => {
    legacyMock.mockRejectedValue(new Error('command not found'))
    await mapLegacyAskCmdrOptIn()
    expect(setSetting).not.toHaveBeenCalled()
  })

  it("never asks once the switch was set explicitly: that's the person's own answer", async () => {
    explicit.add('askCmdr.enabled')
    await mapLegacyAskCmdrOptIn()
    expect(legacyMock).not.toHaveBeenCalled()
  })

  it('leaves a fresh install to onboarding, which sets the switch from the AI pick', async () => {
    settings['onboarding.completed'] = false
    await mapLegacyAskCmdrOptIn()
    expect(legacyMock).not.toHaveBeenCalled()
  })

  it('maps once: the second launch finds the switch set and does nothing', async () => {
    legacyMock.mockResolvedValue('recorded')
    await mapLegacyAskCmdrOptIn()
    await mapLegacyAskCmdrOptIn()
    expect(legacyMock).toHaveBeenCalledOnce()
  })
})
