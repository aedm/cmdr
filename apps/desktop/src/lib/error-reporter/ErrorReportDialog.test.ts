/**
 * What `ErrorReportDialog` shows when a preview, a send, or an amend doesn't land.
 *
 * The toast used to interpolate the raw Rust string ("upload request: error sending request for
 * url …", or a 400 with the server's JSON), and a preview that couldn't be built printed its raw
 * reason while leaving Send live on a bundle nobody had seen. Now every one of those is worded from
 * the catalog, the raw detail goes to the log only, and a preview that didn't build offers Try again
 * instead of Send.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, tick, unmount } from 'svelte'
import { tString } from '$lib/intl/messages.svelte'
import ErrorReportDialog from './ErrorReportDialog.svelte'
import {
  closeErrorReportDialog,
  openErrorReportDialog,
  openErrorReportDialogForAutoSentReport,
} from './error-report-flow.svelte'
import { ErrorReportSendFailure } from './error-report-send-error'

const { api, addToast, logger } = vi.hoisted(() => ({
  api: {
    prepareErrorReportPreview: vi.fn(),
    sendErrorReport: vi.fn(),
    amendErrorReport: vi.fn(),
    getAutoSentReportPreview: vi.fn(),
    saveErrorReportToDisk: vi.fn(),
  },
  addToast: vi.fn(),
  logger: { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() },
}))

vi.mock('$lib/tauri-commands/error-reporter', () => api)

vi.mock('$lib/ui/toast', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  addToast,
}))

vi.mock('$lib/tauri-commands', () => ({
  notifyDialogOpened: vi.fn(() => Promise.resolve()),
  notifyDialogClosed: vi.fn(() => Promise.resolve()),
}))

vi.mock('$lib/settings', () => ({
  setSetting: vi.fn(),
  getSetting: vi.fn(() => ''),
  onSpecificSettingChange: () => () => {},
}))

vi.mock('$lib/logging/logger', () => ({ getAppLogger: () => logger }))

const preview = {
  id: 'ERR-AB23X',
  sizeBytes: 1234,
  manifest: {},
  sampleFirst: [],
  sampleLast: [],
  totalRedactedLines: 0,
}

let mounted: { target: HTMLElement; instance: ReturnType<typeof mount> } | undefined

async function settle(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0))
  await tick()
}

async function mountDialog(): Promise<HTMLElement> {
  const target = document.createElement('div')
  document.body.appendChild(target)
  const instance = mount(ErrorReportDialog, { target })
  mounted = { target, instance }
  await settle()
  return target
}

function button(target: HTMLElement, label: string): HTMLButtonElement | undefined {
  return Array.from(target.querySelectorAll('button')).find((b) => b.textContent.trim() === label)
}

function lastToastMessage(): string {
  const call = addToast.mock.calls.at(-1)
  if (!call) throw new Error('no toast')
  return String(call[0])
}

describe('ErrorReportDialog failures', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    api.prepareErrorReportPreview.mockResolvedValue(preview)
    api.getAutoSentReportPreview.mockResolvedValue({ ...preview, canAmend: true })
  })

  afterEach(async () => {
    if (mounted) {
      await unmount(mounted.instance)
      mounted.target.remove()
      mounted = undefined
    }
    closeErrorReportDialog()
  })

  it('words a send the server couldn’t be reached for, without the raw detail', async () => {
    openErrorReportDialog()
    api.sendErrorReport.mockRejectedValueOnce(
      new ErrorReportSendFailure({
        type: 'server',
        failure: {
          type: 'unreachable',
          detail: 'upload request: error sending request for url (http://localhost:8787)',
        },
      }),
    )
    const target = await mountDialog()

    button(target, tString('errorReporter.dialog.send'))?.click()
    await settle()

    const message = lastToastMessage()
    expect(message).toContain('Check your internet connection')
    expect(message).not.toContain('error sending request')
    expect(message).not.toContain('localhost')
  })

  it('words an amend the server turned down, without its status or its JSON', async () => {
    openErrorReportDialogForAutoSentReport()
    api.amendErrorReport.mockRejectedValueOnce(
      new ErrorReportSendFailure({
        type: 'server',
        failure: { type: 'refused', status: 403, detail: '{"error":"amend key does not match"}' },
      }),
    )
    const target = await mountDialog()
    const note = target.querySelector('textarea')
    if (!note) throw new Error('no note field')
    note.value = 'it happened while copying'
    note.dispatchEvent(new Event('input', { bubbles: true }))
    await tick()

    button(target, tString('errorReporter.amend.submit'))?.click()
    await settle()

    const message = lastToastMessage()
    expect(message).toContain('turned this down')
    expect(message).not.toContain('403')
    expect(message).not.toContain('amend key')
  })

  it('offers Try again for a preview that didn’t build, and keeps Send off until one does', async () => {
    openErrorReportDialog()
    api.prepareErrorReportPreview.mockRejectedValueOnce(new Error('build bundle: Permission denied (os error 13)'))
    const target = await mountDialog()

    expect(target.textContent).not.toContain('Permission denied')
    expect(button(target, tString('errorReporter.dialog.send'))?.disabled).toBe(true)
    const tryAgain = button(target, tString('errorReporter.dialog.tryAgain'))
    expect(tryAgain).toBeDefined()

    tryAgain?.click()
    await settle()

    expect(api.prepareErrorReportPreview).toHaveBeenCalledTimes(2)
    expect(target.textContent).toContain('ERR-AB23X')
    expect(button(target, tString('errorReporter.dialog.send'))?.disabled).toBe(false)
  })
})
