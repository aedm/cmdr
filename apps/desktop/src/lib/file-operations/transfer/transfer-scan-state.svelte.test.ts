/**
 * Headless tests for `createTransferScanState`: a dialog that closes before its scan preview
 * is under way.
 *
 * The scan start is async: four listener registrations, then the `startScanPreview` IPC. The
 * transfer dialog can unmount partway through (an MCP `dialog confirm` takes it down at once),
 * and both its parents null their props object on close. The factory's getters read the
 * dialog's props, which are live getters into that object, so a read after unmount throws:
 * the `null is not an object (evaluating 't.transferDialogProps.sourcePaths')` rejection E2E
 * runs logged. `getSourcePaths` here throws the same way once the dialog is gone. Past the
 * throw, nothing the start leaves behind may outlive the dialog: a listener, or a preview
 * whose id only lands after teardown ran.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import * as commands from '$lib/tauri-commands'
import { createTransferScanState } from './transfer-scan-state.svelte'

vi.mock('$lib/tauri-commands', () => ({
  startScanPreview: vi.fn(),
  cancelScanPreview: vi.fn(() => Promise.resolve()),
  checkScanPreviewStatus: vi.fn(() => Promise.resolve(null)),
  onScanPreviewProgress: vi.fn(),
  onScanPreviewComplete: vi.fn(),
  onScanPreviewError: vi.fn(),
  onScanPreviewCancelled: vi.fn(),
}))

vi.mock('$lib/settings', () => ({
  getSetting: vi.fn(() => 500),
}))

interface Deferred<T> {
  promise: Promise<T>
  resolve: (value: T) => void
}

function deferred<T>(): Deferred<T> {
  let resolve: (value: T) => void = () => {}
  const promise = new Promise<T>((settle) => {
    resolve = settle
  })
  return { promise, resolve }
}

describe('createTransferScanState closed before its scan preview is under way', () => {
  let registrations: Deferred<undefined>
  let unlistens: Array<() => void>
  let destroyed: boolean
  let scan: ReturnType<typeof createTransferScanState>
  let destroyRoot: () => void

  beforeEach(() => {
    vi.clearAllMocks()
    registrations = deferred()
    unlistens = []
    destroyed = false
    for (const register of [
      commands.onScanPreviewProgress,
      commands.onScanPreviewComplete,
      commands.onScanPreviewError,
      commands.onScanPreviewCancelled,
    ]) {
      vi.mocked(register).mockImplementation(async () => {
        await registrations.promise
        const unlisten = vi.fn()
        unlistens.push(unlisten)
        return unlisten
      })
    }
    destroyRoot = $effect.root(() => {
      scan = createTransferScanState({
        getSourcePaths: () => {
          // The dialog's prop getter, once its parent has nulled the props object.
          if (destroyed) throw new TypeError("null is not an object (evaluating 'transferDialogProps.sourcePaths')")
          return ['/src/a.txt']
        },
        getSortColumn: () => 'name',
        getSortOrder: () => 'ascending',
        getSourceVolumeId: () => 'root',
        getIsSameVolumeMove: () => false,
        getConfirmed: () => false,
        getDestroyed: () => destroyed,
        getSampleForEstimate: () => false,
      })
    })
  })

  afterEach(() => {
    destroyRoot()
  })

  /** What `TransferDialog`'s `onDestroy` does for a dialog that wasn't confirmed. */
  function close(): void {
    destroyed = true
    scan.freeAndCleanup()
  }

  it('reads no props and keeps no listener when it closes while its listeners are still registering', async () => {
    scan.start()

    close()
    registrations.resolve(undefined)

    await expect(scan.scanStarted).resolves.toBeUndefined()
    expect(commands.startScanPreview).not.toHaveBeenCalled()
    expect(unlistens.length).toBeGreaterThan(0)
    for (const unlisten of unlistens) expect(unlisten).toHaveBeenCalledOnce()
  })

  it('frees a preview whose id only lands after the dialog closed', async () => {
    const preview = deferred<{ previewId: string }>()
    vi.mocked(commands.startScanPreview).mockReturnValue(preview.promise)
    registrations.resolve(undefined)
    scan.start()
    await vi.waitFor(() => {
      expect(commands.startScanPreview).toHaveBeenCalledWith(['/src/a.txt'], 'name', 'ascending', 500, 'root', false)
    })

    close()
    preview.resolve({ previewId: 'preview-late' })

    await expect(scan.scanStarted).resolves.toBeUndefined()
    expect(commands.cancelScanPreview).toHaveBeenCalledWith('preview-late')
    expect(commands.checkScanPreviewStatus).not.toHaveBeenCalled()
  })
})
