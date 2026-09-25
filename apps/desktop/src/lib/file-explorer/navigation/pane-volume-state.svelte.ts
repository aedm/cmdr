/**
 * The volume a pane is on, live: `paneVolumeOf` fed with the containing volume
 * the backend resolves for the pane's path.
 *
 * ❗ ONE instance per pane, owned by `FilePane.svelte`, so the header, the
 * switcher's checkmark, the first-connect index prompt, and the status bar's
 * disk space all read the same answer. Two resolutions could disagree for as long
 * as one of them was in flight, and the status bar kept a share's figure on the
 * boot disk that way (QA round 5, R4-1).
 */

import { resolvePathVolume } from '$lib/tauri-commands'
import { dependOn } from '$lib/utils/reactivity'
import type { VolumeInfo } from '../types'
import { paneVolumeOf } from './pane-volume'

/** Views with no volume behind them: they show no disk space and no switcher checkmark by path. */
const VIRTUAL_VOLUME_IDS = new Set(['network', 'search-results'])

export interface PaneVolumeStateDeps {
  getVolumes: () => readonly VolumeInfo[]
  getVolumeId: () => string
  getCurrentPath: () => string
}

export interface PaneVolumeState {
  /** The volume the backend says holds the pane's path, or `null` until it answers. */
  readonly containingVolumeId: string | null
  /** The volume the pane is on (`paneVolumeOf`), or `undefined` for a view with none. */
  readonly volume: VolumeInfo | undefined
}

export function createPaneVolumeState(deps: PaneVolumeStateDeps): PaneVolumeState {
  let containingVolumeId = $state<string | null>(null)
  /** Bumped per question, so an answer for a path the pane has left is dropped. */
  let asked = 0

  // Re-ask when the path changes, and when the volume list does: a share that
  // mounts back at the same path turns that folder from the boot disk's into its own.
  $effect(() => {
    dependOn(deps.getVolumes())
    const path = deps.getCurrentPath()
    const volumeId = deps.getVolumeId()
    if (VIRTUAL_VOLUME_IDS.has(volumeId)) return
    const question = ++asked
    void resolvePathVolume(path).then(({ volume }) => {
      if (question === asked) containingVolumeId = volume?.id ?? volumeId
    })
  })

  const volume = $derived.by(() => {
    const volumeId = deps.getVolumeId()
    if (VIRTUAL_VOLUME_IDS.has(volumeId)) return undefined
    return paneVolumeOf(deps.getVolumes(), volumeId, deps.getCurrentPath(), containingVolumeId)
  })

  return {
    get containingVolumeId() {
      return containingVolumeId
    },
    get volume() {
      return volume
    },
  }
}
