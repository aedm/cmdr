/**
 * What a tab writes to disk when it's sitting on a search-results snapshot.
 *
 * A `search-results://<id>` path names an in-memory, per-session result set, so it means
 * nothing on the next launch: a pane restored onto one has no folder to list. The tab
 * persists the newest real folder from its own history instead, with that entry's volume.
 */
import { describe, it, expect, vi } from 'vitest'

vi.mock('$lib/app-status-store', () => ({ savePaneTabs: vi.fn() }))
vi.mock('$lib/tauri-commands', () => ({
  showTabContextMenu: vi.fn(),
  onTabContextAction: vi.fn(() => Promise.resolve(() => {})),
  updatePinTabMenu: vi.fn(),
}))
vi.mock('$lib/logging/logger', () => ({
  getAppLogger: () => ({ warn: vi.fn(), info: vi.fn(), error: vi.fn(), debug: vi.fn() }),
}))

import { buildPersistedPaneTabs, createInitialTabState } from './tab-operations'
import { createTabManager } from '../tabs/tab-state-manager.svelte'
import { back, push } from '../navigation/navigation-history'
import type { TabState } from '../tabs/tab-types'

/** A tab that walked through `visited` and ended up showing the snapshot `snapshotPath`. */
function tabShowing(
  visited: { volumeId: string; path: string }[],
  snapshotPath: string | null = 'search-results://sr-1',
): TabState {
  const [first, ...rest] = visited
  const tab = createInitialTabState(first.path, first.volumeId)
  for (const entry of rest) {
    tab.history = push(tab.history, entry).history
  }
  if (snapshotPath !== null) {
    tab.history = push(tab.history, { volumeId: 'search-results', path: snapshotPath }).history
    tab.path = snapshotPath
    tab.volumeId = 'search-results'
  } else {
    const last = visited[visited.length - 1]
    tab.path = last.path
    tab.volumeId = last.volumeId
  }
  return tab
}

function persistedFirstTab(tab: TabState) {
  return buildPersistedPaneTabs(createTabManager(tab)).tabs[0]
}

describe('a tab on a search-results snapshot', () => {
  it('persists the folder the search started from', () => {
    const persisted = persistedFirstTab(tabShowing([{ volumeId: 'root', path: '/Users/me/Documents' }]))
    expect(persisted.path).toBe('/Users/me/Documents')
  })

  it('persists the volume that folder is on, never the snapshot pane one', () => {
    const persisted = persistedFirstTab(tabShowing([{ volumeId: 'usb-1', path: '/Volumes/Backup/photos' }]))
    expect(persisted.volumeId).toBe('usb-1')
  })

  it('walks past older snapshots to the newest real folder', () => {
    const tab = tabShowing([
      { volumeId: 'root', path: '/Users/me/Documents' },
      { volumeId: 'search-results', path: 'search-results://sr-1' },
      { volumeId: 'root', path: '/Users/me/Projects' },
    ])
    const persisted = persistedFirstTab(tab)
    expect(persisted).toMatchObject({ path: '/Users/me/Projects', volumeId: 'root' })
  })

  it('persists where the tab has BEEN, never a folder ahead of it in history', () => {
    // Walked Documents → snapshot → Projects, then Back into the snapshot. Projects is
    // forward history: a folder the user navigated away from, not one they left open.
    const tab = tabShowing(
      [
        { volumeId: 'root', path: '/Users/me/Documents' },
        { volumeId: 'search-results', path: 'search-results://sr-1' },
        { volumeId: 'root', path: '/Users/me/Projects' },
      ],
      null,
    )
    tab.history = back(tab.history)
    tab.path = 'search-results://sr-1'
    tab.volumeId = 'search-results'

    expect(persistedFirstTab(tab)).toMatchObject({ path: '/Users/me/Documents', volumeId: 'root' })
  })

  it('keeps every other field the tab carries', () => {
    const tab = tabShowing([{ volumeId: 'root', path: '/Users/me/Documents' }])
    tab.pinned = true
    tab.viewMode = 'brief'
    const persisted = persistedFirstTab(tab)
    expect(persisted).toMatchObject({ id: tab.id, pinned: true, viewMode: 'brief', sortBy: tab.sortBy })
  })

  it('leaves the snapshot path in place when history holds no real folder, for the load side to rescue', () => {
    const tab = tabShowing([{ volumeId: 'search-results', path: 'search-results://sr-0' }])
    const persisted = persistedFirstTab(tab)
    expect(persisted.path).toBe('search-results://sr-1')
  })
})

describe('an ordinary tab', () => {
  it('persists exactly where it stands', () => {
    const persisted = persistedFirstTab(tabShowing([{ volumeId: 'root', path: '/Users/me/Documents' }], null))
    expect(persisted).toMatchObject({ path: '/Users/me/Documents', volumeId: 'root' })
  })
})
