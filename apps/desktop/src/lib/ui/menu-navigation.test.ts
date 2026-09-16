import { describe, it, expect } from 'vitest'
import { menuKeyAction, navigableValues, nextValue, sectionOf } from './menu-navigation'
import type { MenuSection } from './menu-types'

/** Three sections: a reorderable one, a plain one holding a disabled row, and an empty one. */
const sections: MenuSection[] = [
  {
    id: 'favorites',
    heading: 'Favorites',
    reorderable: true,
    items: [
      { value: 'fav-a', label: 'A' },
      { value: 'fav-b', label: 'B' },
    ],
  },
  {
    id: 'volumes',
    heading: 'Volumes',
    items: [
      { value: 'vol-1', label: 'Macintosh HD' },
      { value: 'vol-2', label: 'Backup', disabled: true },
      { value: 'vol-3', label: 'Share', submenu: [{ value: 'connect', label: 'Connect directly' }] },
    ],
  },
  { id: 'empty', heading: 'Nothing', items: [], emptyLabel: '(Your favorites will show here)' },
]

/** A keydown-like object: the fields the pure matcher reads, all modifiers explicit. */
function key(name: string, modifiers: Partial<Record<'altKey' | 'metaKey' | 'ctrlKey' | 'shiftKey', boolean>> = {}) {
  return {
    key: name,
    altKey: false,
    metaKey: false,
    ctrlKey: false,
    shiftKey: false,
    ...modifiers,
  } as unknown as KeyboardEvent
}

describe('navigableValues', () => {
  it('lists every enabled row across sections, in display order', () => {
    expect(navigableValues(sections)).toEqual(['fav-a', 'fav-b', 'vol-1', 'vol-3'])
  })

  it('skips disabled rows, headings, and empty placeholders', () => {
    expect(navigableValues(sections)).not.toContain('vol-2')
    expect(navigableValues([{ id: 'x', heading: 'H', items: [], emptyLabel: 'nothing here' }])).toEqual([])
  })
})

describe('nextValue', () => {
  const values = ['a', 'b', 'c']

  it('moves by delta', () => {
    expect(nextValue(values, 'a', 1)).toBe('b')
    expect(nextValue(values, 'c', -1)).toBe('b')
  })

  it('wraps around both ends', () => {
    expect(nextValue(values, 'c', 1)).toBe('a')
    expect(nextValue(values, 'a', -1)).toBe('c')
  })

  it('starts from the first row going down and the last going up when nothing is highlighted', () => {
    expect(nextValue(values, null, 1)).toBe('a')
    expect(nextValue(values, null, -1)).toBe('c')
  })

  it('returns null for an empty list', () => {
    expect(nextValue([], 'a', 1)).toBeNull()
  })
})

describe('sectionOf', () => {
  it('finds the section and in-section index of a row', () => {
    expect(sectionOf(sections, 'fav-b')).toEqual({ section: sections[0], index: 1 })
    expect(sectionOf(sections, 'vol-3')).toEqual({ section: sections[1], index: 2 })
  })

  it('returns null for an unknown value', () => {
    expect(sectionOf(sections, 'nope')).toBeNull()
  })
})

describe('menuKeyAction', () => {
  const plain = { hasSubmenu: false, submenuOpen: false, reorderable: false }

  it('maps the bare navigation keys', () => {
    expect(menuKeyAction(key('ArrowDown'), plain)).toEqual({ kind: 'move', delta: 1 })
    expect(menuKeyAction(key('ArrowUp'), plain)).toEqual({ kind: 'move', delta: -1 })
    expect(menuKeyAction(key('Home'), plain)).toEqual({ kind: 'edge', edge: 'first' })
    expect(menuKeyAction(key('End'), plain)).toEqual({ kind: 'edge', edge: 'last' })
    expect(menuKeyAction(key('Enter'), plain)).toEqual({ kind: 'activate' })
    expect(menuKeyAction(key(' '), plain)).toEqual({ kind: 'activate' })
    expect(menuKeyAction(key('Escape'), plain)).toEqual({ kind: 'close' })
  })

  it('ignores a navigation key carrying a command modifier', () => {
    // ⌘↓ and ⌃↓ mean other things; only the bare arrow moves the cursor.
    expect(menuKeyAction(key('ArrowDown', { metaKey: true }), plain)).toEqual({ kind: 'none' })
    expect(menuKeyAction(key('Enter', { metaKey: true }), plain)).toEqual({ kind: 'none' })
  })

  it('opens a submenu with ArrowRight only when the row has one', () => {
    expect(menuKeyAction(key('ArrowRight'), { ...plain, hasSubmenu: true })).toEqual({ kind: 'openSubmenu' })
    expect(menuKeyAction(key('ArrowRight'), plain)).toEqual({ kind: 'none' })
  })

  it('closes an open submenu with ArrowLeft or Escape, and absorbs its other arrows', () => {
    const open = { hasSubmenu: true, submenuOpen: true, reorderable: false }
    expect(menuKeyAction(key('ArrowLeft'), open)).toEqual({ kind: 'closeSubmenu' })
    expect(menuKeyAction(key('Escape'), open)).toEqual({ kind: 'closeSubmenu' })
    expect(menuKeyAction(key('Enter'), open)).toEqual({ kind: 'activate' })
    // A single-item submenu has nowhere to move: absorb rather than moving the parent's cursor.
    expect(menuKeyAction(key('ArrowDown'), open)).toEqual({ kind: 'absorb' })
    expect(menuKeyAction(key('ArrowRight'), open)).toEqual({ kind: 'absorb' })
  })

  it('reorders on exactly ⌥↑ / ⌥↓ inside a reorderable section', () => {
    const re = { ...plain, reorderable: true }
    expect(menuKeyAction(key('ArrowUp', { altKey: true }), re)).toEqual({ kind: 'reorder', delta: -1 })
    expect(menuKeyAction(key('ArrowDown', { altKey: true }), re)).toEqual({ kind: 'reorder', delta: 1 })
  })

  it('never reorders on ⌥ plus another modifier, nor outside a reorderable section', () => {
    const re = { ...plain, reorderable: true }
    // ⌥⌘↑ and ⇧⌥↑ mean other things and must not reorder on their way elsewhere.
    expect(menuKeyAction(key('ArrowUp', { altKey: true, metaKey: true }), re)).toEqual({ kind: 'none' })
    expect(menuKeyAction(key('ArrowUp', { altKey: true, shiftKey: true }), re)).toEqual({ kind: 'none' })
    expect(menuKeyAction(key('ArrowUp', { altKey: true }), plain)).toEqual({ kind: 'none' })
  })

  it('returns none for a key the menu has no use for', () => {
    expect(menuKeyAction(key('a'), plain)).toEqual({ kind: 'none' })
    expect(menuKeyAction(key('Tab'), plain)).toEqual({ kind: 'none' })
  })
})
