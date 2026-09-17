import { ejectVolume } from '$lib/tauri-commands'
import { isVolumeBusy, isVolumeEjecting } from '$lib/stores/volume-busy-store.svelte'
import { addToast } from '$lib/ui/toast'
import { tString } from '$lib/intl/messages.svelte'
import { wordEjectRefusal } from './eject-error-messages'
import { disconnectServerPlace } from './server-row-actions'
import type { DetachAction } from './detach-control'
import type { VolumeInfo } from '../types'

/**
 * Runs the action `detachControlFor` chose for this volume.
 *
 * ❗ Both surfaces go through here, so the button that says Disconnect can't send an
 * eject: the words and the action come from one answer and are spent in one place.
 */
export async function runDetach(volume: VolumeInfo, action: DetachAction): Promise<void> {
  if (action === 'disconnect-place') {
    if (isVolumeBusy(volume.id)) return
    await disconnectServerPlace(volume.id, volume.name)
    return
  }
  await detachVolume(volume)
}

/**
 * Press the eject-or-disconnect control: the chip's and every switcher row's run this.
 *
 * ❗ It guards again on its own. The controls render disabled while a transfer touches the
 * volume or its eject is still running, but a keyboard or edge path could still reach here:
 * don't tear down a volume mid-transfer, and don't ask twice (the backend would only join
 * the eject already running).
 *
 * Success says nothing: the volume leaves the list on its own through `volume-unmounted` /
 * `mtp-device-disconnected`, which is the feedback. A refusal speaks the catalog through
 * `wordEjectRefusal`, ❌ never `diskutil`'s stderr.
 */
export async function detachVolume(volume: VolumeInfo): Promise<void> {
  if (isVolumeBusy(volume.id) || isVolumeEjecting(volume.id)) return
  try {
    await ejectVolume(volume.id)
  } catch (e) {
    addToast(tString('fileExplorer.pane.ejectFailedToast', { volumeName: volume.name, message: wordEjectRefusal(e) }), {
      level: 'error',
    })
  }
}
