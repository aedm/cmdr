/** Carrying a typed `AddServerError` from `connectToServer` to the add sheet's attempt. */
import type { AddServerError } from '$lib/ipc/bindings'
import { TypedFailure, failureOf } from '$lib/ipc/typed-failure'

/** An `Error` that still carries why adding an SMB host didn't go through. */
export class AddServerFailure extends TypedFailure<AddServerError> {
  constructor(failure: AddServerError) {
    super(failure, `add refused: ${failure.type}`)
    this.name = 'AddServerFailure'
  }
}

/** Throws a wire `AddServerError` as an `Error`, keeping the typed value. */
export function throwAddServerError(failure: AddServerError): never {
  throw new AddServerFailure(failure)
}

/** The typed refusal behind a caught value, or `null` when it isn't one. */
export function asAddServerError(error: unknown): AddServerError | null {
  return failureOf(AddServerFailure, error)
}
