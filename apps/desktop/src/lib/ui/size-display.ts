import { formatSizeForDisplay } from '$lib/file-explorer/selection/selection-info-utils'
import { getFileSizeFormat } from '$lib/settings/reactive-settings.svelte'

/**
 * The tiered parts `<Size>` renders: the friendly dynamic unit, independent of the user's
 * `listing.sizeUnit` choice (that setting is for the file list's Size column, where
 * apples-to-apples comparison matters). Exported so a caller that needs the same TEXT
 * without the markup (a column measuring its widest cell) stays on one format.
 *
 * `rounded` is the live form: a tenth below ten, whole units above ("1.7 GB", "24 GB",
 * not "1.70 GB"), for a readout that changes several times a second.
 */
export function sizeDisplayParts(bytes: number, rounded = false): { value: string; tierClass: string }[] {
  return formatSizeForDisplay(bytes, { unit: 'dynamic', format: getFileSizeFormat(), rounded })
}
