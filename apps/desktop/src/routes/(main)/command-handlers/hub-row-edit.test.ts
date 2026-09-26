/**
 * Edit and Rename in the Servers list are "Edit server…": `file.edit`, `file.rename`, and `servers.edit` all reach the
 * hub row under the cursor, whatever keys they're bound to, and fall through to their file-list meaning elsewhere.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'

const editHubRow = vi.fn((_row: unknown) => Promise.resolve())
const runServerRowAction = vi.fn((_payload: unknown) => Promise.resolve())
const openInEditorOrExplain = vi.fn(() => Promise.resolve('opened'))

vi.mock('$lib/file-explorer/network/servers-hub-actions', () => ({
  editHubRow: (row: unknown) => editHubRow(row),
}))
vi.mock('$lib/file-explorer/navigation/server-row-actions', () => ({
  runServerRowAction: (payload: unknown) => runServerRowAction(payload),
}))
vi.mock('$lib/file-explorer/pane/editor-open', () => ({ openInEditorOrExplain: () => openInEditorOrExplain() }))
vi.mock('$lib/file-explorer/pane/focused-pane-reads', () => ({
  getFocusedPanePath: () => '/Users/me',
  getFocusedPaneVolumeId: () => 'root',
}))
vi.mock('$lib/stores/volume-store.svelte', () => ({ getVolumes: () => [] }))
vi.mock('$lib/tauri-commands', () => ({ listSavedServers: () => Promise.resolve([]), trackEvent: vi.fn() }))

import { fileHandlers } from './file-handlers'
import { serversHandlers } from './servers-handlers'
import type { CommandHandlerContext } from './types'
import type { HubRow } from '$lib/file-explorer/network/servers-hub-rows'

const hubRow = { id: 'manual-nas', kind: 'server', name: 'Naspolya' } as unknown as HubRow

function hctx(row: HubRow | null) {
  const explorerRef = {
    getFocusedPaneHubRow: vi.fn(() => row),
    getFocusedPaneServerRow: vi.fn(() => null),
    startRename: vi.fn(),
    getFileAndPathUnderCursor: vi.fn(() => ({ path: '/Users/me/a.txt', filename: 'a.txt' })),
  }
  return { ctx: { explorerRef, ctx: {}, dispatchArgs: undefined } as unknown as CommandHandlerContext, explorerRef }
}

type Handler = (hctx: CommandHandlerContext) => unknown

beforeEach(() => {
  vi.clearAllMocks()
})

describe('on a Servers list row', () => {
  it.each([
    ['file.rename', fileHandlers['file.rename'] as Handler],
    ['file.edit', fileHandlers['file.edit'] as Handler],
    ['servers.edit', serversHandlers['servers.edit'] as Handler],
  ])('%s opens Edit server for the row under the cursor', async (_id, handler) => {
    const { ctx, explorerRef } = hctx(hubRow)
    await handler(ctx)
    expect(editHubRow).toHaveBeenCalledExactlyOnceWith(hubRow)
    expect(explorerRef.startRename).not.toHaveBeenCalled()
    expect(openInEditorOrExplain).not.toHaveBeenCalled()
  })
})

describe('anywhere else', () => {
  it('file.rename renames the file under the cursor', async () => {
    const { ctx, explorerRef } = hctx(null)
    await (fileHandlers['file.rename'] as Handler)(ctx)
    expect(explorerRef.startRename).toHaveBeenCalledOnce()
    expect(editHubRow).not.toHaveBeenCalled()
  })

  it('file.edit opens the file under the cursor in the editor', async () => {
    const { ctx } = hctx(null)
    await (fileHandlers['file.edit'] as Handler)(ctx)
    expect(openInEditorOrExplain).toHaveBeenCalledOnce()
    expect(editHubRow).not.toHaveBeenCalled()
  })
})
