/**
 * Waiting an instant mutation (new folder, new file, rename) out to its real end.
 *
 * The backend answers within a short deadline. Past it the reply is
 * `stillRunning` with a `pendingId`, and the `mutation-settled` event carrying
 * that id says how the work ended: it may well land seconds later, so this is
 * never a failure. Backend side: `src-tauri/src/file_system/write_operations/mutation_reply.rs`.
 */

import { events, type MutationError, type MutationReply, type MutationSettledOutcome } from '$lib/ipc/bindings'
import { throwMutationError } from '$lib/file-operations/mutation-error'

/** One mutation command's typed reply, as the bindings return it. */
type MutationCall = () => Promise<{ status: 'ok'; data: MutationReply } | { status: 'error'; error: MutationError }>

export interface MutationWaitOptions {
  /**
   * Called once when the volume hasn't answered within the backend's reply
   * deadline. The wait goes on; this is the moment to tell the person it's slow.
   */
  onStillRunning?: () => void
}

/**
 * Runs `call` and resolves when the mutation landed, or throws its typed
 * refusal (`MutationFailure`), however long that takes.
 *
 * The listener goes up BEFORE the call: the backend settles from another task,
 * so the event can overtake the reply, and a listener added after it would wait
 * forever. Settles that arrive first wait in `early` until the reply names the id.
 */
export async function awaitMutation(call: MutationCall, options?: MutationWaitOptions): Promise<void> {
  const early = new Map<string, MutationSettledOutcome>()
  let waiting: { pendingId: string; resolve: (outcome: MutationSettledOutcome) => void } | null = null
  const unlisten = await events.mutationSettled.listen((event) => {
    const { pendingId, outcome } = event.payload
    if (waiting?.pendingId === pendingId) waiting.resolve(outcome)
    else early.set(pendingId, outcome)
  })
  try {
    const res = await call()
    if (res.status === 'error') throwMutationError(res.error)
    const reply = res.data
    if (reply.type === 'done') return
    options?.onStillRunning?.()
    const outcome =
      early.get(reply.pendingId) ??
      (await new Promise<MutationSettledOutcome>((resolve) => {
        waiting = { pendingId: reply.pendingId, resolve }
      }))
    if (outcome.type === 'refused') throwMutationError(outcome.error)
  } finally {
    unlisten()
  }
}
