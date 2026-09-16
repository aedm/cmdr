/**
 * Behavioral tests for the volume switcher: the chip (`VolumeBreadcrumb.svelte`) plus the
 * list it opens (`VolumeChooserMenu.svelte`, the house `Menu`). They're the behavior
 * contract the M2 port had to keep, so they mount the chip and drive it the way a person
 * does — real keydowns, real clicks.
 *
 * One of them is the favorite-rename keyboard guard (Fix E): while a favorite is being
 * renamed inline, the menu must NOT consume arrow / Home / End keys, so the textbox keeps
 * them. The cross-pane suppression itself lives in `pane/key-dispatch.ts`; here we pin the
 * leaf guard.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, tick, flushSync } from 'svelte'
import VolumeBreadcrumb from './VolumeBreadcrumb.svelte'
import type { VolumeChangePayload } from '../pane/types'

const reorderFavorites = vi.fn(() => Promise.resolve())
const ejectVolume = vi.fn(() => Promise.resolve())
const disconnectPlace = vi.fn(() => Promise.resolve(true))
const showVolumeRowContextMenu = vi.fn(() => Promise.resolve())
const hasServerSecret = vi.fn(() => Promise.resolve(true))
const listSavedServers = vi.fn(() =>
  Promise.resolve([{ id: 'sftp-nas-local-22-ada', places: [{ volumeId: 'sftp-nas-local-22-ada' }] }]),
)

/**
 * The volume list the store mock answers with. Swappable so the server-row block
 * can put a place in the switcher without shifting the favorite indices every
 * other block here counts on.
 */
const stubs = vi.hoisted(() => ({
  volumes: null as unknown[] | null,
  ejecting: new Set<string>(),
  /**
   * The per-drive index status the freshness badge renders from. `null` means the
   * fetch answers "nothing to show", so no row grows a badge — which is what every
   * block here except the drive-index one wants.
   */
  indexStatus: null as Record<string, unknown> | null,
}))

const connectDirectly = vi.fn(() => Promise.resolve({ kind: 'connected' }))

// The submenu's one action. The component is the only importer here, so a factory
// mock costs nothing else.
vi.mock('../network/direct-connect', () => ({
  connectDirectly: (...args: unknown[]) => connectDirectly(...(args as [])),
}))

// The drive-index badge's own IPC module. `drive-index-manager.svelte.ts` imports this
// SUB-PATH, not the `$lib/tauri-commands` barrel mocked below, so it needs its own mock.
vi.mock('$lib/tauri-commands/indexing', () => ({
  getVolumeIndexStatusById: () =>
    Promise.resolve(stubs.indexStatus ? { status: 'ok', data: stubs.indexStatus } : { status: 'timedOut' }),
  onIndexFreshnessChanged: () => Promise.resolve(() => {}),
  onIndexScanStarted: () => Promise.resolve(() => {}),
  onIndexScanComplete: () => Promise.resolve(() => {}),
}))

// The test DOM has no layout, so no scroll: a stub keeps `scrollHighlightedIntoView`
// from throwing into a floating promise, and lets a test watch it.
Element.prototype.scrollIntoView = vi.fn()

// Captures the `volume-context-action` listener the component registers in `onMount`, so a
// test can fire a native row-menu pick (Rename / Remove) the same way the backend would.
let volumeContextActionHandler: ((payload: { action: string; volumeId: string }) => void) | undefined

vi.mock('$lib/tauri-commands', () => ({
  resolvePathVolume: vi.fn(() => Promise.resolve({ volume: { id: 'root', path: '/' } })),
  upgradeToSmbVolume: vi.fn(() => Promise.resolve({ status: 'success' })),
  ejectVolume: (...args: unknown[]) => ejectVolume(...(args as [])),
  getVolumeSpace: vi.fn(() => Promise.resolve(null)),
  systemHasSavedSmbPassword: vi.fn(() => Promise.resolve(false)),
  upgradeToSmbVolumeUsingSavedPassword: vi.fn(() => Promise.resolve({ status: 'success' })),
  removeFavorite: vi.fn(() => Promise.resolve()),
  renameFavorite: vi.fn(() => Promise.resolve()),
  reorderFavorites: (...args: unknown[]) => reorderFavorites(...(args as [])),
  stripFavoritePrefix: (id: string) => (id.startsWith('fav-') ? id.slice(4) : id),
  showVolumeRowContextMenu: (...args: unknown[]) => showVolumeRowContextMenu(...(args as [])),
  disconnectPlace: (...args: unknown[]) => disconnectPlace(...(args as [])),
  forgetServer: vi.fn(() => Promise.resolve(true)),
  forgetServerSecret: vi.fn(() => Promise.resolve(true)),
  hasServerSecret: (...args: unknown[]) => hasServerSecret(...(args as [])),
  listSavedServers: () => listSavedServers(),
  onVolumeContextAction: (cb: (payload: { action: string; volumeId: string }) => void) => {
    volumeContextActionHandler = cb
    return Promise.resolve(() => {})
  },
}))

