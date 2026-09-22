/**
 * A row's action list: which entries a volume, server, or favorite row offers, in what
 * order, and which ones a running transfer greys. The ONE list every door shows (the row's
 * → submenu, its right-click, the servers hub's right-click), so it's pinned here rather
 * than per surface.
 */
import { describe, it, expect, vi, beforeAll, afterAll, beforeEach } from 'vitest'
import { _setLocaleForTests } from '$lib/intl/locale'

const ejectVolume = vi.fn(() => Promise.resolve())
const disconnectPlace = vi.fn(() => Promise.resolve(true))
const setPlacePinned = vi.fn(() => Promise.resolve(true))
const busy = new Set<string>()

vi.mock('$lib/tauri-commands', () => ({
  ejectVolume: (...args: unknown[]) => ejectVolume(...(args as [])),
  disconnectPlace: (...args: unknown[]) => disconnectPlace(...(args as [])),
  setPlacePinned: (...args: unknown[]) => setPlacePinned(...(args as [])),
  listSavedServers: () => Promise.resolve([]),
}))
vi.mock('$lib/stores/volume-busy-store.svelte', () => ({
  isVolumeBusy: (id: string) => busy.has(id),
  isVolumeEjecting: () => false,
}))
vi.mock('$lib/ui/toast', () => ({ addToast: vi.fn() }))
vi.mock('$lib/logging/logger', () => ({
  getAppLogger: () => ({ warn: vi.fn(), info: vi.fn(), error: vi.fn(), debug: vi.fn() }),
}))

import {
  favoriteRowMenu,
  rowMenuItems,
  rowMenuSections,
  runVolumeRowAction,
  volumeRowMenu,
  type RowMenu,
} from './row-menu'
import type { VolumeInfo } from '../types'

const idle = { busy: false, ejecting: false, isSaved: false, directConnection: undefined, autoReconnect: undefined }

const usbDrive: VolumeInfo = {
  id: 'usb-stick',
  name: 'Stick',
  path: '/Volumes/Stick',
  category: 'attached_volume',
  isEjectable: true,
}

const phone: VolumeInfo = {
  ...usbDrive,
  id: 'adb-pixel',
  name: 'Pixel',
  category: 'mobile_device',
  deviceReadiness: { kind: 'ready' },
}

const share: VolumeInfo = {
  id: 'smb-naspi-media',
  name: 'media',
  path: '/Volumes/media',
  category: 'network',
  fsType: 'smbfs',
  isEjectable: true,
  connectionState: 'os_mount',
}

const place: VolumeInfo = {
  id: 'sftp-nas-local-22-ada',
  name: 'Naspolya',
  path: 'sftp://ada@nas.local:22/srv/data',
  category: 'network',
  fsType: 'sftp',
  isEjectable: false,
  connectionState: 'direct',
  pinned: false,
}

/** Each group as its entries' action or toggle names, so an assertion reads like the menu. */
function shape(menu: RowMenu): string[][] {
  return menu.map((group) => group.map((entry) => (entry.type === 'action' ? entry.action : `toggle:${entry.toggle}`)))
}

function entry(menu: RowMenu, name: string) {
  return menu.flat().find((e) => (e.type === 'action' ? e.action : `toggle:${e.toggle}`) === name)
}

beforeAll(() => {
  _setLocaleForTests('en-US')
})
afterAll(() => {
  _setLocaleForTests(null)
})
beforeEach(() => {
  vi.clearAllMocks()
  busy.clear()
})

