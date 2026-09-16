/**
 * Unit tests for the switcher's shared index-badge layer: one instance serves the breadcrumb
 * chip and every switcher row, so what a picked menu action DOES is pinned once here.
 *
 * What a dot MEANS is pure and tested elsewhere (`drive-index-status.test.ts`,
 * `image-index-drive-state.test.ts`); what this owns is the routing: which IPC a pick calls,
 * which answers earn a toast, and that `credentials_needed` goes to the connect flow rather
 * than to a message the user can't act on.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import type { EnableIndexingOutcome } from '$lib/ipc/bindings'

/** What `enableDriveIndex` / `rescanDriveIndex` answer: the typed outcome, or a transport break. */
type IndexingAnswer = Promise<{ status: 'ok'; data: EnableIndexingOutcome } | { status: 'error'; error: string }>

const enableDriveIndex = vi.fn<(volumeId: string) => IndexingAnswer>()
const rescanDriveIndex = vi.fn<(volumeId: string) => IndexingAnswer>()
const disableDriveIndex = vi.fn<(volumeId: string) => Promise<void>>(() => Promise.resolve())
const forgetDriveIndex = vi.fn<(volumeId: string) => Promise<void>>(() => Promise.resolve())
const mediaIndexVolumeState = vi.fn((_volumeId: string) => Promise.resolve({ enabled: true, qualifyingCount: 3 }))
const addToast = vi.fn((_message: string, _options: { level: string }) => 'toast-id')
const connectDirectly = vi.fn((_args: { volumeId: string; shareName: string }) =>
  Promise.resolve({ kind: 'connected' }),
)

vi.mock('$lib/tauri-commands', () => ({
  enableDriveIndex: (volumeId: string) => enableDriveIndex(volumeId),
  rescanDriveIndex: (volumeId: string) => rescanDriveIndex(volumeId),
  disableDriveIndex: (volumeId: string) => disableDriveIndex(volumeId),
  forgetDriveIndex: (volumeId: string) => forgetDriveIndex(volumeId),
  mediaIndexVolumeState: (volumeId: string) => mediaIndexVolumeState(volumeId),
}))

vi.mock('$lib/tauri-commands/indexing', () => ({
  getVolumeIndexStatusById: () => Promise.resolve({ status: 'ok', data: { volumeId: 'volumes-backup' } }),
  onIndexFreshnessChanged: () => Promise.resolve(() => {}),
  onIndexScanStarted: () => Promise.resolve(() => {}),
  onIndexScanComplete: () => Promise.resolve(() => {}),
}))

vi.mock('$lib/ui/toast', () => ({
  addToast: (message: string, options: { level: string }) => addToast(message, options),
}))

vi.mock('../network/direct-connect', () => ({
  connectDirectly: (args: { volumeId: string; shareName: string }) => connectDirectly(args),
}))

import { createDriveBadges } from './drive-badges.svelte'
import type { VolumeInfo } from '../types'

const drive: VolumeInfo = {
  id: 'volumes-backup',
  name: 'Backup',
  path: '/Volumes/Backup',
  category: 'attached_volume',
  isEjectable: true,
}

function ok(outcome: EnableIndexingOutcome): IndexingAnswer {
  return Promise.resolve({ status: 'ok' as const, data: outcome })
}

describe('drive-badges', () => {
  let dispose: (() => void) | undefined

  function create() {
    let badges!: ReturnType<typeof createDriveBadges>
    dispose = $effect.root(() => {
      badges = createDriveBadges({ getVolumes: () => [drive], getActiveVolume: () => undefined })
    })
    return badges
  }

  beforeEach(() => {
    vi.clearAllMocks()
  })

  afterEach(() => {
    dispose?.()
    dispose = undefined
  })

  it('a started scan says nothing: the dot going blue is the feedback', async () => {
    enableDriveIndex.mockReturnValueOnce(ok({ status: 'started' }))
    const badges = create()
    badges.runAction(drive.id, 'enable')
    await vi.waitFor(() => {
      expect(enableDriveIndex).toHaveBeenCalledWith(drive.id)
    })
    expect(addToast).not.toHaveBeenCalled()
  })

  it('a deferred rescan promises it in a toast, so the button never looks dead', async () => {
    rescanDriveIndex.mockReturnValueOnce(ok({ status: 'deferred_until_search_ends' }))
    const badges = create()
    badges.runAction(drive.id, 'rescan')
    await vi.waitFor(() => {
      expect(addToast).toHaveBeenCalled()
    })
    expect(addToast.mock.calls[0][1]).toEqual({ level: 'info' })
  })

  // ❗ Not a toast: a missing credential is fixed by signing in, which the connect flow does.
  it('a credentials refusal goes to the connect flow, not to a message', async () => {
    enableDriveIndex.mockReturnValueOnce(ok({ status: 'refused', reason: 'credentials_needed' }))
    const badges = create()
    badges.runAction(drive.id, 'enable')
    await vi.waitFor(() => {
      expect(connectDirectly).toHaveBeenCalledWith({ volumeId: drive.id, shareName: 'Backup' })
    })
    expect(addToast).not.toHaveBeenCalled()
  })

  it('stop and disable both turn the drive off; forget deletes its index', async () => {
    const badges = create()
    badges.runAction(drive.id, 'stop')
    badges.runAction(drive.id, 'disable')
    badges.runAction(drive.id, 'forget')
    await vi.waitFor(() => {
      expect(forgetDriveIndex).toHaveBeenCalledWith(drive.id)
    })
    expect(disableDriveIndex).toHaveBeenCalledTimes(2)
  })

  // ⚠️ A Rust `Err(String)` arrives as a VALUE, not a throw, which is what the pure
  // `driveIndexActionFeedback` answers; a real transport break is what lands here.
  it('a broken call still says something', async () => {
    enableDriveIndex.mockReturnValueOnce(Promise.reject(new Error('nope')))
    const badges = create()
    badges.runAction(drive.id, 'enable')
    await vi.waitFor(() => {
      expect(addToast).toHaveBeenCalled()
    })
    expect(addToast.mock.calls[0][1]).toEqual({ level: 'error' })
  })

  it('fills both maps for the rows a just-opened switcher shows, and skips non-drives', async () => {
    const favorite: VolumeInfo = {
      id: 'fav-1',
      name: 'Documents',
      path: '/Users/test/Documents',
      category: 'favorite',
      isEjectable: false,
    }
    const badges = create()
    badges.fetchForRows([drive, favorite])
    await vi.waitFor(() => {
      expect(badges.imageStateFor(drive.id)).not.toBeUndefined()
    })
    // A favorite is a shortcut into a drive, not a drive: no badge, and no fetch for it.
    expect(badges.imageStateFor(favorite.id)).toBeUndefined()
    expect(mediaIndexVolumeState).toHaveBeenCalledTimes(1)
  })
})
