/**
 * Favoriting ONE folder, with the words that go with it.
 *
 * Two surfaces run this: the `favorites.add` command (the palette and the Go menu,
 * on the focused pane's folder) and the favorites menu's `0` row. They differ only
 * in which path they hand over, so the add and its two toasts live here rather than
 * once per caller — a second copy is how the success and the refusal drift apart.
 *
 * The native folder-row and `..` menus are NOT callers: those favorite the
 * right-clicked path entirely in Rust (`menu/menu_handlers.rs`), and never come back
 * through the frontend.
 */

import { addFavorite } from '$lib/tauri-commands'
import { addToast } from '$lib/ui/toast'
import { tString } from '$lib/intl/messages.svelte'

/** The last path segment, for a friendly toast label (`/Users/me/Docs` → `Docs`). */
export function lastPathSegment(path: string): string {
  const trimmed = path.replace(/\/+$/, '')
  const slash = trimmed.lastIndexOf('/')
  return slash >= 0 ? trimmed.slice(slash + 1) || trimmed : trimmed
}

/**
 * Adds `path` to the favorites, saying so either way.
 *
 * Rust's `add_favorite` is the gate that can refuse (a path the switcher could never
 * show a row for); both callers grey their affordance out on the same reading first,
 * so a refusal here means the two disagreed and the failure toast is the honest answer.
 */
export async function addFavoriteFolder(path: string): Promise<void> {
  try {
    await addFavorite(path, null)
    addToast(tString('commands.handler.favoriteAdded', { name: lastPathSegment(path) }), { level: 'success' })
  } catch {
    addToast(tString('commands.handler.favoriteAddFailed'), { level: 'error' })
  }
}
