/** Carrying a typed `KeychainError` from a server's secret write to the sheet that words it. */
import type { KeychainError } from '$lib/ipc/bindings'
import { TypedFailure } from '$lib/ipc/typed-failure'

/**
 * An `Error` that still carries the secret store's typed refusal.
 *
 * Real causes: someone clicked Deny on the Keychain prompt, the keychain is
 * locked, or Linux has no secret service. The diagnostic keeps the variant AND
 * the store's own words (a framework or io message, ❌ never the secret), which
 * `throwIpcError` would have cut down to the words alone.
 */
export class KeychainFailure extends TypedFailure<KeychainError> {
  constructor(failure: KeychainError) {
    super(failure, `Keychain refused: ${failure.type} (${failure.message})`)
    this.name = 'KeychainFailure'
  }
}

/** Throws a wire `KeychainError` as an `Error`, keeping the typed value. */
export function throwKeychainError(failure: KeychainError): never {
  throw new KeychainFailure(failure)
}
