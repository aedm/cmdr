import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { createMenu } from './menu-controller.svelte'
import type { MenuController, MenuDeps } from './menu-controller.svelte'
import type { MenuSection } from './menu-types'

/**
 * Controller tests for the house `Menu`. The controller owns everything that isn't
 * rendering: open state, the highlight, the keyboard contract, keyboard-vs-pointer
 * mode, submenus, and reorder. It holds no DOM, so it's exercised directly here;
 * `Menu.svelte.test.ts` covers what the surface renders.
 */

/** Two sections: a reorderable one and a plain one holding a disabled row and a submenu row. */
function makeSections(): MenuSection[] {
  return [
    {
      id: 'favorites',
      heading: 'Favorites',
      reorderable: true,
      items: [
        { value: 'fav-a', label: 'A' },
        { value: 'fav-b', label: 'B' },
        { value: 'fav-c', label: 'C' },
      ],
    },
    {
      id: 'volumes',
      heading: 'Volumes',
      items: [
        { value: 'vol-1', label: 'Macintosh HD' },
        { value: 'vol-2', label: 'Backup', disabled: true },
        {
          value: 'vol-3',
          label: 'Share',
          // TWO rows on purpose: a one-row submenu hides a cursor that lights every row.
          submenu: [
            { value: 'connect', label: 'Connect directly' },
            { value: 'forget', label: 'Forget this share' },
          ],
        },
      ],
    },
  ]
}

function keydown(key: string, modifiers: KeyboardEventInit = {}): KeyboardEvent {
  return new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...modifiers })
}

/** A real `MouseEvent`: a bare `{ clientX, clientY }` literal doesn't type-check, and an
 *  `as MouseEvent` cast gets stripped by the lint auto-fixer (`docs/testing.md`). */
function pointerAt(clientX: number, clientY: number): MouseEvent {
  return new MouseEvent('mousemove', { clientX, clientY })
}

/** An anchor element, in the document so the surface can measure it. */
function anchorEl(): HTMLElement {
  const el = document.createElement('button')
  document.body.appendChild(el)
  return el
}

let menus: MenuController[] = []

/** Builds a controller and registers it for teardown, so no document listener outlives a test. */
// `Partial<MenuDeps<undefined>>`, not `Parameters<typeof createMenu>[0]`: on a generic
// function that resolves `T` to `unknown`, which then can't be spread into a `<undefined>` menu.
function build(deps: Partial<MenuDeps> = {}): MenuController {
  const menu = createMenu({
    getSections: makeSections,
    onSelect: vi.fn(),
    ...deps,
  })
  menus.push(menu)
  return menu
}

beforeEach(() => {
  menus = []
})

afterEach(() => {
  for (const menu of menus) menu.destroy()
  document.body.innerHTML = ''
})

describe('open and close', () => {
  it('starts closed and opens under an anchor element', () => {
    const menu = build()
    expect(menu.isOpen).toBe(false)
    const el = anchorEl()
    menu.openUnder(el)
    expect(menu.isOpen).toBe(true)
    expect(menu.anchor).toEqual({ kind: 'element', element: el })
  })

  it('opens at a point for a context-menu style caller', () => {
    const menu = build()
    menu.openAt({ x: 40, y: 80 })
    expect(menu.anchor).toEqual({ kind: 'point', x: 40, y: 80 })
  })

  it('highlights the first navigable row on open', () => {
    const menu = build()
    menu.openUnder(anchorEl())
    expect(menu.highlightedValue).toBe('fav-a')
  })

  it('toggleUnder closes an open menu', () => {
    const menu = build()
    const el = anchorEl()
    menu.toggleUnder(el)
    expect(menu.isOpen).toBe(true)
    menu.toggleUnder(el)
    expect(menu.isOpen).toBe(false)
  })

  it('reports open changes and restores focus once, on a real close', () => {
    const onOpenChange = vi.fn()
    const restoreFocus = vi.fn()
    const menu = build({ onOpenChange, restoreFocus })
    menu.openUnder(anchorEl())
    expect(onOpenChange).toHaveBeenCalledWith(true)
    menu.close()
    expect(onOpenChange).toHaveBeenLastCalledWith(false)
    expect(restoreFocus).toHaveBeenCalledTimes(1)
    menu.close()
    expect(restoreFocus).toHaveBeenCalledTimes(1)
  })

  it('clears the highlight and submenu on close', () => {
    const menu = build()
    menu.openUnder(anchorEl())
    menu.surface.openSubmenu('vol-3', true)
    menu.close()
    expect(menu.highlightedValue).toBeNull()
    expect(menu.openSubmenuValue).toBeNull()
  })
})

