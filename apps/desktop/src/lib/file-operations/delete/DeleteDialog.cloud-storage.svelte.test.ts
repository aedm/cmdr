/**
 * `DeleteDialog.svelte`: the banner a trash routed into a permanent delete shows.
 *
 * A cloud-storage folder's File Provider has no trash, so F8 opens THIS dialog
 * instead of the trash one. The person asked for a trash and got a delete, so the
 * banner has to say why, and the switch that would send them back into the same
 * refusal is gone.
 */

import { describe, expect, it, vi } from 'vitest'
import { mount, tick } from 'svelte'
import DeleteDialog from './DeleteDialog.svelte'

vi.mock('$lib/tauri-commands', () => ({
  notifyDialogOpened: vi.fn(() => Promise.resolve()),
  notifyDialogClosed: vi.fn(() => Promise.resolve()),
  startScanPreview: vi.fn(() => Promise.resolve({ previewId: 'preview-1' })),
  cancelScanPreview: vi.fn(() => Promise.resolve()),
  onScanPreviewProgress: vi.fn(() => Promise.resolve(() => {})),
  onScanPreviewComplete: vi.fn(() => Promise.resolve(() => {})),
  onScanPreviewError: vi.fn(() => Promise.resolve(() => {})),
  onScanPreviewCancelled: vi.fn(() => Promise.resolve(() => {})),
}))

vi.mock('$lib/settings', () => ({
  getSetting: vi.fn(() => 500),
}))

vi.mock('$lib/settings/reactive-settings.svelte', () => ({
  formatFileSize: vi.fn((n: number | undefined) => (n === undefined ? '' : `${String(n)} B`)),
  getFileSizeFormat: vi.fn(() => 'binary'),
  getFileSizeUnit: vi.fn(() => 'bytes'),
}))

const FOLDER = '/Users/me/Library/CloudStorage/Dropbox/Work'

function mountDialog(overrides: Record<string, unknown>): HTMLElement {
  const target = document.createElement('div')
  document.body.appendChild(target)
  mount(DeleteDialog, {
    target,
    props: {
      sourceItems: [{ name: 'emclient.pkg', isDirectory: false, isSymlink: false, size: 120 }],
      sourcePaths: [`${FOLDER}/emclient.pkg`],
      sourceFolderPath: FOLDER,
      isPermanent: true,
      supportsTrash: false,
      isFromCursor: true,
      sortColumn: 'name',
      sortOrder: 'ascending',
      sourceVolumeId: 'root',
      onConfirm: () => {},
      onCancel: () => {},
      ...overrides,
    },
  })
  return target
}

describe('DeleteDialog in a cloud-storage folder', () => {
  it('explains the swap, and offers no way back to the trash', async () => {
    const target = mountDialog({ cloudStorageWithoutTrash: true })
    await tick()

    const banner = target.querySelector('#delete-warning-text')
    expect(banner?.textContent).toContain('This folder syncs to a cloud service that has no trash.')
    expect(banner?.textContent).toContain('the service keeps its own copy you can restore from')
    expect(target.querySelector('[role="switch"]')).toBeNull()
  })

  /** The generic banner is about a volume with no trash (FAT32, SMB); a cloud
   *  folder is on the boot volume, so the two must not be confused. */
  it('leaves the generic no-trash banner to the volumes it describes', async () => {
    const target = mountDialog({})
    await tick()

    const banner = target.querySelector('#delete-warning-text')
    expect(banner?.textContent).toContain("This volume doesn't support trash.")
    expect(banner?.textContent).not.toContain('cloud service')
  })
})
