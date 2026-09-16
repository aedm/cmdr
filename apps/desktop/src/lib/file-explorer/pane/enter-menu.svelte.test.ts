import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest'
import { createEnterMenu, type EnterMenuController } from './enter-menu.svelte'
import type { FileEntry } from '$lib/file-explorer/types'

const { openSettingsWindowMock } = vi.hoisted(() => ({
  openSettingsWindowMock: vi.fn<(surface: string, section?: string[]) => Promise<void>>(),
}))
vi.mock('$lib/settings/settings-window', () => ({
  openSettingsWindow: openSettingsWindowMock,
}))

function makeEntry(name: string, isArchive = true): FileEntry {
  // Only name/isDirectory/isArchive are read; a partial cast keeps the test focused.
  return { name, path: `/left/${name}`, isDirectory: false, isSymlink: false, isArchive } as unknown as FileEntry
}

function makeDeps() {
  return {
    getPaneElement: () => null,
    browse: vi.fn(),
    open: vi.fn(),
    restoreFocus: vi.fn(),
  }
}

/** A real keydown: the menu controller calls `preventDefault` / `stopPropagation` on it. */
function key(name: string): KeyboardEvent {
  return new KeyboardEvent('keydown', { key: name, bubbles: true, cancelable: true })
}

let controllers: EnterMenuController[] = []

function build(deps = makeDeps()): { controller: EnterMenuController; deps: ReturnType<typeof makeDeps> } {
  const controller = createEnterMenu(deps)
  controllers.push(controller)
  return { controller, deps }
}

beforeEach(() => {
  openSettingsWindowMock.mockClear()
  controllers = []
})

afterEach(() => {
  for (const controller of controllers) controller.dispose()
})

describe('createEnterMenu', () => {
  it('starts closed, with the three rows', () => {
    const { controller } = build()
    expect(controller.menu.isOpen).toBe(false)
    expect(controller.menu.sections[0].items.map((i) => i.value)).toEqual(['browse', 'open', 'configure'])
  })

  it('openFor opens the menu and leads with the resolved action', () => {
    const { controller } = build()
    controller.openFor(makeEntry('a.zip'), 'open')
    expect(controller.menu.isOpen).toBe(true)
    expect(controller.menu.highlightedValue).toBe('open')

    controller.openFor(makeEntry('b.zip'), 'ask')
    expect(controller.menu.highlightedValue).toBe('browse')
  })

  it('a pointer pick routes browse and open to the deps with the pending entry', () => {
    const { controller, deps } = build()
    const entry = makeEntry('a.zip')

    controller.openFor(entry, 'ask')
    controller.menu.surface.activate('browse')
    expect(deps.browse).toHaveBeenCalledWith(entry)
    expect(controller.menu.isOpen).toBe(false)
    expect(deps.restoreFocus).toHaveBeenCalled()

    controller.openFor(entry, 'ask')
    controller.menu.surface.activate('open')
    expect(deps.open).toHaveBeenCalledWith(entry)
  })

  it('configure deep-links to the Archives settings section', () => {
    const { controller } = build()
    controller.openFor(makeEntry('a.zip'), 'ask')
    controller.menu.surface.activate('configure')
    expect(openSettingsWindowMock).toHaveBeenCalledWith('enter-menu', ['Behavior', 'Archives'])
  })

  it('restores focus once, on a real close', () => {
    const { controller, deps } = build()
    controller.openFor(makeEntry('a.zip'), 'ask')
    controller.menu.close()
    expect(controller.menu.isOpen).toBe(false)
    expect(deps.restoreFocus).toHaveBeenCalledTimes(1)
    controller.menu.close()
    expect(deps.restoreFocus).toHaveBeenCalledTimes(1)
  })

  describe('keys', () => {
    it('is a no-op when the menu is closed', () => {
      const { controller } = build()
      expect(controller.menu.handleKey(key('Enter'))).toBe(false)
    })

    it('ArrowDown / ArrowUp move the highlight, wrapping at the ends', () => {
      const { controller } = build()
      controller.openFor(makeEntry('a.zip'), 'ask') // highlighted = browse
      expect(controller.menu.handleKey(key('ArrowDown'))).toBe(true)
      expect(controller.menu.highlightedValue).toBe('open')
      controller.menu.handleKey(key('ArrowDown'))
      expect(controller.menu.highlightedValue).toBe('configure')
      // The house menu wraps, where this popup used to clamp at the last row.
      controller.menu.handleKey(key('ArrowDown'))
      expect(controller.menu.highlightedValue).toBe('browse')
      controller.menu.handleKey(key('ArrowUp'))
      expect(controller.menu.highlightedValue).toBe('configure')
    })

    it('Home and End jump to the first and last rows', () => {
      const { controller } = build()
      controller.openFor(makeEntry('a.zip'), 'ask')
      controller.menu.handleKey(key('End'))
      expect(controller.menu.highlightedValue).toBe('configure')
      controller.menu.handleKey(key('Home'))
      expect(controller.menu.highlightedValue).toBe('browse')
    })

    it('Enter selects the highlighted row and closes', () => {
      const { controller, deps } = build()
      const entry = makeEntry('a.zip')
      controller.openFor(entry, 'ask') // browse
      controller.menu.handleKey(key('ArrowDown')) // open
      expect(controller.menu.handleKey(key('Enter'))).toBe(true)
      expect(deps.open).toHaveBeenCalledWith(entry)
      expect(controller.menu.isOpen).toBe(false)
    })

    it('Escape closes without selecting', () => {
      const { controller, deps } = build()
      controller.openFor(makeEntry('a.zip'), 'ask')
      expect(controller.menu.handleKey(key('Escape'))).toBe(true)
      expect(controller.menu.isOpen).toBe(false)
      expect(deps.browse).not.toHaveBeenCalled()
      expect(deps.open).not.toHaveBeenCalled()
    })

    it('swallows an unrelated key, so the pane behind the popup stays inert', () => {
      const { controller } = build()
      controller.openFor(makeEntry('a.zip'), 'ask')
      const event = key('a')
      // An open menu owns the keyboard; this popup used to let stray keys through to the pane.
      expect(controller.menu.handleKey(event)).toBe(true)
      expect(event.defaultPrevented).toBe(false)
    })
  })
})