vi.mock('$lib/stores/volume-store.svelte', () => ({
  getVolumes: () =>
    stubs.volumes ?? [
      { id: 'fav-1', name: 'Documents', path: '/Users/test/Documents', category: 'favorite', isEjectable: false },
      { id: 'fav-2', name: 'Downloads', path: '/Users/test/Downloads', category: 'favorite', isEjectable: false },
      { id: 'fav-3', name: 'Projects', path: '/Users/test/Projects', category: 'favorite', isEjectable: false },
      { id: 'root', name: 'Macintosh HD', path: '/', category: 'main_volume', isEjectable: false },
    ],
  getVolumesTimedOut: () => false,
  isVolumesRefreshing: () => false,
  isVolumeRetryFailed: () => false,
  requestVolumeRefresh: vi.fn(),
}))

vi.mock('$lib/stores/volume-busy-store.svelte', () => ({
  isVolumeBusy: () => false,
  isVolumeEjecting: (id: string) => stubs.ejecting.has(id),
}))

vi.mock('$lib/ui/toast', () => ({ addToast: vi.fn(() => 'toast-id'), dismissToast: vi.fn() }))

vi.mock('$lib/settings/reactive-settings.svelte', () => ({
  formatFileSize: (n: number) => `${String(n)} B`,
  getFileSizeFormat: () => 'binary',
  getFileSizeUnit: () => 'bytes',
  getNetworkEnabled: () => true,
  getUseAppIconsForDocuments: () => false,
  // The drive-index badge's master switch. On, so a row carrying a status renders
  // the badge with its actions rather than the "indexing is off" note.
  getDriveIndexingEnabled: () => true,
}))

vi.mock('$lib/icon-cache', async () => {
  const { writable } = await import('svelte/store')
  return {
    getCachedIcon: vi.fn().mockReturnValue('/icons/dir.png'),
    getCachedCustomFolderIcon: () => undefined,
    iconCacheVersion: writable(0),
    prefetchIcons: vi.fn().mockResolvedValue(undefined),
  }
})

interface BreadcrumbInstance {
  open: () => void
  close: () => void
  toggle: () => void
  getIsOpen: () => boolean
}

// ============================================================================
// The switcher is the house `Menu`: it PORTALS to `document.body`, carries the documented
// `data-*` hooks (`$lib/ui/DETAILS.md` § Menu), and catches keys on its own document capture
// listener. So these look at the document rather than the mount target, select on hooks
// rather than on classes, and press keys for real.
// ============================================================================

/** The open switcher's surface, or null when none is open. */
function menuSurface(): HTMLElement | null {
  return document.querySelector('[data-menu]')
}

/** The main list's rows. The submenu is its own surface, so its row never lands here. */
function menuRows(): NodeListOf<HTMLElement> {
  return document.querySelectorAll('[data-menu] [data-menu-row]')
}

function isHighlighted(row: Element | null | undefined): boolean {
  return row?.hasAttribute('data-highlighted') ?? false
}

/**
 * Press a key the way the app does. Returns true when the menu claimed it, which is the
 * answer the switcher's own `handleKeyDown` gave before the primitive took the keyboard.
 */
function press(key: string, modifiers: KeyboardEventInit = {}): boolean {
  return !document.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...modifiers }))
}

// A switcher left open outlives its target div: it's portaled, and its key listener lives on
// the document. Escape closes whichever ones are open before the next test mounts — twice,
// because the first one only backs out of an open submenu.
afterEach(() => {
  press('Escape')
  press('Escape')
  document.body.innerHTML = ''
})

interface BreadcrumbProps {
  volumeId?: string
  currentPath?: string
  onVolumeChange?: (change: VolumeChangePayload) => void
}

function mountBreadcrumb(props: BreadcrumbProps = {}): { instance: BreadcrumbInstance; target: HTMLDivElement } {
  const target = document.createElement('div')
  document.body.appendChild(target)
  const instance = mount(VolumeBreadcrumb, {
    target,
    props: { volumeId: 'root', currentPath: '/Users/test', ...props },
  }) as unknown as BreadcrumbInstance
  // Settle the chip's `bind:this`: the menu hangs under that element, so `open()` called
  // before the binding lands would have nothing to anchor to.
  flushSync()
  return { instance, target }
}