describe('keyboard navigation', () => {
  it('moves the highlight past headings and skips disabled rows', () => {
    const menu = build()
    menu.openUnder(anchorEl())
    menu.highlight('vol-1')
    menu.handleKey(keydown('ArrowDown'))
    // `vol-2` is disabled, so the cursor lands on `vol-3`.
    expect(menu.highlightedValue).toBe('vol-3')
  })

  it('wraps around at both ends', () => {
    const menu = build()
    menu.openUnder(anchorEl())
    menu.highlight('vol-3') // the last navigable row
    menu.handleKey(keydown('ArrowDown'))
    expect(menu.highlightedValue).toBe('fav-a')
    menu.handleKey(keydown('ArrowUp'))
    expect(menu.highlightedValue).toBe('vol-3')
  })

  it('Home and End jump to the first and last navigable rows', () => {
    const menu = build()
    menu.openUnder(anchorEl())
    menu.handleKey(keydown('End'))
    expect(menu.highlightedValue).toBe('vol-3')
    menu.handleKey(keydown('Home'))
    expect(menu.highlightedValue).toBe('fav-a')
  })

  it('Enter and Space activate the highlighted row and close', () => {
    const onSelect = vi.fn()
    const menu = build({ onSelect })
    menu.openUnder(anchorEl())
    menu.highlight('vol-1')
    expect(menu.handleKey(keydown('Enter'))).toBe(true)
    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ value: 'vol-1' }))
    expect(menu.isOpen).toBe(false)
  })

  it('Escape closes without selecting', () => {
    const onSelect = vi.fn()
    const menu = build({ onSelect })
    menu.openUnder(anchorEl())
    expect(menu.handleKey(keydown('Escape'))).toBe(true)
    expect(menu.isOpen).toBe(false)
    expect(onSelect).not.toHaveBeenCalled()
  })

  it('never activates a disabled row', () => {
    const onSelect = vi.fn()
    const menu = build({ onSelect })
    menu.openUnder(anchorEl())
    menu.highlight('vol-2') // disabled: the highlight refuses to land there
    expect(menu.highlightedValue).not.toBe('vol-2')
    menu.handleKey(keydown('Enter'))
    expect(onSelect).not.toHaveBeenCalledWith(expect.objectContaining({ value: 'vol-2' }))
  })

  it('handles nothing while the menu is closed', () => {
    const menu = build()
    expect(menu.handleKey(keydown('ArrowDown'))).toBe(false)
  })
})

describe('the open menu owns the keyboard', () => {
  it('gives onKey the first look, before its own handling', () => {
    const onKey = vi.fn(() => true)
    const menu = build({ onKey })
    menu.openUnder(anchorEl())
    expect(menu.handleKey(keydown('ArrowDown'))).toBe(true)
    expect(onKey).toHaveBeenCalled()
    // The caller claimed it, so the cursor never moved.
    expect(menu.highlightedValue).toBe('fav-a')
  })

  it('falls through to its own handling when onKey passes', () => {
    const onKey = vi.fn(() => false)
    const menu = build({ onKey })
    menu.openUnder(anchorEl())
    menu.handleKey(keydown('ArrowDown'))
    expect(menu.highlightedValue).toBe('fav-b')
  })

  it('swallows a key it has no use for, so the pane behind stays inert', () => {
    const menu = build()
    menu.openUnder(anchorEl())
    const event = keydown('x')
    const stop = vi.spyOn(event, 'stopPropagation')
    const prevent = vi.spyOn(event, 'preventDefault')
    expect(menu.handleKey(event)).toBe(true)
    expect(stop).toHaveBeenCalled()
    // Swallowed from the app, but never defaulted away: ⌘Q and friends still mean what they mean.
    expect(prevent).not.toHaveBeenCalled()
  })

  it('routes real document keydowns while open, and stops once closed', () => {
    const menu = build()
    menu.openUnder(anchorEl())
    document.dispatchEvent(keydown('ArrowDown'))
    expect(menu.highlightedValue).toBe('fav-b')
    menu.close()
    document.dispatchEvent(keydown('ArrowDown'))
    expect(menu.highlightedValue).toBeNull()
  })

  it('hands every keystroke to an inline editor while isEditing', () => {
    let editing = false
    const menu = build({ isEditing: () => editing })
    menu.openUnder(anchorEl())
    editing = true
    const event = keydown('ArrowDown')
    const stop = vi.spyOn(event, 'stopPropagation')
    expect(menu.handleKey(event)).toBe(false)
    expect(stop).not.toHaveBeenCalled()
    expect(menu.highlightedValue).toBe('fav-a')
  })
})

