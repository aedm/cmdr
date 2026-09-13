/**
 * Tests for the macOS update check wrapper: a check that doesn't land has to reach the updater as the typed
 * `ServerRequestError`, or every failure reads as untyped and the updater can't word it or pick its log level.
 */

import { describe, expect, it, vi } from 'vitest'

vi.mock('$lib/ipc/bindings', () => ({
  commands: { checkForUpdate: vi.fn() },
}))

import { commands } from '$lib/ipc/bindings'
import { serverRequestFailureOf } from '$lib/error-messages/server-request'
import { checkForUpdate } from './updates'

describe('checkForUpdate', () => {
  it('throws the typed request failure, so the updater can word it and pick its log level', async () => {
    const failure = { type: 'badResponse', detail: 'missing field `platforms` at line 1 column 20' } as const
    vi.mocked(commands.checkForUpdate).mockResolvedValueOnce({ status: 'error', error: failure })

    const caught = await checkForUpdate().catch((e: unknown) => e)

    expect(serverRequestFailureOf(caught)).toEqual(failure)
  })

  it('answers the offered update when the check lands', async () => {
    const update = { version: '0.45.1', url: 'https://example.invalid/Cmdr.tar.gz', signature: 'sig' }
    vi.mocked(commands.checkForUpdate).mockResolvedValueOnce({ status: 'ok', data: update })

    expect(await checkForUpdate()).toEqual(update)
  })
})
