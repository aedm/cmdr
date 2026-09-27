/**
 * Which of the Servers list's optional columns fit at a pane width. A narrow pane drops
 * Address first (a WebDAV URL is the widest cell and the least needed), then Status,
 * ❌ never squeezing them to zero width, where they vanished with no ellipsis.
 */

/** Below this, Address goes. */
const ADDRESS_MIN_PX = 481
/** Below this, Status goes too. */
const STATUS_MIN_PX = 361

/** The columns shown at `widthPx`, the list's content width; `0` is unmeasured, so everything shows. */
export function hubColumnsAt(widthPx: number): { address: boolean; status: boolean } {
  if (widthPx <= 0) return { address: true, status: true }
  return { address: widthPx >= ADDRESS_MIN_PX, status: widthPx >= STATUS_MIN_PX }
}
