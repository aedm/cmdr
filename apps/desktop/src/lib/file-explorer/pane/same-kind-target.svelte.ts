/**
 * What `selection.selectSameKind` would select right now, published by the
 * FOCUSED pane so surfaces outside the explorer can label the command with it.
 *
 * The command palette is the only reader today: its row says "Select all with
 * extension *.pdf" rather than the static "Select all of the same kind", so a
 * user sees the effect before pressing Enter. The registry's `displayName`
 * resolver reads `sameKindCommandLabel()` at render time, and `CommandPalette`'s
 * `$derived` re-renders when the cursor moves, since the value below is `$state`.
 *
 * ❗ The COMMAND never reads this. It re-reads the cursor row and the pane's
 * whole-listing snapshot when it runs, so a label that is a frame behind can't
 * change what gets selected. This is a label, not a decision.
 *
 * Only the focused pane writes: `FilePane`'s effect bails when `isFocused` is
 * false, so the blurred pane can't clobber the value on a pane switch (both
 * effects re-run, and only one of them writes, whatever order they run in).
 */

import { tString } from '$lib/intl/messages.svelte'
import type { SameKindTarget } from './select-same-kind'

let target = $state<SameKindTarget | null>(null)

/**
 * Publish the focused pane's current target. Call it with `null` for a row with
 * no kind (the `..` row, an empty listing, a cursor entry still resolving).
 */
export function publishSameKindTarget(next: SameKindTarget | null): void {
  target = next
}

/** The focused pane's target, or `null`. Reactive. */
export function getSameKindTarget(): SameKindTarget | null {
  return target
}

/**
 * What the command palette calls `selection.selectSameKind` right now. Falls
 * back to the command's static name when the cursor row implies no target, which
 * is also what Settings > Shortcuts and the help window always show.
 */
export function sameKindCommandLabel(): string {
  const current = target
  if (!current) return tString('commands.selectionSelectSameKind.label')
  switch (current.kind) {
    case 'allFolders':
      return tString('commands.selectionSelectSameKind.allFolders')
    case 'noExtension':
      return tString('commands.selectionSelectSameKind.noExtension')
    case 'sameExtension':
      return tString('commands.selectionSelectSameKind.sameExtension', { extension: current.extension })
  }
}

/** Test-only reset back to "no target", so one spec can't leak a label into the next. */
export function _resetForTesting(): void {
  target = null
}