describe('volumeRowMenu', () => {
  it('offers nothing on a fixed disk', () => {
    expect(volumeRowMenu({ ...usbDrive, id: 'root', category: 'main_volume', isEjectable: false }, idle)).toEqual([])
  })

  it('offers Eject on a removable drive, keeping the menu up so several go in a row', () => {
    const menu = volumeRowMenu(usbDrive, idle)
    expect(shape(menu)).toEqual([['eject']])
    expect(entry(menu, 'eject')).toMatchObject({ label: 'Eject (Stick)', icon: 'eject', keepsMenuOpen: true })
    expect(entry(menu, 'eject')).not.toMatchObject({ disabled: true })
  })

  it('greys Eject while a transfer touches the drive, and says why', () => {
    const menu = volumeRowMenu(usbDrive, { ...idle, busy: true })
    expect(entry(menu, 'eject')).toMatchObject({ label: 'Eject (Stick) (busy)', disabled: true })
  })

  it('greys Eject while the drive’s eject is still running', () => {
    expect(entry(volumeRowMenu(usbDrive, { ...idle, ejecting: true }), 'eject')).toMatchObject({ disabled: true })
  })

  it('words a phone’s eject as Disconnect, with the unplug glyph', () => {
    expect(entry(volumeRowMenu(phone, idle), 'eject')).toMatchObject({ label: 'Disconnect', icon: 'unplug' })
  })

  it('puts an SMB share’s direct-connection switch in its own group, under Eject', () => {
    const menu = volumeRowMenu(share, { ...idle, directConnection: true })
    expect(shape(menu)).toEqual([['eject'], ['toggle:direct-connection']])
    expect(entry(menu, 'toggle:direct-connection')).toMatchObject({ type: 'toggle', checked: true })
  })

  it('leaves the switch out until Rust has answered for the share', () => {
    expect(shape(volumeRowMenu(share, idle))).toEqual([['eject']])
  })

  it('offers a saved, connected server every server action, in order, and ❌ never Eject', () => {
    const menu = volumeRowMenu(place, { ...idle, isSaved: true })
    expect(shape(menu)).toEqual([['open', 'edit', 'disconnect', 'pin', 'forget-secret', 'forget-server']])
  })

  it('offers Unpin on a pinned server', () => {
    expect(shape(volumeRowMenu({ ...place, pinned: true }, idle))).toEqual([
      ['open', 'disconnect', 'unpin', 'forget-secret'],
    ])
  })

  it('offers no Disconnect on a saved row: there is no session to end', () => {
    const menu = volumeRowMenu({ ...place, connectionState: 'saved', pinned: true }, { ...idle, isSaved: true })
    expect(entry(menu, 'disconnect')).toBeUndefined()
  })

  it('offers no Edit or Forget server on a server nothing saved', () => {
    const menu = volumeRowMenu(place, idle)
    expect(entry(menu, 'edit')).toBeUndefined()
    expect(entry(menu, 'forget-server')).toBeUndefined()
  })

  it('puts a saved place’s “Reconnect automatically” in its own group, explained, so nobody reads it as “connect at startup”', () => {
    const menu = volumeRowMenu(place, { ...idle, isSaved: true, autoReconnect: false })
    expect(shape(menu)).toEqual([
      ['open', 'edit', 'disconnect', 'pin', 'forget-secret', 'forget-server'],
      ['toggle:auto-reconnect'],
    ])
    expect(entry(menu, 'toggle:auto-reconnect')).toMatchObject({
      type: 'toggle',
      label: 'Reconnect automatically',
      checked: false,
      tooltip: expect.stringContaining("doesn't make Cmdr connect at startup") as unknown,
    })
  })

  it('leaves “Reconnect automatically” out where nothing saved backs the row: there is nothing to persist', () => {
    expect(entry(volumeRowMenu(place, idle), 'toggle:auto-reconnect')).toBeUndefined()
  })

  it('greys the destructive server actions under a running transfer, never Open, Edit, or the pin', () => {
    const menu = volumeRowMenu(place, { ...idle, busy: true, isSaved: true })
    const greyed = menu
      .flat()
      .filter((e) => e.type === 'action' && e.disabled)
      .map((e) => (e.type === 'action' ? e.action : ''))
    expect(greyed).toEqual(['disconnect', 'forget-secret', 'forget-server'])
    expect(entry(menu, 'disconnect')?.label).toBe('Disconnect (busy)')
  })
})

describe('favoriteRowMenu', () => {
  it('offers Rename and Remove, both keeping the menu up (the rename happens in the row)', () => {
    const menu = favoriteRowMenu()
    expect(shape(menu)).toEqual([['rename-favorite', 'remove-favorite']])
    expect(menu.flat().every((e) => e.type === 'action' && e.keepsMenuOpen)).toBe(true)
  })
})

describe('rowMenuItems', () => {
  it('turns groups into submenu rows: unique values, a rule between groups, and the entry carried back', () => {
    const menu = volumeRowMenu(share, { ...idle, directConnection: false })
    const items = rowMenuItems(share.id, menu, (e) => e)
    expect(items?.map((item) => item.value)).toEqual([
      'row:smb-naspi-media:eject',
      'row:smb-naspi-media:toggle:direct-connection',
    ])
    expect(items?.[0]).toMatchObject({ icon: { lucide: 'eject' }, keepsMenuOpen: true, separatorBefore: false })
    expect(items?.[1]).toMatchObject({ checked: false, separatorBefore: true })
    expect(items?.[1].data).toBe(menu[1][0])
  })

  it('carries a switch’s explanation onto its row as the tooltip', () => {
    const menu = volumeRowMenu(place, { ...idle, isSaved: true, autoReconnect: true })
    const row = rowMenuItems(place.id, menu, (e) => e)?.find((item) => item.value.endsWith('toggle:auto-reconnect'))
    expect(row).toMatchObject({ checked: true, separatorBefore: true })
    expect(row?.tooltip).toContain('reconnects to this server on its own')
  })

  it('gives a row with no actions no submenu at all, so it draws no arrow', () => {
    expect(rowMenuItems('root', [], (e) => e)).toBeUndefined()
  })
})

describe('rowMenuSections', () => {
  it('turns each group into a section, so the primitive draws the rule between them in a top-level menu', () => {
    const menu = volumeRowMenu(share, { ...idle, directConnection: true })
    const sections = rowMenuSections(share.id, menu, (e) => e)
    expect(sections.map((section) => section.items.map((item) => item.value))).toEqual([
      ['row:smb-naspi-media:eject'],
      ['row:smb-naspi-media:toggle:direct-connection'],
    ])
    // A top-level list splits into sections, never rules inside one.
    expect(sections.flatMap((section) => section.items).some((item) => item.separatorBefore)).toBe(false)
  })
})

describe('runVolumeRowAction', () => {
  it('ejects a drive through the same guarded path as the row’s eject button', async () => {
    await runVolumeRowAction({ volume: usbDrive, action: 'eject' })
    expect(ejectVolume).toHaveBeenCalledWith('usb-stick')
  })

  it('refuses to disconnect a server under a running transfer, whatever the menu showed', async () => {
    busy.add(place.id)
    await runVolumeRowAction({ volume: place, action: 'disconnect' })
    expect(disconnectPlace).not.toHaveBeenCalled()
  })

  it('hands every other server action to the servers family', async () => {
    await runVolumeRowAction({ volume: place, action: 'pin' })
    expect(setPlacePinned).toHaveBeenCalledWith('sftp-nas-local-22-ada', true)
  })

  it('opens through the surface’s own navigation', async () => {
    const onOpen = vi.fn()
    await runVolumeRowAction({ volume: place, action: 'open', onOpen })
    expect(onOpen).toHaveBeenCalledWith('sftp-nas-local-22-ada')
  })
})
