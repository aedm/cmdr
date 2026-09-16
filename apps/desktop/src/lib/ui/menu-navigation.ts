/**
 * Pure keyboard math for the house `Menu` (`lib/ui/Menu.svelte`): which rows the cursor
 * can land on, where it goes next, and what a keystroke means. No DOM and no state, so
 * the whole keyboard contract is unit-testable; `menu-controller.svelte.ts` applies the
 * answers.
 */

import type { MenuItem, MenuSection } from './menu-types'

/** What a keystroke means to an open menu. `none` is "not ours"; the controller still swallows it. */
export type MenuAction =
  | { kind: 'move'; delta: -1 | 1 }
  | { kind: 'edge'; edge: 'first' | 'last' }
  | { kind: 'activate' }
  | { kind: 'close' }
  | { kind: 'openSubmenu' }
  | { kind: 'closeSubmenu' }
  /** Move the cursor WITHIN an open submenu, which has its own. */
  | { kind: 'moveSubmenu'; delta: -1 | 1 }
  /** Consumed on purpose so it can't reach the parent menu (ArrowRight inside a submenu). */
  | { kind: 'absorb' }
  | { kind: 'reorder'; delta: -1 | 1 }
  | { kind: 'none' }

export interface MenuKeyContext {
  /** The highlighted row has a submenu, so ArrowRight opens it. */
  hasSubmenu: boolean
  submenuOpen: boolean
  /** The highlighted row sits in a `reorderable` section, so ⌥↑/⌥↓ move it. */
  reorderable: boolean
}

/**
 * Every row the cursor may land on, in display order: headings, separators, empty
 * placeholders, and disabled rows are not among them.
 */
export function navigableValues<T>(sections: readonly MenuSection<T>[]): string[] {
  const values: string[] = []
  for (const section of sections) {
    for (const item of section.items) {
      if (!item.disabled) values.push(item.value)
    }
  }
  return values
}

/**
 * The value `delta` steps from `current`, wrapping at both ends. With nothing highlighted,
 * a downward step starts at the first row and an upward one at the last. `null` when there
 * is nothing to land on.
 */
export function nextValue(values: readonly string[], current: string | null, delta: number): string | null {
  if (values.length === 0) return null
  const index = current === null ? -1 : values.indexOf(current)
  if (index < 0) return delta > 0 ? values[0] : values[values.length - 1]
  const next = (index + delta + values.length) % values.length
  return values[next]
}

/** The section a row belongs to, plus its index WITHIN that section (disabled rows counted). */
export function sectionOf<T>(
  sections: readonly MenuSection<T>[],
  value: string,
): { section: MenuSection<T>; index: number } | null {
  for (const section of sections) {
    const index = section.items.findIndex((item) => item.value === value)
    if (index >= 0) return { section, index }
  }
  return null
}

/** The item for a value, looking one level into submenus so a submenu pick resolves too. */
export function itemOf<T>(sections: readonly MenuSection<T>[], value: string): MenuItem<T> | null {
  for (const section of sections) {
    for (const item of section.items) {
      if (item.value === value) return item
      const nested = item.submenu?.find((child) => child.value === value)
      if (nested) return nested
    }
  }
  return null
}

/**
 * True when the event carries a modifier that makes it somebody else's combo. A bare cursor
 * key is the menu's; ⌘↓ / ⌃↓ / ⌥↓ mean other things and must pass through untouched. Shift is
 * deliberately not in here: ⇧Enter is still Enter to a menu, and on AZERTY the digits need it.
 */
function hasCommandModifier(event: KeyboardEvent): boolean {
  return event.metaKey || event.ctrlKey || event.altKey
}

/** Exactly ⌥ and nothing else: ⌥⌘↑ and ⇧⌥↑ mean other things and must not reorder on their way. */
function isReorderCombo(event: KeyboardEvent): boolean {
  return event.altKey && !event.metaKey && !event.ctrlKey && !event.shiftKey
}

/** An open submenu owns the cursor keys: it has its own cursor, and its own rows to walk. */
function submenuKeyAction(key: string): MenuAction {
  switch (key) {
    case 'ArrowLeft':
    case 'Escape':
      return { kind: 'closeSubmenu' }
    case 'Enter':
    case ' ':
      return { kind: 'activate' }
    case 'ArrowUp':
      return { kind: 'moveSubmenu', delta: -1 }
    case 'ArrowDown':
      return { kind: 'moveSubmenu', delta: 1 }
    case 'ArrowRight':
      // One level only, so there is nothing further right: swallow it rather than letting it
      // reach the parent list and move the cursor behind the open submenu.
      return { kind: 'absorb' }
    default:
      return { kind: 'none' }
  }
}

/** The plain list contract, with the cursor on a row. */
function rowKeyAction(key: string, hasSubmenu: boolean): MenuAction {
  switch (key) {
    case 'ArrowDown':
      return { kind: 'move', delta: 1 }
    case 'ArrowUp':
      return { kind: 'move', delta: -1 }
    case 'Home':
      return { kind: 'edge', edge: 'first' }
    case 'End':
      return { kind: 'edge', edge: 'last' }
    case 'Enter':
    case ' ':
      return { kind: 'activate' }
    case 'Escape':
      return { kind: 'close' }
    case 'ArrowRight':
      return hasSubmenu ? { kind: 'openSubmenu' } : { kind: 'none' }
    default:
      return { kind: 'none' }
  }
}

/** What one keystroke means to an open menu, given what the cursor is sitting on. */
export function menuKeyAction(event: KeyboardEvent, context: MenuKeyContext): MenuAction {
  if (context.reorderable && isReorderCombo(event)) {
    if (event.key === 'ArrowUp') return { kind: 'reorder', delta: -1 }
    if (event.key === 'ArrowDown') return { kind: 'reorder', delta: 1 }
  }
  if (hasCommandModifier(event)) return { kind: 'none' }
  return context.submenuOpen ? submenuKeyAction(event.key) : rowKeyAction(event.key, context.hasSubmenu)
}
