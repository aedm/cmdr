/**
 * The real-operation preview's guarantees:
 *
 * - it starts a REAL copy through the app's own dispatch, under "Ask for each",
 *   so the prompt that follows is raised by a parked operation the way a
 *   queued copy's is;
 * - each state copies the clash its label promises, from the fixture tree the
 *   backend created (never a path the frontend made up);
 * - with no fixture tree, or a copy the backend refuses, it opens nothing and
 *   says so.
 */

import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { TransferDispatchConfig } from '$lib/file-operations/transfer/transfer-dispatch'
import type { FixtureDirPayload } from './disk-fixture'

const dispatched: TransferDispatchConfig[] = []
let refuse = false
const toasts: string[] = []

vi.mock('$lib/file-operations/transfer/transfer-dispatch', () => ({
  dispatchTransferOperation: (config: TransferDispatchConfig) => {
    dispatched.push(config)
    return refuse ? Promise.reject(new Error('refused')) : Promise.resolve({ operationId: 'op-preview' })
  },
}))

vi.mock('$lib/ui/toast/toast-store.svelte', () => ({
  addToast: (content: string) => {
    toasts.push(content)
    return 'toast-id'
  },
}))

vi.mock('$lib/logging/logger', () => ({
  getAppLogger: () => ({ warn: vi.fn(), info: vi.fn(), error: vi.fn(), debug: vi.fn() }),
}))

import { closeGalleryDialog, getOpenGalleryDialog, openGalleryDialog } from './gallery-state.svelte'
import { openOperationConflictPreview } from './operation-conflict-preview'

const FIXTURES: FixtureDirPayload = {
  root: '/data/dialog-gallery-fixtures',
  destinationDir: '/data/dialog-gallery-fixtures/Backup destination',
  existingFolderName: 'Photos',
  existingFileName: 'Invoice 2026-07.pdf',
  nestedPath: '/data/dialog-gallery-fixtures/Projects',
  conflictPreview: {
    fromDir: '/data/dialog-gallery-conflict-fixtures/From',
    toDir: '/data/dialog-gallery-conflict-fixtures/To',
    folderOverFile: 'Website redesign',
    fileOverFolder: 'Quarterly report.pdf',
    fileOverFile: 'Budget 2026.xlsx',
  },
}

beforeEach(() => {
  dispatched.length = 0
  toasts.length = 0
  refuse = false
  closeGalleryDialog()
})

describe('openOperationConflictPreview', () => {
  it.each([
    ['folder-over-file', 'Website redesign'],
    ['file-over-folder', 'Quarterly report.pdf'],
    ['file-over-file', 'Budget 2026.xlsx'],
  ])('%s starts a real "Ask for each" copy of %s into the clash folder', async (stateId, name) => {
    const outcome = await openOperationConflictPreview(stateId, FIXTURES)

    expect(outcome).toEqual({ kind: 'started', operationId: 'op-preview' })
    expect(dispatched).toHaveLength(1)
    expect(dispatched[0]).toMatchObject({
      operationType: 'copy',
      sourcePaths: [`/data/dialog-gallery-conflict-fixtures/From/${name}`],
      destinationPath: '/data/dialog-gallery-conflict-fixtures/To',
      conflictResolution: 'stop',
    })
  })

  it('closes whatever the gallery had up, so the prompt is not stacked on a stale preview', async () => {
    openGalleryDialog('alert', 'short')

    await openOperationConflictPreview('folder-over-file', FIXTURES)

    expect(getOpenGalleryDialog()).toBeNull()
  })

  it('starts nothing without the fixture tree', async () => {
    expect(await openOperationConflictPreview('folder-over-file', null)).toEqual({ kind: 'no-fixtures' })
    expect(dispatched).toHaveLength(0)
  })

  it('starts nothing for a state the row does not advertise', async () => {
    expect(await openOperationConflictPreview('no-such-state', FIXTURES)).toEqual({ kind: 'unknown-state' })
    expect(dispatched).toHaveLength(0)
  })

  it('says so when the backend refuses the copy', async () => {
    refuse = true

    expect(await openOperationConflictPreview('file-over-file', FIXTURES)).toEqual({ kind: 'refused' })
    expect(toasts).toHaveLength(1)
  })
})
