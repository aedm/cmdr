/**
 * The switcher's "Use Cmdr's fast direct connection" row on each SMB share: what it
 * reads, and what a pick does.
 *
 * The choice is per share and lives in Rust (`src-tauri/src/network/DETAILS.md` § "The
 * per-share direct-connection switch"), asked for by volume id. Rust also does what
 * switching OFF means (a direct share goes back to the macOS mount right away, and the
 * `volumes-changed` push repaints the dot). Switching ON only saves there, so a share
 * still on the OS mount then goes through the ONE "Connect directly" flow, with its
 * sign-in sheet and toasts.
 */

import { SvelteMap } from 'svelte/reactivity'
import { getSmbDirectConnectionEnabled, setSmbDirectConnectionEnabled } from '$lib/tauri-commands'
import { getAppLogger } from '$lib/logging/logger'
import { volumeKindOf } from '../pane/volume-capabilities'
import { connectDirectlyToRow } from './connect-directly-row'
import type { VolumeInfo } from '../types'

const log = getAppLogger('fileExplorer')

export interface DirectConnectionSwitches {
  /** The share's switch, or `undefined` when the row has none (not an SMB share, or not asked yet). */
  valueFor: (volumeId: string) => boolean | undefined
  /** Asks Rust about every SMB share row. Call when the switcher opens. */
  fetchForRows: (volumes: VolumeInfo[]) => Promise<void>
  /** Flips the share's switch, from the value the row showed. */
  pick: (volume: VolumeInfo, volumes: VolumeInfo[]) => Promise<void>
}

/** Whether a row is an SMB share, so it's worth asking Rust for its switch. Rust has the last word. */
function isSmbShareRow(volume: VolumeInfo): boolean {
  return volumeKindOf(volume.id, volume.fsType, volume.category) === 'smb'
}

export function createDirectConnectionSwitches(): DirectConnectionSwitches {
  const values = new SvelteMap<string, boolean>()

  async function fetchOne(volumeId: string): Promise<void> {
    try {
      const value = await getSmbDirectConnectionEnabled(volumeId)
      if (value === null) values.delete(volumeId)
      else values.set(volumeId, value)
    } catch (e) {
      log.warn('Reading the direct-connection switch for {volumeId} broke down: {error}', {
        volumeId,
        error: String(e),
      })
    }
  }

  async function pick(volume: VolumeInfo, volumes: VolumeInfo[]): Promise<void> {
    // A row shows a switch only once its value is known, and it defaults to on.
    const enable = !(values.get(volume.id) ?? true)
    let answer
    try {
      answer = await setSmbDirectConnectionEnabled(volume.id, enable)
    } catch (e) {
      log.warn('Switching the direct connection for {volumeId} broke down: {error}', {
        volumeId: volume.id,
        error: String(e),
      })
      return
    }
    if (answer === 'notAnSmbShare' || answer === 'mountNotResponding') {
      // The share went away or its mount stopped answering, and nothing was saved. The
      // row goes with the share, or the pane already shows the silent mount.
      log.info("The direct-connection switch for {volumeId} wasn't saved: {answer}", { volumeId: volume.id, answer })
      return
    }
    values.set(volume.id, enable)
    if (enable && volume.connectionState === 'os_mount') {
      await connectDirectlyToRow(volume.id, volumes)
    }
  }

  return {
    valueFor: (volumeId) => values.get(volumeId),
    fetchForRows: async (volumes) => {
      await Promise.all(volumes.filter(isSmbShareRow).map((volume) => fetchOne(volume.id)))
    },
    pick,
  }
}
