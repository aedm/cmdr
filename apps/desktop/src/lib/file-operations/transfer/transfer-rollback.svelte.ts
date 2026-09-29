/**
 * The progress dialog's one reversal decision: whether Rollback works right
 * now, what it promises, and the question asked before it happens.
 *
 * It's a controller rather than a component because the decision surfaces in
 * two places that can't share a subtree: the Rollback button sits in the
 * dialog's button row (`TransferRollbackControls.svelte`), while the
 * confirmation stacks OVER the dialog and must stay reachable from the conflict
 * body too, which replaces that row. `../DETAILS.md` § "Rollback asks first".
 */

import type { OpKind, WriteOperationPhase } from '$lib/ipc/bindings'
import { tString } from '$lib/intl/messages.svelte'
import {
  inFlightRollbackTooltipKey,
  inFlightRollbackVariant,
  reversalWindowClosed,
  type RollbackConfirmVariant,
} from '../reversal-wording'

export interface TransferRollbackInputs {
  opKind: () => OpKind
  phase: () => WriteOperationPhase | null
  /** The props-only rule for the frames before the first registry snapshot:
   *  a move within one non-default volume is a server-side rename-merge that
   *  can't reverse. */
  isSameVolumeMove: () => boolean
  /** `supportsRollback` off the operation's registry row, inverted. */
  rollbackUnavailable: () => boolean
  operationSettled: () => boolean
  /** Runs the confirmed rollback (`progress.handleCancel(true)`). */
  rollBack: () => void
}

export interface TransferRollback {
  /** What rolling THIS operation back does to the files: a copy's reversal
   *  deletes what it wrote, a move's carries it back. ❌ Never a fixed
   *  `stopAndDelete`, which pushes people off a move's harmless reversal. */
  readonly variant: RollbackConfirmVariant
  /** What the live button promises, the whole difference from Cancel. */
  readonly liveTooltip: string
  /** Why Rollback is switched off right now, or `null` while it really works. */
  readonly blockedTooltip: string | null
  /** The confirmation is up. Withdrawn once the operation settles (nothing
   *  left to undo) and the moment Rollback becomes blocked. */
  readonly confirming: boolean
  /** Asks the question, unless Rollback is blocked. Both entry points (the
   *  button and the conflict body) come through here. */
  request: () => void
  confirm: () => void
  dismiss: () => void
}

export function createTransferRollback(inputs: TransferRollbackInputs): TransferRollback {
  let asked = $state(false)
  const variant = $derived(inFlightRollbackVariant(inputs.opKind()))
  const liveTooltip = $derived(tString(inFlightRollbackTooltipKey(variant)))
  /** Two reasons, and they answer different halves of the question:
   *
   *  - the operation's STRATEGY can't reverse at all (`supportsRollback` off its
   *    registry row, the authority wherever it has arrived and an adopted view's
   *    only source, with the same-volume-move rule beside it for the frames
   *    before the first snapshot lands),
   *  - or it could, and the moment has passed: a move between filesystems on its
   *    source-deletion phase has already landed every file
   *    (`../reversal-wording.ts`).
   *
   *  Either way the plain Cancel stays live and stays accurate, which is what
   *  the second tooltip points at. */
  const blockedTooltip = $derived.by(() => {
    if (inputs.isSameVolumeMove() || inputs.rollbackUnavailable())
      return tString('fileOperations.transferProgress.rollbackUnavailableTooltip')
    if (reversalWindowClosed(inputs.opKind(), inputs.phase()))
      return tString('fileOperations.transferProgress.rollbackAlreadyLandedTooltip')
    return null
  })

  return {
    get variant() {
      return variant
    },
    get liveTooltip() {
      return liveTooltip
    },
    get blockedTooltip() {
      return blockedTooltip
    },
    get confirming() {
      return asked && !inputs.operationSettled() && blockedTooltip === null
    },
    request() {
      if (blockedTooltip === null) asked = true
    },
    confirm() {
      asked = false
      inputs.rollBack()
    },
    dismiss() {
      asked = false
    },
  }
}
