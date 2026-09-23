import { getNumberFormatter } from '$lib/intl/number-format'
import { formatDriveSize } from '$lib/units'

/**
 * The low-space warning's two figures, shared by the in-app toast and the macOS notification: the
 * free space as precise as the drive's size makes worth reading (the same figure the status bar
 * shows), and the free percent to a tenth, both in the active locale.
 *
 * An unknown total (0) reads as 100% free, mirroring the backend's `free_percent`, so a bogus
 * fetch can't render a nonsense percentage.
 */
export function lowSpaceFigures(availableBytes: number, totalBytes: number): { freeText: string; percentText: string } {
  const freePercent = totalBytes === 0 ? 100 : (availableBytes / totalBytes) * 100
  return {
    freeText: formatDriveSize(availableBytes, totalBytes),
    percentText: getNumberFormatter({ minimumFractionDigits: 1, maximumFractionDigits: 1 }).format(freePercent),
  }
}
