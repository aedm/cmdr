/**
 * Tests for the kept-leftovers notice bridge: what the person is told when a
 * drive comes back holding the working folder of a move that never finished.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'
import type { MoveLeftoversKeptEvent } from '$lib/ipc/bindings'

/** Only the fields these cells assert on; the real options type is wider. */
type ToastOptions = { level: string; timeoutMs: number; id: string }

const addToast = vi.hoisted(() => vi.fn<(message: string, options: ToastOptions) => string>())
vi.mock('$lib/ui/toast', () => ({ addToast }))

let emitKept: (payload: MoveLeftoversKeptEvent) => void
const unlisten = vi.fn()
vi.mock('$lib/tauri-commands', () => ({
  onMoveLeftoversKept: (handler: (payload: MoveLeftoversKeptEvent) => void) => {
    emitKept = handler
    return Promise.resolve(unlisten)
  },
}))

import { startLeftoverNoticeBridge } from './leftover-notice-bridge'

const FOLDER = '.cmdr-staging-0199a2c4-7b31-7c2a-9f10-4e1d8c6b2a55'

beforeEach(async () => {
  addToast.mockClear()
  unlisten.mockClear()
  await startLeftoverNoticeBridge()
})

describe('the kept-leftovers notice', () => {
  it('names the drive and the folder, so the files have somewhere findable', () => {
    emitKept({ volumeName: 'Fältkamera', folderName: FOLDER })

    expect(addToast).toHaveBeenCalledTimes(1)
    const [message] = addToast.mock.calls[0]
    expect(message).toContain('Fältkamera')
    expect(message).toContain(FOLDER)
  })

  /// Nothing is asked of the person and nothing of theirs is at risk, so it
  /// reads as information and goes away on its own.
  it('is an info toast that times out', () => {
    emitKept({ volumeName: 'Fältkamera', folderName: FOLDER })

    const [, options] = addToast.mock.calls[0]
    expect(options.level).toBe('info')
    expect(options.timeoutMs).toBeGreaterThan(0)
  })

  it('keys per folder, so two folders on one drive each get their own notice', () => {
    const second = '.cmdr-staging-0199a2c4-7b31-7c2a-9f10-4e1d8c6b2a56'
    emitKept({ volumeName: 'Fältkamera', folderName: FOLDER })
    emitKept({ volumeName: 'Fältkamera', folderName: second })

    const ids = addToast.mock.calls.map(([, options]) => options.id)
    expect(new Set(ids).size).toBe(2)
  })

  /// A re-plug meets the same folder again. One notice replacing itself is the
  /// honest reading; two stacked copies of the same sentence is not.
  it('reuses one id for the same folder, so a re-plug replaces its notice', () => {
    emitKept({ volumeName: 'Fältkamera', folderName: FOLDER })
    emitKept({ volumeName: 'Fältkamera', folderName: FOLDER })

    const ids = addToast.mock.calls.map(([, options]) => options.id)
    expect(ids[0]).toBe(ids[1])
  })

  it('hands back the unsubscribe it was given', async () => {
    const stop = await startLeftoverNoticeBridge()
    stop()
    expect(unlisten).toHaveBeenCalled()
  })
})
