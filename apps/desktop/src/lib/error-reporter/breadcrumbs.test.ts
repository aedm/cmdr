/**
 * Tests for the FE breadcrumb wrapper.
 *
 * The wrapper is intentionally thin: it forwards the generated closed event type and
 * swallows errors. The Rust tests own rejection of extra and nested JSON fields.
 */

import { describe, it, vi, expect, beforeEach } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { recordBreadcrumb } from './breadcrumbs'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(() => Promise.resolve()),
}))

const mockedInvoke = vi.mocked(invoke)

describe('recordBreadcrumb', () => {
  beforeEach(() => {
    mockedInvoke.mockClear()
    mockedInvoke.mockImplementation(() => Promise.resolve())
  })

  it('forwards retained command identity and boolean facts', () => {
    recordBreadcrumb({ type: 'command', commandId: 'pane.switch' })
    recordBreadcrumb({ type: 'errorReportDialogOpened', hasInitialNote: true })

    expect(mockedInvoke).toHaveBeenNthCalledWith(1, 'record_breadcrumb', {
      event: { type: 'command', commandId: 'pane.switch' },
    })
    expect(mockedInvoke).toHaveBeenNthCalledWith(2, 'record_breadcrumb', {
      event: { type: 'errorReportDialogOpened', hasInitialNote: true },
    })
  })

  it('swallows errors so breadcrumb failures never break the UI', async () => {
    mockedInvoke.mockRejectedValueOnce(new Error('IPC unavailable'))
    expect(() => {
      recordBreadcrumb({ type: 'feedbackDialogClosed' })
    }).not.toThrow()
    // Let the rejection settle on the microtask queue so coverage sees the catch branch.
    await Promise.resolve()
    expect(mockedInvoke).toHaveBeenCalledOnce()
  })
})
