/**
 * `DeleteDialog.svelte`: the banner a trash routed into a permanent delete shows,
 * and the live flip a folder's scan walk can trigger.
 *
 * Online-only content in a cloud-storage folder can't go to the Trash without
 * being downloaded first, so F8 opens THIS dialog instead of the trash one. The
 * person asked for a trash and got a delete, so the banner has to say why, and
 * the switch that would send them back into that download is gone.
 *
 * A selected FOLDER can't carry the flag itself, so its answer arrives mid-scan.
 * These pin that the dialog flips itself when it lands, and that a confirm
 * pressed before it lands doesn't settle the question by luck.
 */

import { describe, expect, it, vi } from 'vitest'
import { mount, tick } from 'svelte'
import DeleteDialog from './DeleteDialog.svelte'
import type { ScanPreviewCompleteEvent, ScanPreviewProgressEvent } from '$lib/ipc/bindings'

/** Scan-event listeners the mounted dialog registered, so a test can drive the walk. */
const listeners = {
  progress: [] as ((event: ScanPreviewProgressEvent) => void)[],
  complete: [] as ((event: ScanPreviewCompleteEvent) => void)[],
}

vi.mock('$lib/tauri-commands', () => ({
  notifyDialogOpened: vi.fn(() => Promise.resolve()),
  notifyDialogClosed: vi.fn(() => Promise.resolve()),
  startScanPreview: vi.fn(() => Promise.resolve({ previewId: 'preview-1' })),
  cancelScanPreview: vi.fn(() => Promise.resolve()),
  onScanPreviewProgress: vi.fn((handler: (event: ScanPreviewProgressEvent) => void) => {
    listeners.progress.push(handler)
    return Promise.resolve(() => {})
  }),
  onScanPreviewComplete: vi.fn((handler: (event: ScanPreviewCompleteEvent) => void) => {
    listeners.complete.push(handler)
    return Promise.resolve(() => {})
  }),
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
const ONLINE_ONLY_BANNER = 'This is online-only content.'

function mountDialog(overrides: Record<string, unknown>): {
  target: HTMLElement
  onConfirm: ReturnType<typeof vi.fn>
} {
  listeners.progress = []
  listeners.complete = []
  const target = document.createElement('div')
  document.body.appendChild(target)
  const onConfirm = vi.fn()
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
      onConfirm,
      onCancel: () => {},
      ...overrides,
    },
  })
  return { target, onConfirm }
}

/** Lets the dialog's `onMount` finish registering its scan listeners. */
async function settle(): Promise<void> {
  for (let i = 0; i < 6; i++) await tick()
}

function bannerText(target: HTMLElement): string {
  return target.querySelector('#delete-warning-text')?.textContent ?? ''
}

function confirmButton(target: HTMLElement): HTMLButtonElement {
  const buttons = [...target.querySelectorAll('button')]
  const button = buttons.at(-1)
  if (!button) throw new Error('the dialog rendered no footer buttons')
  return button
}

function completion(onlineOnlyFound: boolean): ScanPreviewCompleteEvent {
  return {
    previewId: 'preview-1',
    filesTotal: 3,
    dirsTotal: 1,
    bytesTotal: 300,
    dedupBytesTotal: 300,
    estimatedCompressedBytes: null,
    onlineOnlyFound,
  }
}

describe('DeleteDialog over online-only cloud content', () => {
  it('explains the swap, and offers no way back to the trash', async () => {
    const { target } = mountDialog({ cloudStorageOnlineOnly: true })
    await tick()

    expect(bannerText(target)).toContain(ONLINE_ONLY_BANNER)
    expect(bannerText(target)).toContain("There'll be no copy in the Trash")
    expect(bannerText(target)).toContain('the service keeps its own history you can restore from')
    expect(target.querySelector('[role="switch"]')).toBeNull()
  })

  /** The generic banner is about a volume with no trash (FAT32, SMB); a cloud
   *  folder is on the boot volume, so the two must not be confused. */
  it('leaves the generic no-trash banner to the volumes it describes', async () => {
    const { target } = mountDialog({})
    await tick()

    expect(bannerText(target)).toContain("This volume doesn't support trash.")
    expect(bannerText(target)).not.toContain('online-only')
  })

  /** A selected folder opens as an ordinary trash. The walk is the only thing
   *  that can see an evicted file inside it, so the dialog has to flip itself
   *  when one turns up, before anything is pressed. */
  it('flips to the permanent delete when the walk finds an online-only file', async () => {
    const { target } = mountDialog({
      sourceItems: [{ name: 'Work', isDirectory: true, isSymlink: false }],
      sourcePaths: [FOLDER],
      isPermanent: false,
      supportsTrash: true,
      cloudFolderMayHoldOnlineOnly: true,
    })
    await settle()

    expect(bannerText(target)).toBe('')
    expect(target.querySelector('[role="switch"]')).not.toBeNull()

    for (const handler of listeners.complete) handler(completion(true))
    await tick()

    expect(bannerText(target)).toContain(ONLINE_ONLY_BANNER)
    expect(target.querySelector('[role="switch"]')).toBeNull()
  })

  /** A fully materialized folder stays exactly as it is today. */
  it('stays a trash when the walk finds nothing evicted', async () => {
    const { target, onConfirm } = mountDialog({
      sourceItems: [{ name: 'Work', isDirectory: true, isSymlink: false }],
      sourcePaths: [FOLDER],
      isPermanent: false,
      supportsTrash: true,
      cloudFolderMayHoldOnlineOnly: true,
    })
    await settle()

    for (const handler of listeners.complete) handler(completion(false))
    await tick()

    expect(bannerText(target)).toBe('')
    confirmButton(target).click()
    await settle()
    expect(onConfirm).toHaveBeenCalledWith('preview-1', false)
  })

  /** The race the whole wait exists for. Pressing "Move to trash" while the walk
   *  is still counting must not run a trash the answer is about to rule out. */
  it('holds a confirm until the walk answers, then hands the dialog back when it says online-only', async () => {
    const { target, onConfirm } = mountDialog({
      sourceItems: [{ name: 'Work', isDirectory: true, isSymlink: false }],
      sourcePaths: [FOLDER],
      isPermanent: false,
      supportsTrash: true,
      cloudFolderMayHoldOnlineOnly: true,
    })
    await settle()

    confirmButton(target).click()
    await settle()
    expect(onConfirm).not.toHaveBeenCalled()

    // One hit is the whole answer, so a progress tick releases the wait: a folder
    // holding 50 GB of evicted content needn't finish counting first.
    for (const handler of listeners.progress) {
      handler({
        previewId: 'preview-1',
        filesFound: 1,
        dirsFound: 0,
        bytesFound: 1,
        currentPath: null,
        currentDir: null,
        expectedFilesTotal: null,
        expectedBytesTotal: null,
        onlineOnlyFound: true,
      })
    }
    await settle()

    expect(onConfirm).not.toHaveBeenCalled()
    expect(bannerText(target)).toContain(ONLINE_ONLY_BANNER)

    // And the second press, now over a dialog that says what it will do, goes.
    confirmButton(target).click()
    await settle()
    expect(onConfirm).toHaveBeenCalledWith('preview-1', true)
  })
})
