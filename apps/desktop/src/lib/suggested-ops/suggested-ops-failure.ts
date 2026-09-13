/** Carrying a typed `SuggestedOpsError` from a suggested-ops command to the dialog that words it. */
import type { SuggestedOpsError } from '$lib/ipc/bindings'
import { TypedFailure } from '$lib/ipc/typed-failure'

/** An `Error` that still carries the backend's typed suggested-ops refusal. */
export class SuggestedOpsFailure extends TypedFailure<SuggestedOpsError> {
  constructor(failure: SuggestedOpsError) {
    // The `detail` is what SQLite or the runtime reported. `throwIpcError` kept only the
    // variant name, so a log line said "store" and nothing about why.
    const diagnostic =
      'detail' in failure
        ? `suggested ops refused: ${failure.type} (${failure.detail})`
        : `suggested ops refused: ${failure.type}`
    super(failure, diagnostic)
    this.name = 'SuggestedOpsFailure'
  }
}

/** Throws a wire `SuggestedOpsError` as an `Error`, keeping the typed value. */
export function throwSuggestedOpsError(failure: SuggestedOpsError): never {
  throw new SuggestedOpsFailure(failure)
}
