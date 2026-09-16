/**
 * Unit tests for the favorites interaction controller behind the volume switcher.
 *
 * This file uses Svelte runes (`$effect.root`) to instantiate the rune-based factory outside a
 * component, so the filename carries the `.svelte.test.ts` suffix. The component-level behavior
 * (the rename keyboard guard, the native row menu's picks) is pinned by
 * `VolumeBreadcrumb.svelte.test.ts` and `pane/volume-breadcrumb.test.ts`; here we cover rename,
 * remove, and the local-first order directly.
 *
 * ❗ The reorder MECHANICS live in the house `Menu` primitive now
 * (`$lib/ui/menu-controller.svelte.test.ts`: the drag threshold, the drop-line cue, ⌥↑/⌥↓, and
 * carrying the cursor with the moved row). What stays here is the half the primitive has no
 * business knowing: which ids the backend wants, and what to show while it answers.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { tick } from 'svelte'

const reorderFavorites = vi.fn(() => Promise.resolve())
const removeFavorite = vi.fn(() => Promise.resolve())
const renameFavorite = vi.fn(() => Promise.resolve())
const addToast = vi.fn(() => 'toast-id')

vi.mock('$lib/tauri-commands', () => ({
  reorderFavorites: (...args: unknown[]) => reorderFavorites(...(args as [])),
  removeFavorite: (...args: unknown[]) => removeFavorite(...(args as [])),
  renameFavorite: (...args: unknown[]) => renameFavorite(...(args as [])),
  stripFavoritePrefix: (id: string) => (id.startsWith('fav-') ? id.slice(4) : id),
}))

vi.mock('$lib/ui/toast', () => ({ addToast: (...args: unknown[]) => addToast(...(args as [])) }))

import { createFavoritesController } from './favorites-controller.svelte'
import type { VolumeInfo } from '../types'

function fav(id: string, name = id): VolumeInfo {
  return { id, name, path: `/Users/test/${name}`, category: 'favorite', isEjectable: false }
}

describe('favorites-controller', () => {
  let dispose: (() => void) | undefined
  let favorites: VolumeInfo[]
  let renameInput: HTMLInputElement | undefined

  function create(initialFavorites: VolumeInfo[]) {
    favorites = [...initialFavorites]
    renameInput = document.createElement('input')
    let controller!: ReturnType<typeof createFavoritesController>
    dispose = $effect.root(() => {
      controller = createFavoritesController({
        getFavorites: () => favorites,
        getVolumes: () => favorites,
        getRenameInputRef: () => renameInput,
      })
    })
    return controller
  }

  beforeEach(() => {
    vi.clearAllMocks()
  })

  afterEach(() => {
    dispose?.()
    dispose = undefined
  })

  describe('reorder', () => {
    it('persists the settled order with bare ids, and shows it at once', () => {
      const c = create([fav('fav-1'), fav('fav-2'), fav('fav-3')])
      c.applyReorder(['fav-2', 'fav-1', 'fav-3'])
      expect(reorderFavorites).toHaveBeenCalledTimes(1)
      expect(reorderFavorites).toHaveBeenCalledWith(['2', '1', '3'])
      // Optimistic order set synchronously, so the list re-renders before the round-trip.
      expect(c.optimisticFavoriteIds).toEqual(['fav-2', 'fav-1', 'fav-3'])
    })

    it('reverts the optimistic order when the background persist rejects', async () => {
      reorderFavorites.mockRejectedValueOnce(new Error('nope'))
      const c = create([fav('fav-1'), fav('fav-2'), fav('fav-3')])
      c.applyReorder(['fav-2', 'fav-1', 'fav-3'])
      expect(c.optimisticFavoriteIds).toEqual(['fav-2', 'fav-1', 'fav-3'])
      await tick()
      await Promise.resolve()
      expect(c.optimisticFavoriteIds).toBe(null)
      expect(addToast).toHaveBeenCalledWith("Couldn't reorder favorites. Try again?", { level: 'error' })
    })
  })

  describe('rename', () => {
    it('startRename focuses and selects the rename input after a tick', async () => {
      const c = create([fav('fav-1', 'Docs')])
      const focusSpy = vi.spyOn(renameInput as HTMLInputElement, 'focus')
      const selectSpy = vi.spyOn(renameInput as HTMLInputElement, 'select')
      c.startRename(favorites[0])
      expect(c.renamingFavoriteId).toBe('fav-1')
      expect(c.renameDraft).toBe('Docs')
      await tick()
      expect(focusSpy).toHaveBeenCalled()
      expect(selectSpy).toHaveBeenCalled()
    })

    it('commitRename persists the trimmed new name with the bare id, then clears state', async () => {
      const c = create([fav('fav-1', 'Docs')])
      c.startRename(favorites[0])
      c.renameDraft = '  Projects  '
      await c.commitRename(favorites[0])
      expect(renameFavorite).toHaveBeenCalledWith('1', 'Projects')
      expect(c.renamingFavoriteId).toBe(null)
      expect(c.renameDraft).toBe('')
    })

    it('commitRename skips the IPC when the name is unchanged', async () => {
      const c = create([fav('fav-1', 'Docs')])
      c.startRename(favorites[0])
      await c.commitRename(favorites[0])
      expect(renameFavorite).not.toHaveBeenCalled()
    })

    it('cancelRename clears the draft without persisting', () => {
      const c = create([fav('fav-1', 'Docs')])
      c.startRename(favorites[0])
      c.cancelRename()
      expect(c.renamingFavoriteId).toBe(null)
      expect(renameFavorite).not.toHaveBeenCalled()
    })

    it('handleRenameKeyDown stops propagation for every key and commits on Enter', () => {
      const c = create([fav('fav-1', 'Docs')])
      c.startRename(favorites[0])
      c.renameDraft = 'New'
      const enter = new KeyboardEvent('keydown', { key: 'Enter' })
      const stop = vi.spyOn(enter, 'stopPropagation')
      c.handleRenameKeyDown(enter, favorites[0])
      expect(stop).toHaveBeenCalled()
      expect(renameFavorite).toHaveBeenCalledWith('1', 'New')
    })

    it('handleRenameKeyDown cancels on Escape', () => {
      const c = create([fav('fav-1', 'Docs')])
      c.startRename(favorites[0])
      c.handleRenameKeyDown(new KeyboardEvent('keydown', { key: 'Escape' }), favorites[0])
      expect(c.renamingFavoriteId).toBe(null)
    })
  })

  describe('remove', () => {
    it('calls removeFavorite with the bare id', async () => {
      const c = create([fav('fav-1')])
      await c.remove(favorites[0])
      expect(removeFavorite).toHaveBeenCalledWith('1')
    })

    it('shows a toast when removal rejects', async () => {
      removeFavorite.mockRejectedValueOnce(new Error('nope'))
      const c = create([fav('fav-1')])
      await c.remove(favorites[0])
      expect(addToast).toHaveBeenCalledWith("Couldn't remove that favorite. Try again?", { level: 'error' })
    })
  })
})
