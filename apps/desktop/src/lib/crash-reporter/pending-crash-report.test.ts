/**
 * The next-launch crash-report check: which surface a pending report reaches, and which log level a
 * failure earns. A frontend `log.error` auto-sends an error report, so only what means Cmdr is broken
 * may reach it: a check that can't refuse breaking, or Cmdr's own server turning the report down.
 * Network trouble stays at warn, and the crash file stays for next launch either way.
 */

import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { CrashReport } from '$lib/tauri-commands'
import { ServerRequestFailure } from '$lib/error-messages/server-request'

const { api, getSetting, addToast, logger } = vi.hoisted(() => ({
  api: {
    checkPendingCrashReport: vi.fn<() => Promise<CrashReport | null>>(),
    sendCrashReport: vi.fn<(report: CrashReport) => Promise<void>>(),
  },
  getSetting: vi.fn<(id: string) => unknown>(),
  addToast: vi.fn(),
  logger: { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() },
}))

vi.mock('$lib/tauri-commands', () => api)
vi.mock('$lib/settings', () => ({ getSetting }))
vi.mock('$lib/ui/toast', () => ({ addToast }))
vi.mock('$lib/logging/logger', () => ({ getAppLogger: () => logger }))
vi.mock('./CrashReportToastContent.svelte', () => ({ default: {} }))

import { checkForPendingCrashReport } from './pending-crash-report'

function aReport(possibleCrashLoop = false): CrashReport {
  return { possibleCrashLoop } as unknown as CrashReport
}

const showDialog = vi.fn<(report: CrashReport) => void>()

describe('checkForPendingCrashReport', () => {
  beforeEach(() => {
    vi.resetAllMocks()
    getSetting.mockReturnValue(true)
    api.sendCrashReport.mockResolvedValue(undefined)
  })

  it('does nothing on a clean launch', async () => {
    api.checkPendingCrashReport.mockResolvedValue(null)
    await checkForPendingCrashReport(showDialog)
    expect(api.sendCrashReport).not.toHaveBeenCalled()
    expect(showDialog).not.toHaveBeenCalled()
  })

  it('sends and says so when crash reports are on', async () => {
    const report = aReport()
    api.checkPendingCrashReport.mockResolvedValue(report)
    await checkForPendingCrashReport(showDialog)
    expect(api.sendCrashReport).toHaveBeenCalledWith(report)
    expect(addToast).toHaveBeenCalledOnce()
    expect(showDialog).not.toHaveBeenCalled()
  })

  it('asks instead of sending when the app looks stuck in a crash loop', async () => {
    const report = aReport(true)
    api.checkPendingCrashReport.mockResolvedValue(report)
    await checkForPendingCrashReport(showDialog)
    expect(api.sendCrashReport).not.toHaveBeenCalled()
    expect(showDialog).toHaveBeenCalledWith(report)
  })

  it('asks when crash reports are off', async () => {
    getSetting.mockReturnValue(false)
    api.checkPendingCrashReport.mockResolvedValue(aReport())
    await checkForPendingCrashReport(showDialog)
    expect(api.sendCrashReport).not.toHaveBeenCalled()
    expect(showDialog).toHaveBeenCalledOnce()
  })

  it('keeps an auto-send the network didn’t carry at warn, and shows no toast', async () => {
    api.checkPendingCrashReport.mockResolvedValue(aReport())
    api.sendCrashReport.mockRejectedValue(new ServerRequestFailure({ type: 'unreachable', detail: 'dns error' }))
    await checkForPendingCrashReport(showDialog)
    expect(logger.warn).toHaveBeenCalledOnce()
    expect(logger.error).not.toHaveBeenCalled()
    expect(addToast).not.toHaveBeenCalled()
  })

  it('logs an auto-send Cmdr’s server turned down at error, since the contract broke', async () => {
    api.checkPendingCrashReport.mockResolvedValue(aReport())
    api.sendCrashReport.mockRejectedValue(new ServerRequestFailure({ type: 'refused', status: 422, detail: '{}' }))
    await checkForPendingCrashReport(showDialog)
    expect(logger.error).toHaveBeenCalledOnce()
    expect(logger.warn).not.toHaveBeenCalled()
  })

  it('logs a check that broke at error, since the command can’t refuse and only a broken bridge lands there', async () => {
    api.checkPendingCrashReport.mockRejectedValue(new Error('bridge broke'))
    await checkForPendingCrashReport(showDialog)
    expect(logger.error).toHaveBeenCalledOnce()
    expect(logger.warn).not.toHaveBeenCalled()
    expect(showDialog).not.toHaveBeenCalled()
  })
})
