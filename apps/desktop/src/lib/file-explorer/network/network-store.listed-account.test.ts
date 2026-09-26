/**
 * Which account a host's share list signed in as: what the Servers list and the
 * share list's header say ("as guest", "as testuser"). ❗ Only from the listings
 * themselves: nothing here reads the Keychain.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'

const { ipc } = vi.hoisted(() => ({
  ipc: {
    listSharesOnHost: vi.fn(),
    getSmbCredentials: vi.fn(),
  },
}))
vi.mock('$lib/tauri-commands', () => ipc)
vi.mock('$lib/settings/network-settings', () => ({ getNetworkTimeoutMs: () => 1000, getShareCacheTtlMs: () => 1000 }))

import { fetchShares, forgetShareListsOfMachine, getListedAccount, setListedAccount } from './network-store.svelte'
import type { NetworkHost } from '../types'

const host = (id: string): NetworkHost => ({ id, name: id, hostname: 'localhost', port: 11482, source: 'manual' })
const listing = (authMode: string) => ({ shares: [], authMode, fromCache: false })

beforeEach(() => {
  vi.clearAllMocks()
})

describe('the account a share list signed in as', () => {
  it('is guest after a guest listing', async () => {
    ipc.listSharesOnHost.mockResolvedValue(listing('guest_allowed'))
    await fetchShares(host('a'))
    expect(getListedAccount('a')).toEqual({ kind: 'guest' })
  })

  it('is the account a sign-in listed with, and a later refresh from the cache keeps it', async () => {
    setListedAccount('b', { kind: 'user', username: 'testuser' })
    ipc.listSharesOnHost.mockResolvedValue(listing('creds_required'))
    await fetchShares(host('b'))
    expect(getListedAccount('b')).toEqual({ kind: 'user', username: 'testuser' })
  })

  it('is unknown after a listing that failed, and after the machine’s lists are dropped', async () => {
    setListedAccount('c', { kind: 'guest' })
    ipc.listSharesOnHost.mockRejectedValue({ type: 'auth_required', message: 'sign in' })
    await fetchShares(host('c')).catch(() => {})
    expect(getListedAccount('c')).toBeUndefined()

    setListedAccount('d', { kind: 'guest' })
    forgetShareListsOfMachine(host('d'))
    expect(getListedAccount('d')).toBeUndefined()
    expect(ipc.getSmbCredentials).not.toHaveBeenCalled()
  })
})
