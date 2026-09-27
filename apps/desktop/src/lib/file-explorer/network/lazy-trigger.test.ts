/**
 * The one chokepoint for a network action, and the two things it must get right.
 *
 * ❗ The action's upgrade pass dials private IPs and browses mDNS, which is what
 * fires the macOS "Cmdr wants to find devices on local networks" prompt. So a call
 * that slips past the `network.enabled` gate shows a system permission dialog to
 * someone who turned discovery off, which is the failure this file exists to
 * prevent.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'

const noteNetworkAction = vi.fn(() => Promise.resolve())
const settings = new Map<string, unknown>()

vi.mock('$lib/tauri-commands', () => ({
  noteNetworkAction: () => noteNetworkAction(),
}))
vi.mock('$lib/settings', () => ({
  getSetting: (id: string) => settings.get(id),
  setSetting: (id: string, value: unknown) => {
    settings.set(id, value)
  },
}))

import { triggerNetworkDiscovery } from './lazy-trigger'

beforeEach(() => {
  vi.clearAllMocks()
  settings.clear()
  settings.set('network.enabled', true)
  settings.set('network.firstTriggerDone', false)
})

describe('triggerNetworkDiscovery', () => {
  it('notes the action and records that the prompt has been paid for', () => {
    triggerNetworkDiscovery()
    expect(noteNetworkAction).toHaveBeenCalledTimes(1)
    // Later launches warm the server list up, so a returning user gets it at once
    // without re-prompting.
    expect(settings.get('network.firstTriggerDone')).toBe(true)
  })

  it('❌ never browses while discovery is off', () => {
    settings.set('network.enabled', false)
    triggerNetworkDiscovery()
    expect(noteNetworkAction).not.toHaveBeenCalled()
    expect(settings.get('network.firstTriggerDone')).toBe(false)
  })

  it('is idempotent: a second call notes again and writes nothing', () => {
    triggerNetworkDiscovery()
    triggerNetworkDiscovery()
    // The backend command is idempotent, so calling it again is free; the
    // setting is already true, so nothing writes.
    expect(noteNetworkAction).toHaveBeenCalledTimes(2)
    expect(settings.get('network.firstTriggerDone')).toBe(true)
  })
})