describe('keyboard versus pointer mode', () => {
  it('enters keyboard mode on an arrow key', () => {
    const menu = build()
    menu.openUnder(anchorEl())
    expect(menu.keyboardMode).toBe(false)
    menu.handleKey(keydown('ArrowDown'))
    expect(menu.keyboardMode).toBe(true)
  })

  it('leaves keyboard mode only after the pointer travels more than 5 px', () => {
    const menu = build()
    menu.openUnder(anchorEl())
    menu.handleKey(keydown('ArrowDown'))
    menu.surface.pointerMoved(pointerAt(100, 100))
    menu.surface.pointerMoved(pointerAt(103, 102))
    expect(menu.keyboardMode).toBe(true)
    menu.surface.pointerMoved(pointerAt(120, 100))
    expect(menu.keyboardMode).toBe(false)
  })

  it('ignores hover while in keyboard mode, and follows it otherwise', () => {
    const menu = build()
    menu.openUnder(anchorEl())
    menu.handleKey(keydown('ArrowDown')) // keyboard mode, cursor on fav-b
    menu.surface.hover('vol-1')
    expect(menu.highlightedValue).toBe('fav-b')
    menu.surface.pointerMoved(pointerAt(0, 0))
    menu.surface.pointerMoved(pointerAt(200, 200))
    menu.surface.hover('vol-1')
    expect(menu.highlightedValue).toBe('vol-1')
  })
})

describe('submenus', () => {
  it('opens one with ArrowRight only on a row that has one', () => {
    const menu = build()
    menu.openUnder(anchorEl())
    menu.highlight('vol-1')
    menu.handleKey(keydown('ArrowRight'))
    expect(menu.openSubmenuValue).toBeNull()
    menu.highlight('vol-3')
    menu.handleKey(keydown('ArrowRight'))
    expect(menu.openSubmenuValue).toBe('vol-3')
  })

  it('closes one with ArrowLeft, keeping the parent cursor', () => {
    const menu = build()
    menu.openUnder(anchorEl())
    menu.highlight('vol-3')
    menu.handleKey(keydown('ArrowRight'))
    menu.handleKey(keydown('ArrowLeft'))
    expect(menu.openSubmenuValue).toBeNull()
    expect(menu.highlightedValue).toBe('vol-3')
  })

  it('activates the submenu item, not the parent row', () => {
    const onSelect = vi.fn()
    const menu = build({ onSelect })
    menu.openUnder(anchorEl())
    menu.highlight('vol-3')
    menu.handleKey(keydown('ArrowRight'))
    menu.handleKey(keydown('Enter'))
    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ value: 'connect' }))
  })

  it('shows one cursor: an open submenu takes the parent row’s highlight', () => {
    const menu = build()
    menu.openUnder(anchorEl())
    menu.highlight('vol-3')
    menu.handleKey(keydown('ArrowRight'))
    expect(menu.submenuHighlighted).toBe(true)
    expect(menu.parentHighlightSuppressed).toBe(true)
  })

  it('opens onto the submenu’s first row, and only that one', () => {
    const menu = build()
    menu.openUnder(anchorEl())
    menu.highlight('vol-3')
    menu.handleKey(keydown('ArrowRight'))
    expect(menu.submenuHighlightedValue).toBe('connect')
  })

  it('walks the submenu’s own rows with the arrows, wrapping', () => {
    const menu = build()
    menu.openUnder(anchorEl())
    menu.highlight('vol-3')
    menu.handleKey(keydown('ArrowRight'))
    menu.handleKey(keydown('ArrowDown'))
    expect(menu.submenuHighlightedValue).toBe('forget')
    // The parent's cursor never moves while its submenu owns the keys.
    expect(menu.highlightedValue).toBe('vol-3')
    menu.handleKey(keydown('ArrowDown'))
    expect(menu.submenuHighlightedValue).toBe('connect')
    menu.handleKey(keydown('ArrowUp'))
    expect(menu.submenuHighlightedValue).toBe('forget')
  })

  it('activates the submenu row the cursor is on, not its first', () => {
    const onSelect = vi.fn()
    const menu = build({ onSelect })
    menu.openUnder(anchorEl())
    menu.highlight('vol-3')
    menu.handleKey(keydown('ArrowRight'))
    menu.handleKey(keydown('ArrowDown'))
    menu.handleKey(keydown('Enter'))
    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ value: 'forget' }))
  })

  it('shows no submenu cursor when the pointer opened it, until the pointer reaches in', () => {
    const menu = build()
    menu.openUnder(anchorEl())
    menu.surface.openSubmenu('vol-3', false)
    expect(menu.submenuHighlightedValue).toBeNull()
    expect(menu.submenuHighlighted).toBe(false)
    menu.surface.hoverSubmenu('forget')
    expect(menu.submenuHighlightedValue).toBe('forget')
  })

  it('still activates the first row when Enter lands on a hover-opened submenu', () => {
    const onSelect = vi.fn()
    const menu = build({ onSelect })
    menu.openUnder(anchorEl())
    menu.highlight('vol-3')
    menu.surface.openSubmenu('vol-3', false)
    menu.handleKey(keydown('Enter'))
    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ value: 'connect' }))
  })
})

