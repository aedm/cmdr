/**
 * Tests for the FE-owned drive-indexing prefs: per-drive silences and the
 * one-time stale-dialog flag, stored as hidden settings. The settings store is
 * mocked so the JSON-array plumbing is exercised without disk I/O.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'

let store: Record<string, unknown>

vi.mock('$lib/settings', () => ({
  getSetting: (id: string) => store[id],
  setSetting: (id: string, value: unknown) => {
    store[id] = value
  },
}))

const { warn } = vi.hoisted(() => ({ warn: vi.fn() }))
vi.mock('$lib/logging/logger', () => ({
  getAppLogger: () => ({ warn, debug: vi.fn(), info: vi.fn(), error: vi.fn() }),
}))

import {
  getSilencedDrives,
  isDriveSilenced,
  silenceDrive,
  clearSilencedDrives,
  hasSilencedDrives,
  hasShownFirstStaleDialog,
  markFirstStaleDialogShown,
  resetFirstStaleDialogShown,
} from './drive-index-prefs'

beforeEach(() => {
  warn.mockClear()
  store = {
    'indexing.silencedDrives': '[]',
    'indexing.firstStaleDialogShown': false,
  }
})

describe('silenced drives', () => {
  it('starts empty', () => {
    expect(getSilencedDrives()).toEqual([])
    expect(hasSilencedDrives()).toBe(false)
  })

  it('silences a drive idempotently', () => {
    silenceDrive('smb-a')
    silenceDrive('smb-a')
    expect(getSilencedDrives()).toEqual(['smb-a'])
    expect(isDriveSilenced('smb-a')).toBe(true)
    expect(isDriveSilenced('smb-b')).toBe(false)
    expect(hasSilencedDrives()).toBe(true)
  })

  it('clears all silences', () => {
    silenceDrive('smb-a')
    silenceDrive('smb-b')
    clearSilencedDrives()
    expect(getSilencedDrives()).toEqual([])
    expect(hasSilencedDrives()).toBe(false)
  })

  it('tolerates a corrupt stored value', () => {
    store['indexing.silencedDrives'] = 'not json'
    expect(getSilencedDrives()).toEqual([])
    // And a non-array JSON value.
    store['indexing.silencedDrives'] = '{"a":1}'
    expect(getSilencedDrives()).toEqual([])
  })

  it('drops non-string entries from a malformed array', () => {
    store['indexing.silencedDrives'] = '["smb-a", 42, null]'
    expect(getSilencedDrives()).toEqual(['smb-a'])
  })

  it('repairs a corrupt stored value, so it warns once rather than on every read', async () => {
    store['indexing.silencedDrives'] = 'not json'
    // Every reader asks again (the search CTA derives it, the first-connect prompt
    // checks per drive), so a value left corrupt would warn on each of them.
    expect(isDriveSilenced('smb-a')).toBe(false)
    expect(hasSilencedDrives()).toBe(false)
    // A reader can be a `$derived`, where a settings write is an unsafe state
    // mutation, so the repair lands after the read returns, never inside it.
    expect(store['indexing.silencedDrives']).toBe('not json')
    await Promise.resolve()
    expect(store['indexing.silencedDrives']).toBe('[]')
    expect(getSilencedDrives()).toEqual([])
    expect(warn).toHaveBeenCalledOnce()
  })

  it('keeps the valid entries when it repairs a malformed array', async () => {
    store['indexing.silencedDrives'] = '["smb-a", 42, null]'
    expect(getSilencedDrives()).toEqual(['smb-a'])
    await Promise.resolve()
    expect(store['indexing.silencedDrives']).toBe('["smb-a"]')
    expect(warn).toHaveBeenCalledOnce()
  })
})

describe('first stale dialog flag', () => {
  it('reads and writes the one-shot', () => {
    expect(hasShownFirstStaleDialog()).toBe(false)
    markFirstStaleDialogShown()
    expect(hasShownFirstStaleDialog()).toBe(true)
  })

  it('clears the one-shot, so a dev-only re-trigger can fire the dialog again', () => {
    markFirstStaleDialogShown()
    resetFirstStaleDialogShown()
    expect(hasShownFirstStaleDialog()).toBe(false)
    // The dialog gallery resets before EVERY trigger, so this has to survive a
    // second round rather than only undoing the first stamp.
    markFirstStaleDialogShown()
    resetFirstStaleDialogShown()
    expect(hasShownFirstStaleDialog()).toBe(false)
  })
})
