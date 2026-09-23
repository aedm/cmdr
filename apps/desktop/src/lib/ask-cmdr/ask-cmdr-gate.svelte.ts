/**
 * What the Ask Cmdr rail shows in place of the chat, if anything. Two gates, in order:
 *
 * 1. `off`: Ask Cmdr's own switch (`askCmdr.enabled`) is off. The rail offers to turn it on.
 * 2. `cloudOff`: the AI mode is Cloud and "Allow cloud AI" isn't on. The rail points at the
 *    switch in Settings > AI > Provider; it never grants consent itself.
 *
 * `loading` is Cloud while consent isn't known yet: the rail renders nothing, so neither a
 * gate nor the chat flashes. Local and "off" need no consent (the composer says when no
 * provider is set up). The backend enforces both gates on every send; this only decides what
 * the rail draws.
 */

import { cloudConsentState, refreshCloudConsent } from '$lib/ai/cloud-consent.svelte'
import { getSetting, type AiProvider } from '$lib/settings'

export type RailGate = 'loading' | 'off' | 'cloudOff' | 'chat'

/** The pure decision, so the rail can feed it reactive values and tests can pin the order. */
export function railGate(input: { enabled: boolean; provider: AiProvider; cloudConsent: boolean | null }): RailGate {
  if (!input.enabled) return 'off'
  if (input.provider !== 'cloud') return 'chat'
  if (input.cloudConsent === null) return 'loading'
  return input.cloudConsent ? 'chat' : 'cloudOff'
}

/** Re-read cloud consent from the store, then answer the gate for the current settings. */
export async function refreshRailGate(): Promise<RailGate> {
  await refreshCloudConsent()
  return railGate({
    enabled: getSetting('askCmdr.enabled'),
    provider: getSetting('ai.provider'),
    cloudConsent: cloudConsentState.accepted,
  })
}