/**
 * The payload type is the API's promise to consumers, so it's pinned here rather than
 * eyeballed: the no-type-argument call must compile, and a typed one must reach the snippets.
 */
describe('the payload type', () => {
  it('needs no type argument, and `unknown` is what a row without one carries', () => {
    const plain = createMenu({ getSections: makeSections, onSelect: () => {} })
    // @ts-expect-error `data` is `unknown` here, so it can't be used as a string without narrowing.
    const _payload: string = plain.sections[0].items[0].data
    expect(plain.isOpen).toBe(false)
  })

  it('hands a typed payload back on select', () => {
    interface Place {
      id: string
    }
    let captured: string | null = null
    const menu = createMenu<Place>({
      getSections: () => [{ id: 'places', items: [{ value: 'a', label: 'A', data: { id: 'x' } }] }],
      onSelect: (item) => {
        // Compile-time half: this assignment fails if `data` ever widens back to `unknown`.
        const payload: Place | undefined = item.data
        captured = payload ? payload.id : null
      },
    })
    menus.push(menu)
    menu.openUnder(anchorEl())
    menu.surface.activate('a')
    expect(captured).toBe('x')
  })
})

describe('keyboard reorder', () => {
  it('moves the row and reports the section’s new order', () => {
    const onReorder = vi.fn()
    const menu = build({ onReorder })
    menu.openUnder(anchorEl())
    menu.highlight('fav-a')
    expect(menu.handleKey(keydown('ArrowDown', { altKey: true }))).toBe(true)
    expect(onReorder).toHaveBeenCalledWith({
      sectionId: 'favorites',
      orderedValues: ['fav-b', 'fav-a', 'fav-c'],
      from: 0,
      to: 1,
    })
  })

  it('carries the highlight with the moved row', () => {
    const menu = build({ onReorder: vi.fn() })
    menu.openUnder(anchorEl())
    menu.highlight('fav-a')
    menu.handleKey(keydown('ArrowDown', { altKey: true }))
    expect(menu.highlightedValue).toBe('fav-a')
  })

  it('does nothing at either edge of the section', () => {
    const onReorder = vi.fn()
    const menu = build({ onReorder })
    menu.openUnder(anchorEl())
    menu.highlight('fav-a')
    menu.handleKey(keydown('ArrowUp', { altKey: true }))
    expect(onReorder).not.toHaveBeenCalled()
    menu.highlight('fav-c')
    menu.handleKey(keydown('ArrowDown', { altKey: true }))
    expect(onReorder).not.toHaveBeenCalled()
  })

  it('does nothing in a section that is not reorderable', () => {
    const onReorder = vi.fn()
    const menu = build({ onReorder })
    menu.openUnder(anchorEl())
    menu.highlight('vol-1')
    menu.handleKey(keydown('ArrowDown', { altKey: true }))
    expect(onReorder).not.toHaveBeenCalled()
  })
})

