/**
 * A pane whose listing failed lists its volume again the moment that volume is live,
 * by whatever route it got there (a connect, a kernel mount that finished after a
 * Cancel, the reconnect manager), with no timing assumptions.
 *
 * ❗ The race it closes (final QA, 4/4): a Cancel within ~20 ms of picking a saved
 * share left the pane on "Not connected yet /Volumes/public" while the kernel mount
 * finished and the header dot turned green, and only re-picking recovered it. Chasing
 * that one timing would leave the next one; this makes "failed, then live" heal
 * whichever way it happens.
 *
 * ❗ Once per live spell: a listing that fails again while the volume stays live is a
 * real error, and retrying it on every refresh would be a loop. A spell ends when the
 * volume stops being live, and the next one may try again.
 */

import type { VolumeInfo } from '../types'
import { isLiveSession } from '../navigation/connection-state'

export interface LiveRetryDeps {
  /** The pane's volume as the volume list has it now. */
  getVolumeInfo: () => VolumeInfo | null
  /** Whether the pane shows a listing error. */
  hasListingError: () => boolean
  /** Lists the pane's folder again. */
  retry: () => void
}

export function createLiveRetry(deps: LiveRetryDeps): void {
  /** The live spell already retried in: the volume id and the path it's live at. */
  let retriedIn: string | null = null

  $effect(() => {
    const info = deps.getVolumeInfo()
    if (!info || !isLiveSession(info.connectionState)) {
      retriedIn = null
      return
    }
    if (!deps.hasListingError()) return
    const spell = `${info.id}:${info.path}`
    if (retriedIn === spell) return
    retriedIn = spell
    deps.retry()
  })
}
