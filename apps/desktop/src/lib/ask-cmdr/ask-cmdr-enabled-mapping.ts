/**
 * The one-time `askCmdr.enabled` mapping for installs that predate the switch, run as a
 * main-window startup step (`lib/ask-cmdr/DETAILS.md` § Gates, cost, and settings).
 *
 * Before the switch, Ask Cmdr was "on" when its own opt-in was recorded in `main.db`. That
 * record grants nothing now (cloud consent is `$lib/ai/cloud-consent.svelte.ts`), but it still
 * says who wanted Ask Cmdr: someone who accepted keeps it on (on Cloud it then waits for "Allow
 * cloud AI"), and someone who saw the opt-in and didn't take it, or held a "no", stays off.
 * Turning it on for them would turn a "not yet" into a yes, proactive loop included.
 *
 * Idempotent, and it writes at most once: it runs only while the switch was never set
 * explicitly, and only after onboarding finished (a fresh install gets the switch from its
 * AI pick instead). An unreadable store writes nothing, so the next launch asks again. Not a
 * settings schema migration: those run before the agent store is guaranteed open, and can't
 * retry once `_schemaVersion` is stamped.
 */

import { getAppLogger } from '$lib/logging/logger'
import { getSetting, isExplicitlySet, setSetting } from '$lib/settings'
import { askCmdrLegacyOptIn } from '$lib/tauri-commands'

const log = getAppLogger('askCmdr')

export async function mapLegacyAskCmdrOptIn(): Promise<void> {
  if (isExplicitlySet('askCmdr.enabled')) return
  if (!getSetting('onboarding.completed')) return
  let answer
  try {
    answer = await askCmdrLegacyOptIn()
  } catch (e) {
    log.warn("couldn't read the old Ask Cmdr opt-in, so the switch waits for the next launch: {error}", {
      error: String(e),
    })
    return
  }
  if (answer === 'storeUnavailable') {
    log.info("the agent store isn't open, so the Ask Cmdr switch waits for the next launch")
    return
  }
  setSetting('askCmdr.enabled', answer === 'recorded')
}