/** Opens the switcher over `rows` and hands back the mounted pieces. */
async function openWithRows(
  rows: unknown[],
  props: BreadcrumbProps = {},
): Promise<{ instance: BreadcrumbInstance; target: HTMLDivElement }> {
  stubs.volumes = rows
  const mounted = mountBreadcrumb(props)
  mounted.instance.open()
  await tick()
  flushSync()
  return mounted
}

describe('VolumeBreadcrumb favorite keyboard reorder (Alt+Up / Alt+Down)', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
    reorderFavorites.mockClear()
  })

  it('Alt+ArrowDown on the highlighted favorite persists the moved order via reorderFavorites', async () => {
    const { instance } = mountBreadcrumb()
    instance.open()
    await tick()
    flushSync()

    // Highlight the first favorite (fav-1). Home jumps the virtual highlight to index 0.
    expect(press('Home')).toBe(true)
    await tick()
    flushSync()

    // Alt+ArrowDown moves fav-1 down one slot: ['2', '1', '3'] (bare ids).
    expect(press('ArrowDown', { altKey: true })).toBe(true)
    await tick()
    flushSync()

    expect(reorderFavorites).toHaveBeenCalledTimes(1)
    expect(reorderFavorites).toHaveBeenCalledWith(['2', '1', '3'])
  })

  it('two quick Alt+ArrowDown presses keep moving the SAME favorite (optimistic local order, no stale-state race)', async () => {
    const { instance } = mountBreadcrumb()
    instance.open()
    await tick()
    flushSync()

    expect(press('Home')).toBe(true)
    await tick()
    flushSync()

    // First press: fav-1 (index 0) → index 1, order ['2', '1', '3'].
    expect(press('ArrowDown', { altKey: true })).toBe(true)
    await tick()
    flushSync()

    // Second press immediately, BEFORE any `volumes-changed` refresh (the mock store never updates).
    // It must compute against the optimistic order, moving fav-1 from index 1 → 2: ['2', '3', '1'].
    // Without the local-first override it would re-read the stale store and wrongly emit ['2','1','3'].
    expect(press('ArrowDown', { altKey: true })).toBe(true)
    await tick()
    flushSync()

    expect(reorderFavorites).toHaveBeenCalledTimes(2)
    expect(reorderFavorites).toHaveBeenLastCalledWith(['2', '3', '1'])
  })

  it('Alt+ArrowUp at the top favorite is a no-op (no persist)', async () => {
    const { instance } = mountBreadcrumb()
    instance.open()
    await tick()
    flushSync()

    expect(press('Home')).toBe(true)
    await tick()
    flushSync()

    // Already at the top: Alt+ArrowUp is consumed but persists nothing.
    expect(press('ArrowUp', { altKey: true })).toBe(true)
    await tick()
    flushSync()
    expect(reorderFavorites).not.toHaveBeenCalled()
  })

  it('Alt+ArrowDown on a non-favorite (real volume) does not reorder', async () => {
    const { instance } = mountBreadcrumb()
    instance.open()
    await tick()
    flushSync()

    // End jumps to the last item: the real volume (Macintosh HD), not a favorite.
    expect(press('End')).toBe(true)
    await tick()
    flushSync()

    press('ArrowDown', { altKey: true })
    await tick()
    flushSync()
    expect(reorderFavorites).not.toHaveBeenCalled()
  })
})

describe('VolumeBreadcrumb favorite-rename keyboard guard', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
  })

  it('claims no key while the dropdown is closed', () => {
    mountBreadcrumb()
    expect(press('ArrowDown')).toBe(false)
  })

  it('consumes ArrowDown when the dropdown is open and not renaming', async () => {
    const { instance } = mountBreadcrumb()
    instance.open()
    await tick()
    flushSync()
    expect(instance.getIsOpen()).toBe(true)
    expect(press('ArrowDown')).toBe(true)
  })

  it('does NOT consume ArrowDown / Home / End while a favorite rename is active', async () => {
    const { instance } = mountBreadcrumb()
    instance.open()
    await tick()
    flushSync()

    // Start the inline rename the way the native row menu does: the backend emits
    // `volume-context-action` with `rename-favorite` for the right-clicked favorite.
    const favRow = menuRows()[0]
    expect(favRow).toBeTruthy()
    volumeContextActionHandler?.({ action: 'rename-favorite', volumeId: 'fav-1' })
    await tick()
    flushSync()

    expect(document.querySelector('.favorite-rename-input')).toBeTruthy()

    // The guard: keys the dropdown would otherwise eat must fall through (false)
    // so the rename textbox keeps them.
    for (const key of ['ArrowDown', 'ArrowUp', 'Home', 'End']) {
      expect(press(key)).toBe(false)
    }
  })

  it('stops EVERY key (Space included) from bubbling out of the rename input to the pane', async () => {
    const { instance } = mountBreadcrumb()
    instance.open()
    await tick()
    flushSync()

    const favRow = menuRows()[0]
    expect(favRow).toBeTruthy()
    volumeContextActionHandler?.({ action: 'rename-favorite', volumeId: 'fav-1' })
    await tick()
    flushSync()

    const input = document.querySelector('.favorite-rename-input') as HTMLInputElement
    expect(input).toBeTruthy()

    // A document-level listener stands in for the pane's Space-selection / type-to-jump
    // DOM listeners. The rename input must stop ALL keys from reaching it.
    const leaked: string[] = []
    const docListener = (e: KeyboardEvent) => leaked.push(e.key)
    document.addEventListener('keydown', docListener)
    try {
      for (const key of [' ', 'a', 'ArrowDown', 'Backspace']) {
        input.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }))
      }
      await tick()
      flushSync()
      expect(leaked).toEqual([])
    } finally {
      document.removeEventListener('keydown', docListener)
    }
  })
})

