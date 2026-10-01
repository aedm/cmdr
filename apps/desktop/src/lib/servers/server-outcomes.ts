/**
 * The ONE place a backend `ServerConnectOutcome` or `SavedServerOutcome` becomes
 * the app's own words.
 *
 * ❗ Exhaustive over the wire enums, so a new outcome fails to COMPILE here rather
 * than falling into a default arm that words it as something else. Two readers of
 * this switch would be two chances to word one outcome differently, which is why
 * `connect-flow.ts` folds this result rather than re-reading the wire.
 *
 * The two host-key arms keep their payloads: the sheet's key step needs a
 * fingerprint to show, and a refusal sentence has nowhere to put one.
 */

import type { SavedServerOutcome } from '$lib/ipc/bindings'
import type { ServerConnectOutcome } from '$lib/tauri-commands'
import type { ConnectRefusalKind } from './connect-refusals'
import type { SignInAttemptOutcome } from './sign-in-contract'

/** How one dial ended. Everything a dial can answer, minus the caller-only hand-off. */
export type ServerDialOutcome = Exclude<SignInAttemptOutcome, { kind: 'handed_off' } | { kind: 'added' }>

/** One dial's answer, in the app's own vocabulary. */
export function readConnectOutcome(outcome: ServerConnectOutcome): ServerDialOutcome {
  switch (outcome.outcome) {
    case 'connected':
      return { kind: 'connected', volumeId: outcome.volumeId }
    case 'cancelled':
      return { kind: 'cancelled' }
    case 'needs_host_key_approval':
      return { kind: 'needs_host_key', prompt: outcome }
    case 'host_key_revoked':
      return { kind: 'host_key_revoked', key: outcome }
    case 'authentication_rejected':
      return { kind: 'refused', refusal: 'authentication_rejected' }
    case 'needs_credentials':
      return { kind: 'refused', refusal: 'needs_credentials' }
    case 'auth_method_unsupported':
      return { kind: 'refused', refusal: 'auth_method_unsupported' }
    case 'certificate_untrusted':
      return { kind: 'refused', refusal: 'certificate_untrusted' }
    case 'not_a_webdav_server':
      return { kind: 'refused', refusal: 'not_a_webdav_server' }
    case 'invalid_url':
      return { kind: 'refused', refusal: 'invalid_url' }
    case 'start_folder_outside_root':
      return { kind: 'refused', refusal: 'start_folder_outside_root' }
    case 'timed_out':
      return { kind: 'refused', refusal: 'timed_out' }
    case 'unreachable':
      return { kind: 'refused', refusal: 'unreachable' }
    case 'access_denied':
      return { kind: 'refused', refusal: 'access_denied' }
    case 'bucket_list_refused':
      return { kind: 'refused', refusal: 'bucket_list_refused' }
    case 'bucket_not_found':
      return { kind: 'refused', refusal: 'bucket_not_found' }
    case 'region_mismatch':
      // ❗ The region rides along: "this bucket is in us-east-2" is the fix, and the
      // bare kind can only say "another region".
      return outcome.region
        ? { kind: 'refused', refusal: 'region_mismatch', region: outcome.region }
        : { kind: 'refused', refusal: 'region_mismatch' }
    case 'clock_skewed':
      return { kind: 'refused', refusal: 'clock_skewed' }
    case 'not_an_s3_endpoint':
      return { kind: 'refused', refusal: 'not_an_s3_endpoint' }
  }
}

/** How saving an edit ended: written, or refused with nothing written. */
export type SaveOutcome = { kind: 'saved' } | { kind: 'refused'; refusal: ConnectRefusalKind }

/**
 * One save's answer, in the app's own vocabulary.
 *
 * ❗ The backend's `unreachable` reads as `save_unconfirmed`, ❌ not the dial's
 * `unreachable`: nothing was saved, and the address that sentence points at is
 * locked in edit mode.
 */
export function readSavedServerOutcome(outcome: SavedServerOutcome): SaveOutcome {
  switch (outcome.outcome) {
    case 'saved':
      return { kind: 'saved' }
    case 'start_folder_outside_root':
      return { kind: 'refused', refusal: 'start_folder_outside_root' }
    case 'root_not_found':
      return { kind: 'refused', refusal: 'root_not_found' }
    case 'start_folder_not_found':
      return { kind: 'refused', refusal: 'start_folder_not_found' }
    case 'unreachable':
      return { kind: 'refused', refusal: 'save_unconfirmed' }
  }
}

/**
 * Whether the backend is waiting on a PERSON rather than reporting a dead end.
 *
 * ❗ `auth_method_unsupported` is deliberately out: the server challenged with a
 * scheme Cmdr doesn't speak, the secret never left, and no typing fixes it.
 * Opening a password box over it would ask for something that cannot help.
 *
 * S3's `access_denied` is IN: a bucket that turns a key away can mean a wrong
 * secret (Garage answers one that way), so the sheet, with the sentence asking
 * about both, is the one place a fix can be typed. `bucket_list_refused` is OUT:
 * the way past it is a bucket place, which no secret typed here creates.
 */
export function needsAHuman(outcome: ServerDialOutcome): boolean {
  return (
    outcome.kind === 'needs_host_key' ||
    (outcome.kind === 'refused' &&
      (outcome.refusal === 'needs_credentials' ||
        outcome.refusal === 'authentication_rejected' ||
        outcome.refusal === 'access_denied'))
  )
}
