/**
 * Pins how a failed viewer open is logged. An ERROR log counts toward an auto-sent error
 * report, so it's for failures that are ours to fix. A typed backend outcome (a timeout,
 * a file that's gone) is something the window already shows with a way forward: at error
 * level those filed reports of their own (ERR-GW3BE, ERR-XV6SN).
 */

import { describe, expect, it, vi } from 'vitest'

import { handleOpenFailure } from './viewer-open-failure'
import { tString } from '$lib/intl/messages.svelte'
import type { Logger } from '$lib/logging/logger'
import type { ViewerError } from '$lib/ipc/bindings'

function fakeLog() {
  const log = { trace: vi.fn(), debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() }
  return { log, asLogger: log as unknown as Logger }
}

/** The shape `viewerOpen` throws: an `Error` carrying the typed `ViewerError`. */
function viewerFailure(viewerError: ViewerError): Error {
  return Object.assign(new Error(viewerError.kind), { viewerError })
}

describe('handleOpenFailure', () => {
  it('logs a timed-out open at warn, and offers Retry', () => {
    const { log, asLogger } = fakeLog()

    const failure = handleOpenFailure(asLogger, 'Open', viewerFailure({ kind: 'timedOut' }))

    expect(failure).toEqual({ message: tString('viewer.error.timeout'), canRetry: true })
    expect(log.error).not.toHaveBeenCalled()
    expect(log.warn).toHaveBeenCalledTimes(1)
  })

  it('logs a file that is gone at warn', () => {
    const { log, asLogger } = fakeLog()

    const failure = handleOpenFailure(asLogger, 'Retry', viewerFailure({ kind: 'notFound', path: '/gone.txt' }))

    expect(failure).toEqual({ message: tString('viewer.error.readFailed'), canRetry: false })
    expect(log.error).not.toHaveBeenCalled()
    expect(log.warn).toHaveBeenCalledTimes(1)
  })

  it('keeps a failure that never reached the typed path at error, since that one is a defect', () => {
    const { log, asLogger } = fakeLog()

    const failure = handleOpenFailure(asLogger, 'Open', new TypeError('undefined is not an object'))

    expect(failure).toEqual({ message: tString('viewer.error.readFailed'), canRetry: false })
    expect(log.error).toHaveBeenCalledTimes(1)
    expect(log.warn).not.toHaveBeenCalled()
  })
})