/**
 * The switcher's server rows: the dot that says how live the place is, and the
 * Disconnect control that replaces Eject on one.
 *
 * ❗ The control's WORD is the point. "Eject" promises safe-to-unplug, which a
 * server can't deliver, and `saved` is the row where a control would have no
 * subject at all: nothing is open to close.
 */
describe('VolumeBreadcrumb server rows', () => {
  function serverRow(overrides: Record<string, unknown>) {
    return {
      id: 'sftp-nas-local-22-ada',
      name: 'Naspolya',
      path: 'sftp://ada@nas.local:22/srv/data',
      category: 'network',
      fsType: 'sftp',
      isEjectable: false,
      ...overrides,
    }
  }

  async function openWith(rows: unknown[]) {
    stubs.volumes = rows
    const { instance } = mountBreadcrumb()
    instance.open()
    await tick()
    flushSync()
  }

  beforeEach(() => {
    document.body.innerHTML = ''
    stubs.volumes = null
    disconnectPlace.mockClear()
    showVolumeRowContextMenu.mockClear()
  })

  it('gives a live place a Disconnect control, and clicking it drops the session', async () => {
    await openWith([serverRow({ connectionState: 'direct' })])
    const button = document.querySelector('[data-menu-row] .eject-button') as HTMLButtonElement
    expect(button).toBeTruthy()
    expect(button.getAttribute('aria-label')).toBe('Disconnect Naspolya')

    button.click()
    await tick()
    expect(disconnectPlace).toHaveBeenCalledWith('sftp-nas-local-22-ada')
  })

  it('gives a dropped-but-registered place one too: there is still a session to close', async () => {
    await openWith([serverRow({ connectionState: 'disconnected' })])
    const button = document.querySelector('[data-menu-row] .eject-button') as HTMLButtonElement
    expect(button.getAttribute('aria-label')).toBe('Disconnect Naspolya')
  })

  it('gives a saved place NO control: nothing is open to close', async () => {
    await openWith([serverRow({ connectionState: 'saved' })])
    expect(document.querySelector('[data-menu-row] .eject-button')).toBeNull()
    // ❗ And it reads as saved rather than as a failure: greyed, hollow dot.
    expect(document.querySelector('[data-menu-row] .is-saved-place')).toBeTruthy()
  })

  // The dot's WORDS are pinned in `connection-tooltips.test.ts` (a pure call, no
  // hover timer); what the row owes is a class per state, since the stylesheet
  // paints each one differently and a missing rule renders an unpainted circle.
  it('paints one dot class per connection state', async () => {
    for (const state of ['direct', 'disconnected', 'needs_sign_in', 'needs_host_key_approval', 'saved'] as const) {
      document.body.innerHTML = ''
      await openWith([serverRow({ connectionState: state })])
      expect(document.querySelector(`[data-menu-row] .smb-indicator-${state}`), `no dot for ${state}`).toBeTruthy()
    }
  })

  it('shows the protocol in the filesystem slot, so a row says what it speaks', async () => {
    await openWith([serverRow({ connectionState: 'direct' })])
    expect(document.querySelector('[data-menu-row] .volume-fs')?.textContent).toBe('SFTP')
  })

  it('opens a server menu on right-click, with the row read as the caller sees it', async () => {
    await openWith([serverRow({ connectionState: 'direct' })])
    // Row 0 is the hub ("Servers"); the place is the one after it.
    const row = menuRows()[1]
    row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))
    // One store read runs before the popup: let it settle. ❗ One, ❌ not two —
    // deciding whether a secret is stored would cost a Keychain read, and every
    // read of one can raise a system prompt in front of the menu appearing.
    await vi.waitFor(() => {
      expect(showVolumeRowContextMenu).toHaveBeenCalled()
    })
    expect(showVolumeRowContextMenu).toHaveBeenCalledWith('sftp-nas-local-22-ada', 'Naspolya', false, false, {
      showsDisconnect: true,
      isSaved: true,
      pinned: false,
    })
  })
})

