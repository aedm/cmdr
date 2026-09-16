import { connectDirectly } from '../network/direct-connect'
import type { VolumeInfo } from '../types'

/**
 * Hand a switcher row's volume to the direct-connect flow (`../network/DETAILS.md` §
 * "Connect directly"), which owns the whole thing: stored credentials, the saved-password
 * probe, the login sheet, and every toast along the way.
 *
 * ❗ The share's NAME is read HERE, while the row still lists it: it's what words the
 * answer if the share goes away before the backend gets there.
 */
export async function connectDirectlyToRow(volumeId: string, volumes: VolumeInfo[]): Promise<void> {
  const shareName = volumes.find((volume) => volume.id === volumeId)?.name ?? volumeId
  await connectDirectly({ volumeId, shareName })
}
