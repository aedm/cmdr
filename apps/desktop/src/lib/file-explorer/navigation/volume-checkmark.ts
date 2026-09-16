import type { VolumeInfo } from '../types'

/**
 * Whether a switcher row shows the active checkmark.
 *
 * ❗ It answers against `containingVolumeId` (`resolvePathVolume(currentPath)`), ❌ never the
 * pane's `volumeId` prop, which is virtual for a favorite. Favorites never carry a checkmark:
 * a path can sit inside several of them at once, so a mark there would claim more than it knows.
 */
export function shouldShowCheckmark(volume: VolumeInfo, containingVolumeId: string | null): boolean {
  if (volume.category === 'favorite') return false
  return volume.id === containingVolumeId
}