/**
 * An eject that's still running. One took 10.5 s with no sign of life in a user's
 * log, which invites another click; the backend joins that click to the running
 * eject anyway, and the control shows the eject is underway so nobody has to try.
 */
describe('VolumeBreadcrumb eject in progress', () => {
  const drive = {
    id: 'volumes-backup',
    name: 'Backup',
    path: '/Volumes/Backup',
    category: 'attached_volume',
    isEjectable: true,
  }

  async function openWith(rows: unknown[]) {
    stubs.volumes = rows
    const { instance } = mountBreadcrumb()
    instance.open()
    await tick()
    flushSync()
  }

  beforeEach(() => {
    document.body.innerHTML = ''
    stubs.volumes = null
    stubs.ejecting = new Set()
    ejectVolume.mockClear()
  })

  afterEach(() => {
    stubs.volumes = null
    stubs.ejecting = new Set()
  })

  it('shows a drive whose eject is running as in progress, and a click starts nothing', async () => {
    stubs.ejecting = new Set([drive.id])
    await openWith([drive])
    const button = document.querySelector('[data-menu-row] .eject-button') as HTMLButtonElement
    expect(button.disabled).toBe(true)
    expect(button.getAttribute('aria-label')).toBe('Ejecting Backup…')
    expect(button.querySelector('.spinner')).toBeTruthy()

    button.click()
    await tick()
    expect(ejectVolume).not.toHaveBeenCalled()
  })

  it('says Disconnecting on a phone whose disconnect is running', async () => {
    const phone = {
      id: 'adb-pixel-7-a1b2c3d',
      name: 'Pixel 7',
      path: 'adb://R58M12345',
      category: 'mobile_device',
      fsType: 'adb',
      isEjectable: true,
      deviceReadiness: { kind: 'ready' },
    }
    stubs.ejecting = new Set([phone.id])
    await openWith([phone])
    const button = document.querySelector('[data-menu-row] .eject-button') as HTMLButtonElement
    expect(button.disabled).toBe(true)
    expect(button.getAttribute('aria-label')).toBe('Disconnecting Pixel 7…')
    expect(button.querySelector('.spinner')).toBeTruthy()
  })

  it('keeps an idle drive pressable, with its eject glyph', async () => {
    await openWith([drive])
    const button = document.querySelector('[data-menu-row] .eject-button') as HTMLButtonElement
    expect(button.disabled).toBe(false)
    expect(button.getAttribute('aria-label')).toBe('Eject Backup')
    expect(button.querySelector('.spinner')).toBeNull()

    button.click()
    await tick()
    expect(ejectVolume).toHaveBeenCalledWith(drive.id)
  })
})

describe('VolumeBreadcrumb phone rows', () => {
  function phoneRow(overrides: Record<string, unknown>) {
    return {
      id: 'adb-pixel-7-a1b2c3d',
      name: 'Pixel 7',
      path: 'adb://R58M12345',
      category: 'mobile_device',
      fsType: 'adb',
      isEjectable: true,
      ...overrides,
    }
  }

  async function openWith(rows: unknown[]) {
    stubs.volumes = rows
    const { instance } = mountBreadcrumb()
    instance.open()
    await tick()
    flushSync()
  }

  beforeEach(() => {
    document.body.innerHTML = ''
    stubs.volumes = null
    ejectVolume.mockClear()
  })

  // ❗ `adb` has no per-client detach, so nothing here is made safe to unplug.
  // The ACTION is still the ordinary eject path (which for ADB answers
  // `DeviceDisconnect`); only the word changes.
  it('says Disconnect on a phone, and still runs the eject path', async () => {
    await openWith([phoneRow({ deviceReadiness: { kind: 'ready' } })])
    const button = document.querySelector('[data-menu-row] .eject-button') as HTMLButtonElement
    expect(button.getAttribute('aria-label')).toBe('Disconnect Pixel 7')

    button.click()
    await tick()
    expect(ejectVolume).toHaveBeenCalledWith('adb-pixel-7-a1b2c3d')
  })

  // A regression anchor: it passes with the readiness gate absent too, and that
  // is the point — it is what fails the day someone "tidies up" by disabling
  // every non-ready row.
  it('keeps a phone waiting for its Allow tap openable, and says what it waits for', async () => {
    await openWith([phoneRow({ deviceReadiness: { kind: 'waiting_for_authorization' } })])
    const row = menuRows()[0]
    expect(row.hasAttribute('data-disabled')).toBe(false)
    expect(row.getAttribute('aria-disabled')).toBeNull()
  })

  it('greys a phone the daemon lists but cannot use, and refuses to open it', async () => {
    await openWith([phoneRow({ deviceReadiness: { kind: 'unavailable', reason: 'offline' } })])
    const row = menuRows()[0]
    // Greyed and unopenable is what `disabled` means to the menu: it paints the row down,
    // says so to assistive tech, and never activates it.
    expect(row.hasAttribute('data-disabled')).toBe(true)
    expect(row.getAttribute('aria-disabled')).toBe('true')

    // Clicking it leaves the dropdown where it was: there is nothing to open.
    row.click()
    await tick()
    expect(menuSurface()).toBeTruthy()
  })
})

