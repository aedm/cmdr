/**
 * Ask Cmdr consent gate: the opt-in that must precede any message reaching a provider
 * (spec §2.1 privacy line; plan §12). The record lives in `main.db` (agent state, not a
 * preference), read/written through the backend commands.
 *
 * The rail gates on {@link consentState}.accepted: while `null` (unknown/loading) it shows
 * nothing, `false` shows the consent screen, `true` shows the chat. Both the rail's consent
 * screen and the settings section drive the same accept/revoke here, so the two surfaces
 * stay in sync (each refreshes on mount / open).
 *
 * **A "no" the store refused is HELD, not dropped.** When onboarding's revoke is refused twice,
 * {@link holdConsentRevoke} records the answer in `settings.json` (`askCmdr.consentRevokePending`),
 * which every Rust consent gate reads, so the "no" holds at once. {@link settleHeldConsentRevoke}
 * retries the store on every refresh and at launch, and lets go once it lands.
 */

import { getAppLogger } from '$lib/logging/logger'
import { forceSave, getSetting, setSetting } from '$lib/settings'
import {
  acceptAskCmdrConsent,
  askCmdrConsentRevokePendingChanged,
  askCmdrConsentStatus,
  revokeAskCmdrConsent,
  type AskCmdrConsentStatus,
} from '$lib/tauri-commands'

const log = getAppLogger('askCmdr')

/** The hidden setting that holds a refused "no" until `main.db` takes it. */
const HELD_REVOKE = 'askCmdr.consentRevokePending'

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
  // While a "no" is held the backend reads not-accepted, but the store's audit still names the
  // version they once accepted. That's a "no", not an opt-in paused by changed wording.
  consentState.needsReconsent = !status.accepted && status.acceptedVersion !== null && !getSetting(HELD_REVOKE)
}

/**
 * Sets or lets go of the held "no", saves `settings.json` NOW (the Rust gates read the file, and
 * the store's usual save is debounced), then tells the gates to re-read. Resolves to whether the
 * save landed.
 */
async function setHeldRevoke(held: boolean): Promise<boolean> {
  setSetting(HELD_REVOKE, held)
  const saved = await forceSave()
  try {
    await askCmdrConsentRevokePendingChanged()
  } catch (e) {
    log.warn('telling the consent gates about a held "no" failed: {error}', { error: String(e) })
  }
  return saved
}

/**
 * Hold a "no" the store refused, so every consent gate reads it as "no consent" from the next
 * check on. Resolves to whether `settings.json` took it.
 */
export async function holdConsentRevoke(): Promise<boolean> {
  return setHeldRevoke(true)
}

/**
 * Retry a held "no" against the store, and let go of it once the store takes it. A no-op when
 * nothing is held. Runs on every {@link refreshConsent} and once at launch.
 */
export async function settleHeldConsentRevoke(): Promise<void> {
  if (!getSetting(HELD_REVOKE)) return
  try {
    await revokeAskCmdrConsent()
  } catch (e) {
    log.warn("a held 'no' to Ask Cmdr still can't reach the store, so it stays held: {error}", {
      error: String(e),
    })
    return
  }
  if (!(await setHeldRevoke(false))) {
    log.warn("the store took a held 'no', but settings.json wouldn't let go of it; the next refresh retries")
  }
}

/** Refresh the cached consent status from the store. Called on rail open and settings mount. */
export async function refreshConsent(): Promise<void> {
  await settleHeldConsentRevoke()
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
 *
 * A deliberate yes lets go of any held "no" FIRST, or the next refresh would revoke it again.
 */
export async function acceptConsent(): Promise<ConsentOutcome> {
  if (getSetting(HELD_REVOKE)) await setHeldRevoke(false)
  try {
    await acceptAskCmdrConsent()
  } catch (e) {
    log.warn('recording consent failed: {error}', { error: String(e) })
  }
  await refreshConsent()
  return consentState.accepted === true ? 'done' : 'notSaved'
}

/**
 * Turn Ask Cmdr off as a person's answer, wherever they gave it (Settings' Turn off, onboarding's
 * "no AI"). ❌ Every "no" goes through here, never a bare {@link revokeConsent}: a "no" means no
 * wherever it was said.
 *
 * Revokes, gives a refusal one more try, and holds the "no" when the store refuses both, so every
 * consent gate reads it from the next check on. `done` when the "no" holds (recorded, or held for
 * the store); `notSaved` only when neither the store nor `settings.json` took it.
 */
export async function declineConsent(): Promise<ConsentOutcome> {
  if ((await revokeConsent()) === 'done' || (await revokeConsent()) === 'done') return 'done'
  if (await holdConsentRevoke()) {
    log.warn("the store refused to turn Ask Cmdr off twice; holding the 'no' until it takes it")
    await refreshConsent()
    return 'done'
  }
  log.warn(
    "the store refused to turn Ask Cmdr off twice, and settings.json wouldn't hold the 'no' either; consent stays recorded",
  )
  return 'notSaved'
}

/**
 * Turn Ask Cmdr off (clear consent) and refresh. Chats are kept. `notSaved` when the store
 * refused the write; the status is re-read either way, so the surfaces show what it holds.
 * One attempt, no hold: a person's "no" goes through {@link declineConsent}.
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
