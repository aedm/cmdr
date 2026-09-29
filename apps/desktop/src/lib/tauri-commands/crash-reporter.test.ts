/** The crash-report send wrapper exposes consent inputs, never previewed payload fields. */

import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('$lib/ipc/bindings', () => ({
  commands: {
    sendCrashReport: vi.fn(),
  },
}))

import { commands } from '$lib/ipc/bindings'
import { sendCrashReport } from './crash-reporter'

describe('sendCrashReport wrapper', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('sends only the pending report id and separately consented email', async () => {
    vi.mocked(commands.sendCrashReport).mockResolvedValueOnce({ status: 'ok', data: null })

    await sendCrashReport('CRASH-A2345', 'explicit@example.test')

    expect(commands.sendCrashReport).toHaveBeenCalledExactlyOnceWith('CRASH-A2345', 'explicit@example.test')
  })

  it('sends null when no email was attached', async () => {
    vi.mocked(commands.sendCrashReport).mockResolvedValueOnce({ status: 'ok', data: null })

    await sendCrashReport('CRASH-A2345')

    expect(commands.sendCrashReport).toHaveBeenCalledExactlyOnceWith('CRASH-A2345', null)
  })
})
