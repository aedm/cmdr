/**
 * What launch does with a `search-results://` path that's already on disk.
 *
 * A snapshot id names an in-memory, per-session result set, so a stored one names nothing
 * by the time it's read back: the pane came up on a path that isn't a folder and the app
 * flickered instead of listing anything. Every load path replaces such a path with the
 * default volume's last-used folder, else the home folder.
 *
 * A shared `disk` Map backs a fake `@tauri-apps/plugin-store`, the same rig
 * `app-status-store.first-run.test.ts` uses.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { loadAppStatus, loadPaneTabs } from './app-status-store'
import type { PersistedTab } from './file-explorer/tabs/tab-types'

const disk = vi.hoisted(() => new Map<string, unknown>())

vi.mock('@tauri-apps/plugin-store', () => ({
  load: vi.fn(() =>
    Promise.resolve({
      get: (key: string) => Promise.resolve(disk.get(key)),
      set: (key: string, value: unknown) => {
        disk.set(key, value)
        return Promise.resolve()
      },
      delete: (key: string) => Promise.resolve(disk.delete(key)),
      has: (key: string) => Promise.resolve(disk.has(key)),
      keys: () => Promise.resolve([...disk.keys()]),
      save: () => Promise.resolve(),
    }),
  ),
}))

vi.mock('./settings/store-path', () => ({
  resolveStorePath: (name: string) => Promise.resolve(name),
}))

/** Everything exists, so no walk-up fallback kicks in. */
const alwaysExists = () => Promise.resolve(true)

const SNAPSHOT_PATH = 'search-results://sr-3'

function storedTab(overrides: Partial<PersistedTab> = {}): PersistedTab {
  return {
    id: 'tab-1',
    path: SNAPSHOT_PATH,
    volumeId: 'search-results',
    sortBy: 'name',
    sortOrder: 'ascending',
    viewMode: 'full',
    pinned: false,
    ...overrides,
  }
}

beforeEach(() => {
  disk.clear()
})

describe('a stored tab on a search-results path', () => {
  it('comes back on the default volume last-used folder', async () => {
    disk.set('leftTabs', { tabs: [storedTab()], activeTabId: 'tab-1' })
    disk.set('lastUsedPaths', { root: '/Users/me/Projects' })

    const paneTabs = await loadPaneTabs('left', alwaysExists)

    expect(paneTabs.tabs[0]).toMatchObject({ path: '/Users/me/Projects', volumeId: 'root' })
  })

  it('comes back on the home folder when the volume has no last-used folder', async () => {
    disk.set('rightTabs', { tabs: [storedTab()], activeTabId: 'tab-1' })

    const paneTabs = await loadPaneTabs('right', alwaysExists)

    expect(paneTabs.tabs[0]).toMatchObject({ path: '~', volumeId: 'root' })
  })

  it('keeps the rest of the tab, so only the location is rewritten', async () => {
    disk.set('leftTabs', { tabs: [storedTab({ pinned: true, viewMode: 'brief' })], activeTabId: 'tab-1' })

    const paneTabs = await loadPaneTabs('left', alwaysExists)

    expect(paneTabs.tabs[0]).toMatchObject({ id: 'tab-1', pinned: true, viewMode: 'brief' })
  })

  it('leaves a real folder alone', async () => {
    disk.set('leftTabs', { tabs: [storedTab({ path: '/Users/me/Documents', volumeId: 'usb-1' })], activeTabId: 'tab-1' })
    disk.set('lastUsedPaths', { root: '/Users/me/Projects' })

    const paneTabs = await loadPaneTabs('left', alwaysExists)

    expect(paneTabs.tabs[0]).toMatchObject({ path: '/Users/me/Documents', volumeId: 'usb-1' })
  })
})

describe('a stored pane path on a search-results path', () => {
  it('comes back on the default volume last-used folder', async () => {
    disk.set('leftPath', SNAPSHOT_PATH)
    disk.set('leftVolumeId', 'search-results')
    disk.set('lastUsedPaths', { root: '/Users/me/Projects' })

    const status = await loadAppStatus(alwaysExists)

    expect(status.leftPath).toBe('/Users/me/Projects')
    expect(status.leftVolumeId).toBe('root')
  })

  it('comes back on the home folder when the volume has no last-used folder, both sides', async () => {
    disk.set('leftPath', SNAPSHOT_PATH)
    disk.set('leftVolumeId', 'search-results')
    disk.set('rightPath', SNAPSHOT_PATH)
    disk.set('rightVolumeId', 'search-results')

    const status = await loadAppStatus(alwaysExists)

    expect(status).toMatchObject({
      leftPath: '~',
      leftVolumeId: 'root',
      rightPath: '~',
      rightVolumeId: 'root',
    })
  })
})
