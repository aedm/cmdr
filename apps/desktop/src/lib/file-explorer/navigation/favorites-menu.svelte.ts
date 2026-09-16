/**
 * Everything the favorites menu is, minus its DOM: the rows it offers, the `0` row's
 * three states, what a pick does, and the three edits a favorite takes (rename inline,
 * remove, reorder).
 *
 * `FavoritesMenu.svelte` builds the house `Menu` around this and renders it. ❗ The
 * reorder MECHANICS (the drag threshold, the drop-line cue, ⌥↑/⌥↓, carrying the cursor
 * with the moved row) are the primitive's, which hands the settled order here through
 * `onReorder`. This module's half is what the primitive deliberately has no business
 * knowing: which ids the backend wants, and what to show while it answers.
 */

import { tick } from 'svelte'
import { removeFavorite, renameFavorite, reorderFavorites, stripFavoritePrefix } from '$lib/tauri-commands'
import { addToast } from '$lib/ui/toast'
import { tString } from '$lib/intl/messages.svelte'
import { isMacOS } from '$lib/shortcuts/key-capture'
import { isRestricted } from '$lib/stores/restricted-paths-store.svelte'
import type { MenuActivationSource, MenuIcon, MenuItem, MenuSection } from '$lib/ui/menu-types'
import type { VolumeContextActionKind } from '$lib/ipc/bindings'
import { paneFolderCanBeFavorited } from '../pane/volume-capabilities'
import type { VolumeChangePayload } from '../pane/types'
import type { VolumeInfo } from '../types'
import { addFavoriteFolder } from './add-favorite-folder'
import { buildFavoriteTooltip } from './favorite-tooltip'
import { openFavorite } from './open-favorite'
import type { FavoriteOpenedEvent } from './favorites-analytics'

/**
 * What a row carries back on a pick. A discriminated union rather than an optional
 * `volume`, so the add row can't be mistaken for a favorite that lost its data.
 */
export type FavoritesRow = { kind: 'favorite'; volume: VolumeInfo } | { kind: 'add' }

/** The add row's `value`. A `fav-` id can never collide with it. */
export const ADD_ROW_VALUE = 'favorites:add'

/** The reorderable section holding the favorites themselves, named for `onReorder`. */
export const FAVORITES_SECTION_ID = 'favorites'

/**
 * How many rows get a number. Past nine there is no single digit left to give, so those
 * favorites are listed with a blank number column and reached by arrow or pointer.
 */
const NUMBERED_FAVORITES = 9

export interface FavoritesMenuDeps {
  /** The whole volume list; the favorites are filtered out of it, in store order. */
  getVolumes: () => VolumeInfo[]
  /** The pane's volume id and folder: what the `0` row acts on, and what gates it. */
  getPaneVolumeId: () => string
  getPaneCurrentPath: () => string
  /** The generic folder icon, for a favorite whose own icon isn't fetched (FDA-gated paths). */
  getDirIconFallback: () => string | undefined
  /** The inline rename `<input>`, focused + selected when a rename starts. */
  getRenameInputRef: () => HTMLInputElement | undefined
  /** The last mile of opening a favorite: put the pane on the containing volume. */
  go: (target: VolumeChangePayload) => void
}

export interface FavoritesMenuController {
  /** The two sections the `Menu` renders: the favorites, then the add row. */
  get sections(): MenuSection<FavoritesRow>[]
  /** The favorites in display order (the optimistic override applied). */
  get favorites(): VolumeInfo[]
  get renamingFavoriteId(): string | null
  get renameDraft(): string
  set renameDraft(value: string)
  /** True while the inline editor owns every keystroke, so the primitive handles none. */
  isEditing: () => boolean
  /** Carry out a pick. `source` is the primitive's, and becomes the analytics `via`. */
  select: (item: MenuItem<FavoritesRow>, source: MenuActivationSource) => Promise<void>
  /** Take the order the menu just settled on (drag or ⌥↑/⌥↓) and persist it. */
  applyReorder: (orderedLocationIds: string[]) => void
  /** A pick from a favorite's native row menu, once the caller has checked its menu is open. */
  handleContextAction: (payload: { action: VolumeContextActionKind; volumeId: string }) => void
  cancelRename: () => void
  commitRename: (volume: VolumeInfo) => Promise<void>
  handleRenameKeyDown: (event: KeyboardEvent, volume: VolumeInfo) => void
}