describe('pointer drag reorder', () => {
  /** Three favorite rows 20px tall from y=0: midpoints 10, 30, 50. */
  function bindMidpoints(menu: MenuController): void {
    menu.surface.bindSurface({ getRowMidpoints: () => [10, 30, 50] })
  }

  function mouse(type: string, clientY: number): MouseEvent {
    return new MouseEvent(type, { clientY, clientX: 0, button: 0, bubbles: true })
  }

  it('treats a press without travel as a plain click, not a reorder', () => {
    const onSelect = vi.fn()
    const onReorder = vi.fn()
    const menu = build({ onSelect, onReorder })
    menu.openUnder(anchorEl())
    bindMidpoints(menu)
    menu.surface.startDrag('fav-a', mouse('mousedown', 10))
    window.dispatchEvent(mouse('mouseup', 11))
    expect(onReorder).not.toHaveBeenCalled()
    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ value: 'fav-a' }))
  })

  it('shows the drop cue at the gap the pointer is over, then reorders on drop', () => {
    const onSelect = vi.fn()
    const onReorder = vi.fn()
    const menu = build({ onSelect, onReorder })
    menu.openUnder(anchorEl())
    bindMidpoints(menu)
    menu.surface.startDrag('fav-a', mouse('mousedown', 10))
    window.dispatchEvent(mouse('mousemove', 45))
    expect(menu.draggingValue).toBe('fav-a')
    // Below midpoints 10 and 30, above 50: the gap above the last row.
    expect(menu.dropSlot).toBe(2)
    window.dispatchEvent(mouse('mouseup', 45))
    expect(onReorder).toHaveBeenCalledWith({
      sectionId: 'favorites',
      orderedValues: ['fav-b', 'fav-a', 'fav-c'],
      from: 0,
      to: 1,
    })
    // A drag is not a click: the row must not also open.
    expect(onSelect).not.toHaveBeenCalled()
    expect(menu.draggingValue).toBeNull()
    expect(menu.dropSlot).toBeNull()
  })

  it('hides the cue where a drop would leave the row where it is', () => {
    const menu = build({ onReorder: vi.fn() })
    menu.openUnder(anchorEl())
    bindMidpoints(menu)
    menu.surface.startDrag('fav-a', mouse('mousedown', 10))
    window.dispatchEvent(mouse('mousemove', 20))
    expect(menu.dropSlot).toBeNull()
  })

  it('never drags from a section that is not reorderable', () => {
    const onReorder = vi.fn()
    const menu = build({ onReorder })
    menu.openUnder(anchorEl())
    bindMidpoints(menu)
    menu.surface.startDrag('vol-1', mouse('mousedown', 10))
    window.dispatchEvent(mouse('mousemove', 45))
    expect(menu.draggingValue).toBeNull()
    window.dispatchEvent(mouse('mouseup', 45))
    expect(onReorder).not.toHaveBeenCalled()
  })

  it('drops its window listeners on destroy, mid-drag', () => {
    const onReorder = vi.fn()
    const menu = build({ onReorder })
    menu.openUnder(anchorEl())
    bindMidpoints(menu)
    menu.surface.startDrag('fav-a', mouse('mousedown', 10))
    menu.destroy()
    window.dispatchEvent(mouse('mousemove', 45))
    window.dispatchEvent(mouse('mouseup', 45))
    expect(onReorder).not.toHaveBeenCalled()
  })
})

describe('pointer selection', () => {
  it('activates a row and closes', () => {
    const onSelect = vi.fn()
    const menu = build({ onSelect })
    menu.openUnder(anchorEl())
    menu.surface.activate('vol-1')
    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ value: 'vol-1' }))
    expect(menu.isOpen).toBe(false)
  })

  it('refuses a disabled row', () => {
    const onSelect = vi.fn()
    const menu = build({ onSelect })
    menu.openUnder(anchorEl())
    menu.surface.activate('vol-2')
    expect(onSelect).not.toHaveBeenCalled()
    expect(menu.isOpen).toBe(true)
  })

  it('reports a right-click with the item, leaving the menu open', () => {
    const onContextMenu = vi.fn()
    const menu = build({ onContextMenu })
    menu.openUnder(anchorEl())
    const event = new MouseEvent('contextmenu', { bubbles: true })
    menu.surface.contextMenu('fav-b', event)
    expect(onContextMenu).toHaveBeenCalledWith(expect.objectContaining({ value: 'fav-b' }), event)
    expect(menu.isOpen).toBe(true)
  })
})

describe('live sections', () => {
  it('reads the caller’s data on every access, so the menu tracks its state', () => {
    let sections = makeSections()
    const menu = build({ getSections: () => sections })
    menu.openUnder(anchorEl())
    sections = [{ id: 'favorites', items: [{ value: 'fav-z', label: 'Z' }] }]
    menu.handleKey(keydown('Home'))
    expect(menu.highlightedValue).toBe('fav-z')
  })
})
