/**
 * Whether the switcher's Network group has grown long enough to teach the user
 * how to shorten it.
 *
 * Pure, because the decision is the part worth pinning down: the seam that
 * observes the counts and raises the toast is `$lib/stores/volume-store`.
 */

/** How many pinned places make the group long enough to be worth a word. */
export const PIN_HINT_AT = 5

/** The switcher as the hint sees it. */
export interface PinHintInputs {
  /** Server places pinned to the switcher right now. */
  pinnedCount: number
  /** `behavior.serversPinHintSeen`: whether this has already been said once. */
  seen: boolean
}

/**
 * Whether to raise the hint.
 *
 * ❗ "At least five", ❌ not "the fifth one just landed": nothing records what
 * the count was last launch, and a person whose list was already long is exactly
 * who the hint is for. The once-ever flag is what keeps it from nagging.
 */
export function shouldShowPinHint({ pinnedCount, seen }: PinHintInputs): boolean {
  if (seen) return false
  return pinnedCount >= PIN_HINT_AT
}
