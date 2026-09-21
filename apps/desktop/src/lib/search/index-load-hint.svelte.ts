/**
 * Whether the dialog should say out loud that it's waiting for a volume's search arena.
 *
 * `search-lifecycle.svelte.ts` asks the backend to load the arena for whichever volume
 * this session will search, and records the wait as `pendingIndexVolumeId`. That wait is
 * the one stretch of the dialog's life nothing else speaks for: `QueryResults` only
 * shows its loading state once a run has been attempted, and `CoverageNote` only answers
 * after one has come back. This module fills exactly that hole, and only when the wait
 * is long enough to be worth a sentence.
 *
 * Deliberately NOT a "no index here" voice: a volume that can't be indexed sets no
 * pending volume at all (`warmVolume` passes `null` unless the backend promised a
 * `search-index-ready`), so that case falls through here in silence and stays
 * `CoverageNote`'s to answer, with a run's evidence behind it.
 */

import { getPendingIndexVolumeId } from './search-state.svelte'

/**
 * How long a load may run before the dialog mentions it.
 *
 * An arena load is commonly well under a second, and a line that appears and vanishes
 * inside a couple of frames reads as a flicker rather than an explanation: it costs the
 * reader more than the silence it replaced. So the hint is for the waits a person
 * actually notices, and the quick ones pass unremarked.
 */
export const INDEX_LOAD_HINT_DELAY_MS = 500

export interface IndexLoadHint {
  /** Whether the hint has earned its place on screen right now. */
  readonly visible: boolean
}

/**
 * Creates the hint's clock. Call it during component init (it owns an `$effect`), the
 * way `SearchDialog.svelte` does with the other Search factories.
 */
export function createIndexLoadHint(): IndexLoadHint {
  /** The volume being waited for, or `null` when nothing is coming. */
  const pendingVolumeId = $derived(getPendingIndexVolumeId())

  /** Whether THIS wait has outlasted the threshold. */
  let waited = $state(false)

  // One timer per wait. The cleanup is what keeps a switch to another volume, a landed
  // arena, and the dialog closing from all leaving a timer behind that would light the
  // hint up over something nobody is waiting for any more.
  $effect(() => {
    const volumeId = pendingVolumeId
    waited = false
    if (volumeId === null) return
    const timer = setTimeout(() => {
      waited = true
    }, INDEX_LOAD_HINT_DELAY_MS)
    return () => {
      clearTimeout(timer)
    }
  })

  // Reading the pending volume here too, rather than trusting the effect's reset alone:
  // effects run AFTER the DOM updates, so the arena landing would otherwise leave the
  // hint on screen for one frame past the thing it describes.
  const visible = $derived.by(() => waited && pendingVolumeId !== null)

  return {
    get visible() {
      return visible
    },
  }
}