/** Trailing slashes aside, the same folder. Matches how the store dedupes an add. */
function samePath(a: string, b: string): boolean {
  return a.replace(/\/+$/, '') === b.replace(/\/+$/, '')
}

/** The primitive says HOW a row was activated; the analytics contract words it its own way. */
function viaOf(source: MenuActivationSource): Extract<FavoriteOpenedEvent, { surface: 'favorites_menu' }>['via'] {
  switch (source) {
    case 'accelerator':
      return 'digit'
    case 'keyboard':
      return 'keyboard'
    case 'pointer':
      return 'pointer'
  }
}

export function createFavoritesMenu(deps: FavoritesMenuDeps): FavoritesMenuController {
  // Optimistic favorite order for an instant, local-first reorder. A keyboard (⌥↑/⌥↓) or
  // pointer reorder sets this to the new order of favorite ids SYNCHRONOUSLY, so the menu
  // re-renders immediately and a rapid next press computes against fresh state; the backend
  // persist runs in the background. Reconciled to `null` once `volumes-changed` brings the
  // store to the same order (or the favorite set changes elsewhere). `null` = render the
  // store order.
  let optimisticFavoriteIds = $state<string[] | null>(null)

  // ── Inline rename ────────────────────────────────────────────────────
  let renamingFavoriteId = $state<string | null>(null)
  let renameDraft = $state('')

  const storeFavorites = $derived(deps.getVolumes().filter((volume) => volume.category === 'favorite'))

  const favorites = $derived.by(() => {
    const order = optimisticFavoriteIds
    if (!order) return storeFavorites
    // A linear `indexOf` per row: a favorites list is a handful of entries, so the lookup
    // table a `Map` would buy costs more than it saves.
    const rank = (id: string) => {
      const index = order.indexOf(id)
      return index === -1 ? Number.POSITIVE_INFINITY : index
    }
    return storeFavorites.slice().sort((a, b) => rank(a.id) - rank(b.id))
  })

  // Drop the optimistic order once the store catches up to it (the persisted
  // `volumes-changed` landed), or if the favorite set changed elsewhere (add / remove) so
  // the override is stale.
  $effect(() => {
    const order = optimisticFavoriteIds
    if (!order) return
    const storeFavIds = storeFavorites.map((volume) => volume.id)
    const sameSet = storeFavIds.length === order.length && storeFavIds.every((id) => order.includes(id))
    const sameOrder = sameSet && storeFavIds.every((id, index) => id === order[index])
    if (sameOrder || !sameSet) optimisticFavoriteIds = null
  })

  /**
   * Why the `0` row can't be picked, or `null` when it can.
   *
   * ❗ Capability first: on an archive or `.git`-portal pane the folder could never be a
   * favorite at all, so "already a favorite" would be answering a question that doesn't
   * arise. Re-adding a folder that IS one is refused rather than allowed because the store
   * answers a duplicate by moving that favorite to the end of the list, which looks like
   * the row jumping for no reason.
   */
  const addRefusal = $derived.by(() => {
    const path = deps.getPaneCurrentPath()
    if (!paneFolderCanBeFavorited(deps.getPaneVolumeId(), path)) {
      return tString('fileExplorer.navigation.favoritesCantAddHere')
    }
    if (favorites.some((favorite) => samePath(favorite.path, path))) {
      return tString('fileExplorer.navigation.favoritesAlreadyAdded')
    }
    return null
  })

  function favoriteIcon(volume: VolumeInfo): MenuIcon {
    // TCC-denied paths: `NSWorkspace.iconForFile` returns a confusing "no access"
    // placeholder, so the generic Aqua folder icon stands in.
    const fallback = deps.getDirIconFallback()
    if (isRestricted(volume.path) && fallback) return { src: fallback }
    if (volume.icon) return { src: volume.icon }
    if (fallback) return { src: fallback }
    return { lucide: 'folder' }
  }

  function favoriteItem(volume: VolumeInfo, index: number): MenuItem<FavoritesRow> {
    return {
      value: volume.id,
      label: volume.name,
      icon: favoriteIcon(volume),
      // 1-indexed, and only while a single digit is left to give.
      accelerator: index < NUMBERED_FAVORITES ? String(index + 1) : undefined,
      // The PATH leads, so a renamed favorite still reveals where it points.
      tooltip: buildFavoriteTooltip(volume.path, isMacOS()),
      data: { kind: 'favorite', volume },
    }
  }

  const sections = $derived<MenuSection<FavoritesRow>[]>([
    {
      id: FAVORITES_SECTION_ID,
      heading: tString('fileExplorer.navigation.groupFavorites'),
      reorderable: true,
      // An emptied list is a real user state (they can remove every favorite), so the
      // section still reads as itself rather than vanishing.
      emptyLabel: tString('fileExplorer.navigation.favoritesEmpty'),
      items: favorites.map(favoriteItem),
    },
    {
      id: 'add',
      items: [
        {
          value: ADD_ROW_VALUE,
          label: tString('fileExplorer.navigation.favoritesAddCurrent'),
          icon: { lucide: 'plus' },
          accelerator: '0',
          disabled: addRefusal !== null,
          // The reason IS the tooltip: a greyed row that says nothing is a dead end.
          tooltip: addRefusal ?? undefined,
          data: { kind: 'add' },
        },
      ],
    },
  ])

  async function select(item: MenuItem<FavoritesRow>, source: MenuActivationSource): Promise<void> {
    const row = item.data
    if (!row) return
    if (row.kind === 'add') {
      await addFavoriteFolder(deps.getPaneCurrentPath())
      return
    }
    // `open-favorite.ts` owns the whole favorite open: resolve the containing volume,
    // emit, and switch onto it. A favorite that resolves to no volume leaves the pane
    // where it is.
    await openFavorite({
      favoritePath: row.volume.path,
      picked: { surface: 'favorites_menu', via: viaOf(source) },
      go: deps.go,
    })
  }

  async function remove(volume: VolumeInfo): Promise<void> {
    try {
      await removeFavorite(stripFavoritePrefix(volume.id))
    } catch {
      addToast(tString('fileExplorer.navigation.removeFavoriteFailed'), { level: 'error' })
    }
  }

  function startRename(volume: VolumeInfo): void {
    renamingFavoriteId = volume.id
    renameDraft = volume.name
    void tick().then(() => {
      const input = deps.getRenameInputRef()
      input?.focus()
      input?.select()
    })
  }

  function cancelRename(): void {
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

  function handleRenameKeyDown(event: KeyboardEvent, volume: VolumeInfo): void {
    // The focused rename `<input>` owns every keystroke. Stop ALL keys from bubbling to
    // the pane's DOM listeners (Space-selection, type-to-jump, etc.); the dispatch-level
    // guards don't cover the raw DOM Space handler, so without this a Space typed into the
    // box also selects the file under the cursor. Enter commits, Escape cancels,
    // everything else edits the text.
    event.stopPropagation()
    if (event.key === 'Enter') {
      event.preventDefault()
      void commitRename(volume)
    } else if (event.key === 'Escape') {
      event.preventDefault()
      cancelRename()
    }
  }

  function handleContextAction(payload: { action: VolumeContextActionKind; volumeId: string }): void {
    if (payload.action !== 'rename-favorite' && payload.action !== 'remove-favorite') return
    const volume = favorites.find((favorite) => favorite.id === payload.volumeId)
    if (!volume) return
    if (payload.action === 'rename-favorite') startRename(volume)
    else void remove(volume)
  }

  /**
   * Local-first reorder: show the new order instantly via the optimistic override, then
   * persist in the background. On failure, drop the override so the UI reverts to the
   * store truth.
   */
  function applyReorder(orderedLocationIds: string[]): void {
    optimisticFavoriteIds = orderedLocationIds
    void reorderFavorites(orderedLocationIds.map(stripFavoritePrefix)).catch(() => {
      addToast(tString('fileExplorer.navigation.reorderFavoritesFailed'), { level: 'error' })
      optimisticFavoriteIds = null
    })
  }

  return {
    get sections() {
      return sections
    },
    get favorites() {
      return favorites
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
    isEditing: () => renamingFavoriteId !== null,
    select,
    applyReorder,
    handleContextAction,
    cancelRename,
    commitRename,
    handleRenameKeyDown,
  }
}