/**
 * Where the cursor sits the moment the switcher opens. It lands on the row wearing
 * the checkmark — the volume the pane's path actually sits on — so Enter re-opens
 * where you already are instead of jumping somewhere else. With nothing checked,
 * it falls back to the first row.
 */
describe('VolumeBreadcrumb highlight on open', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
    stubs.volumes = null
  })

  it('starts on the row carrying the checkmark, not on row 0', async () => {
    const { instance, target } = mountBreadcrumb()
    // The checkmark tracks `containingVolumeId`, which `resolvePathVolume` fills in
    // asynchronously; the chip naming the volume is that answer having landed.
    await vi.waitFor(() => {
      expect(target.querySelector('.volume-name')?.textContent).toContain('Macintosh HD')
    })
    instance.open()
    await tick()
    flushSync()

    // Three favorites lead the list, so the containing volume is row 3 — the case
    // that tells "the checked row" apart from "the first row".
    const rows = menuRows()
    expect(rows[3].hasAttribute('data-checked')).toBe(true)
    expect(isHighlighted(rows[3])).toBe(true)
    expect(isHighlighted(rows[0])).toBe(false)
  })

  it('falls back to row 0 when no row is the containing volume', async () => {
    // Favorites only: they never carry a checkmark, and the synthetic Servers row
    // isn't the pane's volume either, so nothing is checked.
    await openWithRows([
      { id: 'fav-1', name: 'Documents', path: '/Users/test/Documents', category: 'favorite', isEjectable: false },
      { id: 'fav-2', name: 'Downloads', path: '/Users/test/Downloads', category: 'favorite', isEjectable: false },
    ])

    expect(document.querySelector('[data-menu-row][data-checked]')).toBeNull()
    expect(isHighlighted(menuRows()[0])).toBe(true)
  })
})

/**
 * One cursor, one input device. Touching the keyboard suppresses hover highlighting
 * so the mouse's resting position can't show a second cursor; moving the pointer
 * more than 5 px hands control back and takes the highlight with it.
 */
describe('VolumeBreadcrumb keyboard vs pointer mode', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
    stubs.volumes = null
  })

  it('suppresses hover highlighting once the keyboard has the cursor', async () => {
    const { instance } = mountBreadcrumb()
    instance.open()
    await tick()
    flushSync()

    press('Home')
    await tick()
    flushSync()

    const dropdown = menuSurface()
    expect(dropdown?.classList.contains('keyboard-mode')).toBe(true)

    // Hovering another row must not move the cursor off row 0 while keyboard mode holds.
    const rows = menuRows()
    rows[2].dispatchEvent(new MouseEvent('mouseover', { bubbles: true }))
    await tick()
    flushSync()
    expect(isHighlighted(rows[0])).toBe(true)
    expect(isHighlighted(rows[2])).toBe(false)
  })

  it('a pointer move over 5 px leaves keyboard mode and takes the highlight to the row under the cursor', async () => {
    const { instance } = mountBreadcrumb()
    instance.open()
    await tick()
    flushSync()

    press('Home')
    await tick()
    flushSync()

    const dropdown = menuSurface()
    const rows = menuRows()

    // The first move only records where the pointer was: a mouse sitting still under
    // a moving list must not steal the cursor.
    rows[2].dispatchEvent(new MouseEvent('mousemove', { bubbles: true, clientX: 100, clientY: 100 }))
    await tick()
    flushSync()
    expect(dropdown?.classList.contains('keyboard-mode')).toBe(true)
    expect(isHighlighted(rows[0])).toBe(true)

    // A move past the 5 px threshold is a real gesture: the pointer takes over.
    rows[2].dispatchEvent(new MouseEvent('mousemove', { bubbles: true, clientX: 100, clientY: 120 }))
    await tick()
    flushSync()
    expect(dropdown?.classList.contains('keyboard-mode')).toBe(false)
    expect(isHighlighted(rows[2])).toBe(true)
    expect(isHighlighted(rows[0])).toBe(false)
  })
})

/**
 * The "Connect directly" submenu on a share the OS mounted for us. Pointer and
 * keyboard both reach it, and while it's up it owns the only visible cursor.
 */
