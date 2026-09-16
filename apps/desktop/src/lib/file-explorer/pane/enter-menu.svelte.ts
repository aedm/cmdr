/**
 * Reactive controller for the Enter-behavior popup (the house `Menu` shown when an
 * archive/bundle set to Ask is opened). It holds the pending entry and routes a choice —
 * browse, open, or deep-link to Settings — so `FilePane` renders `<Menu menu={enterMenu.menu}>`
 * and calls `openFor` from its navigate fork.
 *
 * Everything about the menu itself (open state, anchoring, the cursor, the document-capture
 * key routing, focus) belongs to `$lib/ui/menu-controller.svelte.ts`. Pure decision logic stays
 * in `archive-enter-policy.ts`; section building and anchoring in `enter-menu.ts`.
 */

import type { FileEntry } from '$lib/file-explorer/types'
import { createMenu, type MenuController } from '$lib/ui/menu-controller.svelte'
import { openSettingsWindow } from '$lib/settings/settings-window'
import type { EnterAction } from './archive-enter-policy'
import { buildEnterMenuSections, enterMenuAnchor, enterMenuHighlight } from './enter-menu'

export interface EnterMenuDeps {
  /** The pane's root element, for anchoring the menu at the cursor row. */
  getPaneElement: () => HTMLElement | null
  /** Step into the archive/bundle like a folder. */
  browse: (entry: FileEntry) => void
  /** Hand the archive/bundle to its default app (LaunchServices). */
  open: (entry: FileEntry) => void
  /** Return DOM focus to the explorer container after the menu closes. */
  restoreFocus: () => void
}

export interface EnterMenuController {
  /** Hand this to `<Menu menu={…}>`; it renders nothing while closed. */
  readonly menu: MenuController
  /** Open the popup for an entry; `action` (the resolved policy) picks the lead row. */
  openFor: (entry: FileEntry, action: EnterAction) => void
  /** Detach the menu's listeners (call from the host's teardown). */
  dispose: () => void
}

export function createEnterMenu(deps: EnterMenuDeps): EnterMenuController {
  // Not reactive: read only when a row is picked, never rendered.
  let pendingEntry: FileEntry | null = null

  const menu = createMenu({
    // Rebuilt per read so a live locale switch is reflected (cheap: three rows).
    getSections: buildEnterMenuSections,
    onSelect: (item) => {
      const entry = pendingEntry
      pendingEntry = null
      if (item.value === 'configure') {
        void openSettingsWindow('enter-menu', ['Behavior', 'Archives'])
        return
      }
      if (!entry) return
      if (item.value === 'browse') deps.browse(entry)
      else if (item.value === 'open') deps.open(entry)
    },
    restoreFocus: deps.restoreFocus,
  })

  return {
    menu,
    openFor(entry, action) {
      pendingEntry = entry
      menu.openAt(enterMenuAnchor(deps.getPaneElement()))
      menu.highlight(enterMenuHighlight(action))
    },
    dispose() {
      menu.destroy()
    },
  }
}
