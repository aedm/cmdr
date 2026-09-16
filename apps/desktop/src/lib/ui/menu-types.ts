import type { IconName } from './icons/icon-map'

/**
 * The vocabulary of the house `Menu` (`lib/ui/Menu.svelte`). Lives in a `.ts` (not the
 * component's module script like `SelectItem`) because non-Svelte glue — menu controllers
 * such as `file-explorer/pane/enter-menu.ts` — consumes it, and a type imported from a
 * `.svelte` file resolves to `any` under the plain-TypeScript lint service.
 *
 * `T` is the caller's own payload, carried on `MenuItem.data` and handed back untouched on
 * select and to every row snippet, so a consumer never has to look its row up again. It
 * defaults to `unknown`, so `createMenu({ getSections, onSelect })` needs no type argument;
 * a caller that wants typed `data` writes `createMenu<VolumeInfo>({ … })` and gets it in
 * `onSelect` and in every snippet's `MenuRowContext`.
 */

/** A lucide glyph, or an image the caller already has a URL for (a volume or folder icon). */
export type MenuIcon = { lucide: IconName } | { src: string }

/** One row. `value` is the stable identity: it's what `onSelect` emits and what the highlight tracks. */
export interface MenuItem<T = unknown> {
  value: string
  label: string
  icon?: MenuIcon
  /** Renders the leading checkmark. The checkmark column is always reserved, so rows stay aligned. */
  checked?: boolean
  /**
   * A single character shown in the leftmost column, which also activates the row when typed
   * (digits only today). The column appears only in a menu where at least one row declares one,
   * and the rows that don't get a blank placeholder so every label still lines up.
   */
  accelerator?: string
  /** Greyed, skipped by the keyboard, never activates. */
  disabled?: boolean
  tooltip?: string
  /** One level only: a submenu item's own `submenu` is ignored. */
  submenu?: MenuItem<T>[]
  data?: T
}

export interface MenuSection<T = unknown> {
  id: string
  heading?: string
  items: MenuItem<T>[]
  /** Rows reorder within this section by drag and ⌥↑/⌥↓; the caller persists in `onReorder`. */
  reorderable?: boolean
  /** Shown (disabled, unfocusable) when the section is empty, so the section still reads as a real state. */
  emptyLabel?: string
}

/** The one argument every row snippet (`label`, `trailing`, `below`) takes. */
export interface MenuRowContext<T = unknown> {
  item: MenuItem<T>
  section: MenuSection<T>
  /** The row's index WITHIN its section, which is also what a reorder moves. */
  index: number
  highlighted: boolean
  dragging: boolean
}

/** What `onReorder` receives, once, on drop (or on a ⌥↑/⌥↓ that actually moves something). */
export interface MenuReorder {
  sectionId: string
  /** The section's item values in their new order: hand this straight to a persist call. */
  orderedValues: string[]
  from: number
  to: number
}

/** Where an open menu pins itself: under an element, or at a viewport point (a context menu). */
export type MenuAnchor = { kind: 'element'; element: HTMLElement } | { kind: 'point'; x: number; y: number }