describe('VolumeBreadcrumb os_mount submenu', () => {
  const share = {
    id: 'volumes-share',
    name: 'Share',
    path: '/Volumes/Share',
    category: 'network',
    fsType: 'smbfs',
    connectionState: 'os_mount',
    isEjectable: true,
  }

  /** Row 0 is the synthetic Servers hub; the share is the row after it. */
  const SHARE_ROW = 1

  beforeEach(() => {
    document.body.innerHTML = ''
    stubs.volumes = null
    connectDirectly.mockClear()
  })

  it('opens the submenu when the pointer rests on the row', async () => {
    await openWithRows([share])
    expect(document.querySelector('[data-menu-submenu]')).toBeNull()

    menuRows()[SHARE_ROW].dispatchEvent(new MouseEvent('mouseover', { bubbles: true }))
    await tick()
    flushSync()
    expect(document.querySelector('[data-menu-submenu]')).toBeTruthy()
  })

  it('ArrowRight opens it at the highlight, and ArrowLeft closes it again', async () => {
    await openWithRows([share])

    press('ArrowDown')
    await tick()
    flushSync()
    expect(press('ArrowRight')).toBe(true)
    await tick()
    flushSync()
    expect(document.querySelector('[data-menu-submenu]')).toBeTruthy()

    expect(press('ArrowLeft')).toBe(true)
    await tick()
    flushSync()
    expect(document.querySelector('[data-menu-submenu]')).toBeNull()
  })

  // Escape belongs to the submenu while the submenu is up: it backs out one level
  // rather than dropping the whole switcher.
  it('Escape closes the submenu and leaves the switcher open', async () => {
    const { instance } = await openWithRows([share])
    press('ArrowDown')
    await tick()
    flushSync()
    press('ArrowRight')
    await tick()
    flushSync()

    expect(press('Escape')).toBe(true)
    await tick()
    flushSync()
    expect(document.querySelector('[data-menu-submenu]')).toBeNull()
    expect(menuSurface()).toBeTruthy()
    expect(instance.getIsOpen()).toBe(true)
  })

  it('suppresses the parent row highlight while the submenu is up (one cursor at a time)', async () => {
    await openWithRows([share])
    press('ArrowDown')
    await tick()
    flushSync()
    const rows = menuRows()
    expect(isHighlighted(rows[SHARE_ROW])).toBe(true)

    press('ArrowRight')
    await tick()
    flushSync()
    expect(document.querySelector('[data-menu-submenu] [data-menu-row][data-highlighted]')).toBeTruthy()
    expect(isHighlighted(rows[SHARE_ROW])).toBe(false)
  })

  it('Enter runs "Connect directly" for the row the submenu belongs to', async () => {
    await openWithRows([share])
    press('ArrowDown')
    await tick()
    flushSync()
    press('ArrowRight')
    await tick()
    flushSync()

    expect(press('Enter')).toBe(true)
    await tick()
    flushSync()
    // The share's NAME rides along, read while the row is still listed: it's what
    // words the answer if the share goes away before the backend gets there.
    expect(connectDirectly).toHaveBeenCalledWith({ volumeId: 'volumes-share', shareName: 'Share' })
  })
})

/**
 * Where the dropdown lands. It's `position: fixed`, so it gets its coordinates from
 * the anchor's rect and a height budget from the space left below it; the highlighted
 * row is then scrolled into that budget.
 */
describe('VolumeBreadcrumb dropdown placement', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
    stubs.volumes = null
  })

  it('sets top, left, and a max height from the anchor rect and the space below it', async () => {
    const { instance, target } = mountBreadcrumb()
    // The test DOM has no layout engine, so the anchor reports its own geometry.
    const anchor = target.querySelector('.volume-name') as HTMLElement
    anchor.getBoundingClientRect = () =>
      ({ top: 30, bottom: 50, left: 12, right: 200, width: 188, height: 20, x: 12, y: 30 }) as DOMRect

    instance.open()
    // `querySelector<HTMLElement>` rather than an `as` cast: eslint's
    // `no-unnecessary-type-assertion` fixer strips the cast, and `Element.style`
    // then can't resolve (`docs/testing.md` § "Merging test files").
    await vi.waitFor(() => {
      expect(menuSurface()?.style.top).toBeTruthy()
    })
    const dropdown = menuSurface()

    expect(dropdown?.style.top).toBe('54px') // the anchor's bottom, plus 4px of air
    expect(dropdown?.style.left).toBe('12px') // flush with the anchor's left edge
    expect(dropdown?.style.maxHeight).toBe(`${String(window.innerHeight - 54 - 8)}px`)
  })

  it('scrolls the row the keyboard just landed on into view', async () => {
    const { instance } = mountBreadcrumb()
    instance.open()
    await tick()
    flushSync()

    const rows = menuRows()
    const scrollIntoView = vi.fn()
    ;(rows[1]).scrollIntoView = scrollIntoView

    press('ArrowDown')
    await vi.waitFor(() => {
      expect(scrollIntoView).toHaveBeenCalled()
    })
    // `nearest`: bring it in if it's out, ❌ never re-centre a row already on screen.
    expect(scrollIntoView).toHaveBeenCalledWith({ block: 'nearest' })
  })
})

