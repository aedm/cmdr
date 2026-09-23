/**
 * The settings-applier's push for `askCmdr.enabled`.
 *
 * The backend reads the switch fresh from `settings.json` (the send gate, the wake loop's
 * readiness), and the store's usual save is debounced. So a flip must land on disk BEFORE the
 * backend is told to re-read, or "Turn on Ask Cmdr, then send" races the flush and the send
 * is refused as off.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

const { order } = vi.hoisted(() => ({ order: [] as string[] }))

/** The change listener the applier registers, captured so the test can fire it. */
let changeListener: ((change: { id: string; value: unknown }) => void) | undefined

vi.mock('$lib/tauri-commands', async (importOriginal) => {
  const actual = await importOriginal<Record<string, unknown>>()
  const noop = () => Promise.resolve()
  const stubbed: Record<string, unknown> = {}
  for (const key of Object.keys(actual)) {
    stubbed[key] = typeof actual[key] === 'function' ? noop : actual[key]
  }
  stubbed.askCmdrEnabledChanged = () => {
    order.push('enabledChanged')
    return Promise.resolve()
  }
  return stubbed
})

vi.mock('$lib/settings', async (importOriginal) => {
  const actual = await importOriginal<typeof import('$lib/settings')>()
  return {
    ...actual,
    initializeSettings: vi.fn().mockResolvedValue(undefined),
    forceSave: () => {
      order.push('save')
      return Promise.resolve(true)
    },
    onSettingChange: (listener: (change: { id: string; value: unknown }) => void) => {
      changeListener = listener
      return () => {
        changeListener = undefined
      }
    },
  }
})

import { initSettingsApplier, cleanupSettingsApplier } from './settings-applier'

beforeEach(() => {
  order.length = 0
  changeListener = undefined
})

afterEach(() => {
  cleanupSettingsApplier()
})

describe('settings-applier: askCmdr.enabled', () => {
  it('saves settings.json first, then tells the backend to re-read the switch', async () => {
    await initSettingsApplier()
    order.length = 0
    changeListener?.({ id: 'askCmdr.enabled', value: true })
    await vi.waitFor(() => {
      expect(order).toEqual(['save', 'enabledChanged'])
    })
  })
})
