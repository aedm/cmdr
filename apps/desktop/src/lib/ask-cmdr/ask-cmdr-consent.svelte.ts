/**
 * Ask Cmdr consent gate: the opt-in that must precede any message reaching a provider
 * (spec §2.1 privacy line; plan §12). The record lives in `main.db` (agent state, not a
 * preference), read/written through the backend commands.
 *
 * The rail gates on {@link consentState}.accepted: while `null` (unknown/loading) it shows
 * nothing, `false` shows the consent screen, `true` shows the chat. Both the rail's consent
 * screen and the settings section drive the same accept/revoke here, so the two surfaces
 * stay in sync (each refreshes on mount / open).
 */

import { getAppLogger } from '$lib/logging/logger'
import {
  acceptAskCmdrConsent,
  askCmdrConsentStatus,
  revokeAskCmdrConsent,
  type AskCmdrConsentStatus,
} from '$lib/tauri-commands'

const log = getAppLogger('askCmdr')

interface ConsentState {
  /** `null` = not yet known (loading); `true`/`false` = the current-version opt-in. */
  accepted: boolean | null
  /** Unix secs the user last accepted the current copy, or `null`. */
  acceptedAt: number | null
  /**
   * The user opted in once, to copy that has since changed materially, so the bump revoked
   * them. Without this, someone with a whole thread history behind the consent screen looks
   * exactly like someone who never wanted AI, and both surfaces say a bare "off" at them.
   */
  needsReconsent: boolean
}

/**
 * What an accept or revoke came to. `notSaved` means the person's choice isn't what the store
 * holds, so the caller has to say so (or try again): ❌ never let it pass as `done`, because a
 * silently kept consent is a "no" that didn't stick.
 */
export type ConsentOutcome = 'done' | 'notSaved'

export const consentState = $state<ConsentState>({ accepted: null, acceptedAt: null, needsReconsent: false })

function apply(status: AskCmdrConsentStatus): void {
  consentState.accepted = status.accepted
  consentState.acceptedAt = status.accepted ? status.acceptedAt : null
  consentState.needsReconsent = !status.accepted && status.acceptedVersion !== null
}

/** Refresh the cached consent status from the store. Called on rail open and settings mount. */
export async function refreshConsent(): Promise<void> {
  try {
    apply(await askCmdrConsentStatus())
  } catch (e) {
    log.warn('reading consent status failed: {error}', { error: String(e) })
    // Fail closed: an unreadable status keeps the gate shut rather than opening it.
    consentState.accepted = false
    consentState.acceptedAt = null
    consentState.needsReconsent = false
  }
}

/**
 * Record the opt-in (turn Ask Cmdr on) and refresh. `done` only when the store now reads
 * accepted: a store that never opened takes the write as a no-op and still reads "off".
 */
export async function acceptConsent(): Promise<ConsentOutcome> {
  try {
    await acceptAskCmdrConsent()
  } catch (e) {
    log.warn('recording consent failed: {error}', { error: String(e) })
  }
  await refreshConsent()
  return consentState.accepted === true ? 'done' : 'notSaved'
}

/**
 * Turn Ask Cmdr off (clear consent) and refresh. Chats are kept. `notSaved` when the store
 * refused the write; the status is re-read either way, so the surfaces show what it holds.
 */
export async function revokeConsent(): Promise<ConsentOutcome> {
  let outcome: ConsentOutcome = 'done'
  try {
    await revokeAskCmdrConsent()
  } catch (e) {
    log.warn('turning Ask Cmdr off failed: {error}', { error: String(e) })
    outcome = 'notSaved'
  }
  await refreshConsent()
  return outcome
}
