/**
 * Volume selection by index / name for a pane — the MCP `select_volume` tool and
 * the palette's volume commands. Lifted out of `DualPaneExplorer`; the component
 * keeps the one-line `export function selectVolumeByName` delegate.
 *
 * Both routes fold onto `navigate({ to: { selectVolume }, source: 'user' })`, so
 * the standard volume-switch mechanics (focus shift, history push, new-tab-on-
 * pinned) apply uniformly. Matches `VolumeBreadcrumb`'s `handleVolumeSelect`: a
 * favorite navigates to its path on the containing volume; a real volume opens
 * where `pathForPickedVolume` says (a saved server place on its start folder,
 * anything else at its root); the virtual servers-hub volume isn't in the
 * volumes list, so it's special-cased. The switch arm shifts STORE focus but not DOM focus — re-
 * anchoring the container would drop a Space press during the multi-select-then-
 * delete sequence (regression guard: mtp.spec.ts).
 */

import { resolvePathVolume } from '$lib/tauri-commands'
import { tString } from '$lib/intl/messages.svelte'
import { getAppLogger } from '$lib/logging/logger'
import { reportFavoriteOpened } from '../navigation/favorites-analytics'
import { pathForPickedVolume } from '../navigation/picked-volume-path'
import type { VolumeInfo } from '../types'
import type { NavigateIntent, NavigateResult } from './navigate'

const log = getAppLogger('fileExplorer')

export interface VolumeSelectionDeps {
  getVolumes: () => VolumeInfo[]
  navigate: (intent: NavigateIntent) => NavigateResult
}

/**
 * What a volume select did. `selected` names the volume the pane was sent to (a
 * favorite's CONTAINING volume) and carries `navigate()`'s result, whose `corrected`
 * says when the switch's destination is final: MCP `select_volume` waits on both to
 * report where the pane came to rest.
 */
export type VolumeSelectOutcome =
  | { kind: 'not-found' }
  | { kind: 'selected'; volumeId: string; navigation: NavigateResult }

export interface VolumeSelection {
  /** Select a volume by zero-based index into the volumes array. */
  selectVolumeByIndex: (pane: 'left' | 'right', index: number) => Promise<VolumeSelectOutcome>
  /** Select a volume by name (MCP `select_volume`). The servers hub is virtual. */
  selectVolumeByName: (pane: 'left' | 'right', name: string) => Promise<VolumeSelectOutcome>
}

export function createVolumeSelection(deps: VolumeSelectionDeps): VolumeSelection {
  function select(pane: 'left' | 'right', volumeId: string, path: string): VolumeSelectOutcome {
    const navigation = deps.navigate({ pane, to: { selectVolume: { volumeId, path } }, source: 'user' })
    return { kind: 'selected', volumeId, navigation }
  }

  async function selectVolumeByIndex(pane: 'left' | 'right', index: number): Promise<VolumeSelectOutcome> {
    const volumes = deps.getVolumes()
    if (index < 0 || index >= volumes.length) {
      log.warn('Invalid volume index: {index} (valid range: 0-{max})', { index, max: volumes.length - 1 })
      return { kind: 'not-found' }
    }

    const volume = volumes[index]

    // Handle favorites differently from actual volumes (same as VolumeBreadcrumb).
    if (volume.category === 'favorite') {
      reportFavoriteOpened('command')
      // For favorites, navigate to the favorite's path on its containing volume.
      const { volume: containingVolume } = await resolvePathVolume(volume.path)
      return select(pane, containingVolume?.id ?? 'root', volume.path)
    }
    // A saved server place opens on its start folder; anything else at its root.
    return select(pane, volume.id, pathForPickedVolume(volume))
  }

  async function selectVolumeByName(pane: 'left' | 'right', name: string): Promise<VolumeSelectOutcome> {
    // ❗ The servers hub row is SYNTHETIC: `volume-grouping.ts` builds it, so it
    // is not in the volume list and no `findIndex` can reach it. Its name comes
    // from the catalog rather than a literal, so the label, the MCP pane push,
    // and Rust's `volume_listing::SERVERS_VOLUME_NAME` stay one word.
    if (name === tString('fileExplorer.navigation.networkVolume')) {
      return select(pane, 'network', 'smb://')
    }

    const index = deps.getVolumes().findIndex((v) => v.name === name)
    if (index !== -1) {
      return selectVolumeByIndex(pane, index)
    }

    log.warn('Volume not found: {name}', { name })
    return { kind: 'not-found' }
  }

  return { selectVolumeByIndex, selectVolumeByName }
}
