import { tick } from 'svelte'
import { removeFavorite, renameFavorite, reorderFavorites, stripFavoritePrefix } from '$lib/tauri-commands'
import { addToast } from '$lib/ui/toast'
import { tString } from '$lib/intl/messages.svelte'
import type { VolumeInfo } from '../types'

export interface FavoritesControllerDeps {
  /** The favorites in current display order (the component's `favorites` derived list). */
  getFavorites: () => VolumeInfo[]
  /** The full store volume list, for reconciling the optimistic order against store truth. */
  getVolumes: () => VolumeInfo[]
  /** The inline rename `<input>`, focused + selected when a rename starts. */
  getRenameInputRef: () => HTMLInputElement | undefined
}

export interface FavoritesController {
  /** Optimistic favorite-id order override (null = render the store order). Read by the component's
   *  `effectiveVolumes` / `favorites` deriveds so a reorder shows instantly, before the IPC round-trip. */
  get optimisticFavoriteIds(): string[] | null
  get renamingFavoriteId(): string | null
  get renameDraft(): string
  set renameDraft(value: string)
  remove: (volume: VolumeInfo) => Promise<void>
  startRename: (volume: VolumeInfo) => void
  cancelRename: () => void
  commitRename: (volume: VolumeInfo) => Promise<void>
  handleRenameKeyDown: (e: KeyboardEvent, volume: VolumeInfo) => void
  /** Take the new favorite order the menu just settled on (drag or ⌥↑/⌥↓) and persist it. */
  applyReorder: (orderedLocationIds: string[]) => void
}

/**
 * The favorites the switcher owns: inline rename, remove, and the local-first optimistic
 * order behind a reorder.
 *
 * ❗ The reorder MECHANICS (the drag threshold, the drop-line cue, ⌥↑/⌥↓, and carrying the
 * cursor with the moved row) belong to the house `Menu` primitive, which hands the settled
 * order here through `onReorder`. This module's half is what the primitive deliberately has
 * no business knowing: which ids the backend wants, and what to show while it answers.
 */
export function createFavoritesController(deps: FavoritesControllerDeps): FavoritesController {
  // Optimistic favorite order for instant, local-first reorder. A keyboard (⌥↑/⌥↓) or pointer
  // reorder sets this to the new order of favorite ids SYNCHRONOUSLY, so the switcher re-renders
  // immediately and a rapid next press computes against fresh state; the backend persist runs in the
  // background. Reconciled to `null` once `volumes-changed` brings the store to the same order (or
  // the favorite set changes elsewhere). `null` = no override, render the store order.
  let optimisticFavoriteIds = $state<string[] | null>(null)

  // ── Inline rename ────────────────────────────────────────────────────
  let renamingFavoriteId = $state<string | null>(null)
  let renameDraft = $state('')

  // Drop the optimistic order once the store catches up to it (the persisted `volumes-changed`
  // landed), or if the favorite set changed elsewhere (add / remove) so the override is stale.
  $effect(() => {
    const order = optimisticFavoriteIds
    if (!order) return
    const storeFavIds = deps
      .getVolumes()
      .filter((v) => v.category === 'favorite')
      .map((v) => v.id)
    const sameSet = storeFavIds.length === order.length && storeFavIds.every((id) => order.includes(id))
    const sameOrder = sameSet && storeFavIds.every((id, i) => id === order[i])
    if (sameOrder || !sameSet) optimisticFavoriteIds = null
  })

  async function remove(volume: VolumeInfo): Promise<void> {
    try {
      await removeFavorite(stripFavoritePrefix(volume.id))
    } catch {
      addToast(tString('fileExplorer.navigation.removeFavoriteFailed'), { level: 'error' })
    }
  }

  function startRename(volume: VolumeInfo) {
    renamingFavoriteId = volume.id
    renameDraft = volume.name
    void tick().then(() => {
      const input = deps.getRenameInputRef()
      input?.focus()
      input?.select()
    })
  }

  function cancelRename() {
    renamingFavoriteId = null
    renameDraft = ''
  }

  async function commitRename(volume: VolumeInfo): Promise<void> {
    const trimmed = renameDraft.trim()
    const id = renamingFavoriteId
    cancelRename()
    if (!id || !trimmed || trimmed === volume.name) return
    try {
      await renameFavorite(stripFavoritePrefix(id), trimmed)
    } catch {
      addToast(tString('fileExplorer.navigation.renameFavoriteFailed'), { level: 'error' })
    }
  }

  function handleRenameKeyDown(e: KeyboardEvent, volume: VolumeInfo) {
    // The focused rename `<input>` owns every keystroke. Stop ALL keys from
    // bubbling to the pane's DOM listeners (Space-selection, type-to-jump,
    // etc.); the dispatch-level guards don't cover the raw DOM Space handler,
    // so without this a Space typed into the box also selects the file under
    // the cursor. Enter commits, Escape cancels, everything else edits the text.
    e.stopPropagation()
    if (e.key === 'Enter') {
      e.preventDefault()
      void commitRename(volume)
    } else if (e.key === 'Escape') {
      e.preventDefault()
      cancelRename()
    }
  }

  /** Local-first reorder: show the new order instantly via the optimistic override, then persist in
   *  the background. On failure, drop the override so the UI reverts to the store truth. */
  function applyReorder(orderedLocationIds: string[]) {
    optimisticFavoriteIds = orderedLocationIds
    void reorderFavorites(orderedLocationIds.map(stripFavoritePrefix)).catch(() => {
      addToast(tString('fileExplorer.navigation.reorderFavoritesFailed'), { level: 'error' })
      optimisticFavoriteIds = null
    })
  }

  return {
    get optimisticFavoriteIds() {
      return optimisticFavoriteIds
    },
    get renamingFavoriteId() {
      return renamingFavoriteId
    },
    get renameDraft() {
      return renameDraft
    },
    set renameDraft(value: string) {
      renameDraft = value
    },
    remove,
    startRename,
    cancelRename,
    commitRename,
    handleRenameKeyDown,
    applyReorder,
  }
}