/**
 * A control sitting inside a row does its own job and nothing else. Each of these
 * would otherwise activate the row it sits on, switching the pane's volume behind
 * the user's back.
 */
describe('VolumeBreadcrumb row controls do not activate their row', () => {
  const drive = {
    id: 'volumes-backup',
    name: 'Backup',
    path: '/Volumes/Backup',
    category: 'attached_volume',
    isEjectable: true,
  }

  beforeEach(() => {
    document.body.innerHTML = ''
    stubs.volumes = null
    stubs.indexStatus = null
    ejectVolume.mockClear()
    disconnectPlace.mockClear()
  })

  afterEach(() => {
    stubs.indexStatus = null
  })

  it('the eject button ejects, without navigating the pane or closing the switcher', async () => {
    const onVolumeChange = vi.fn()
    await openWithRows([drive], { onVolumeChange })

    const button = document.querySelector('[data-menu-row] .eject-button') as HTMLButtonElement
    button.click()
    await tick()
    flushSync()

    expect(ejectVolume).toHaveBeenCalledWith('volumes-backup')
    expect(onVolumeChange).not.toHaveBeenCalled()
    // Still open, so several drives can be ejected in a row.
    expect(menuSurface()).toBeTruthy()
  })

  it("a server row's Disconnect drops the session, without navigating the pane", async () => {
    const onVolumeChange = vi.fn()
    await openWithRows(
      [
        {
          id: 'sftp-nas-local-22-ada',
          name: 'Naspolya',
          path: 'sftp://ada@nas.local:22/srv/data',
          category: 'network',
          fsType: 'sftp',
          isEjectable: false,
          connectionState: 'direct',
        },
      ],
      { onVolumeChange },
    )

    const button = document.querySelector('[data-menu-row] .eject-button') as HTMLButtonElement
    button.click()
    await tick()
    flushSync()

    expect(disconnectPlace).toHaveBeenCalledWith('sftp-nas-local-22-ada')
    expect(onVolumeChange).not.toHaveBeenCalled()
    expect(menuSurface()).toBeTruthy()
  })

  it('the drive-index badge opens its own menu, without navigating the pane', async () => {
    stubs.indexStatus = {
      volumeId: drive.id,
      enabled: true,
      freshness: 'fresh',
      failure: null,
      scanCompletedAt: 1_750_000_000,
      scanDurationMs: 134_000,
      coalescedSignalsSinceSweep: 0,
      unreadableLocations: 0,
      unreadableRetried: false,
      nextSweepDueAt: null,
      liveWatch: true,
    }
    const onVolumeChange = vi.fn()
    await openWithRows([drive], { onVolumeChange })

    // The status fetch is async, so the badge arrives a beat after the row.
    await vi.waitFor(() => {
      expect(document.querySelector('[data-menu-row] .drive-index-badge')).not.toBeNull()
    })
    document.querySelector<HTMLButtonElement>('[data-menu-row] .drive-index-badge')?.click()
    await tick()
    flushSync()

    expect(document.querySelector('.drive-index-menu')).toBeTruthy()
    expect(onVolumeChange).not.toHaveBeenCalled()
    expect(menuSurface()).toBeTruthy()
  })
})

/**
 * The native row menu is built from the row that was right-clicked, whatever the
 * keyboard cursor is doing. (The webview freezes while a muda menu tracks, so the
 * highlight can't drift onto another row mid-menu either.)
 */
describe('VolumeBreadcrumb row context menu targeting', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
    stubs.volumes = null
    showVolumeRowContextMenu.mockClear()
  })

  it('acts on the right-clicked row, not on wherever the keyboard cursor sits', async () => {
    const { instance } = mountBreadcrumb()
    instance.open()
    await tick()
    flushSync()

    // Park the cursor at the far end of the list.
    press('End')
    await tick()
    flushSync()
    const rows = menuRows()
    expect(isHighlighted(rows[rows.length - 1])).toBe(true)

    // Right-click the FIRST favorite: the menu is built from that row's own facts.
    rows[0].dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))
    await vi.waitFor(() => {
      expect(showVolumeRowContextMenu).toHaveBeenCalled()
    })
    expect(showVolumeRowContextMenu).toHaveBeenCalledWith('fav-1', 'Documents', true, false)
  })
})
