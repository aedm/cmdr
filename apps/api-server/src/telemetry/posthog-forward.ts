/**
 * Forwards the desktop app's feature events to PostHog, after `/heartbeat` has stored them in D1.
 *
 * The app never talks to PostHog itself: events ride the heartbeat to our server, D1 keeps them, and
 * this module passes them on so PostHog's history stays continuous while nothing reads the D1 copy
 * yet. **This is the whole PostHog dependency.** Dropping PostHog means deleting this file, its test,
 * and the one `forwardEventsToPostHog` call in `heartbeat.ts`, then removing the
 * `POSTHOG_PROJECT_KEY` secret; nothing else knows PostHog exists.
 *
 * The body mirrors what the app used to send to `/capture/` one event at a time, so events look the
 * same in PostHog before and after the relay: `distinct_id` is the `anal_` install id, every event
 * carries `source`, `app_version`, `os_version`, and `arch` (injected last, so an event property of
 * the same name can't shadow them), and the config snapshot rides as `$set` person properties. Two
 * things are new because the caller changed: each event carries its own client `timestamp` (it may
 * reach us hours after it fired) and the app version that produced it, a `uuid` from the client's event
 * id so PostHog drops the repeats of a retried beat, and `$geoip_disable` is on, because the IP PostHog sees is our
 * Worker's, and we don't need a country per install anyway. We don't forward the user's IP.
 *
 * Best effort by design: runs in `waitUntil` after the beat is acknowledged, and never throws. A
 * failed forward loses those events in PostHog only; D1 still has them.
 */

/** PostHog's batch ingestion endpoint on the EU cloud, the region the project (`136072`) lives in. */
const posthogBatchUrl = 'https://eu.i.posthog.com/batch/'

/** How long one forward may take. `waitUntil` work gets 30 s after the response; this stays well inside. */
const forwardTimeoutMs = 10_000

/** Who sent the events: the beat's install id plus the platform it ran on. The version is per event. */
export interface ForwardIdentity {
  analId: string
  osVersion: string
  arch: string
}

/** One event as `/heartbeat` stored it: a validated name, a UTC timestamp, and its own properties. */
export interface ForwardEvent {
  event: string
  timestamp: string
  properties: Record<string, unknown>
  /** The build that produced the event, which can be older than the beat that carried it. */
  appVersion: string
  /** The client's per-event UUID, forwarded as `uuid` so PostHog drops a retried beat's repeats. */
  id: string | null
}

interface PostHogBatchEntry {
  event: string
  distinct_id: string
  timestamp: string
  uuid?: string
  properties: Record<string, unknown>
}

interface PostHogBatchBody {
  api_key: string
  batch: PostHogBatchEntry[]
}

/** Builds the `/batch/` request body. Pure, so the shape is tested without a network. */
export function buildPostHogBatch(
  apiKey: string,
  identity: ForwardIdentity,
  events: ForwardEvent[],
  config: Record<string, unknown> | null,
): PostHogBatchBody {
  return {
    api_key: apiKey,
    batch: events.map((e) => ({
      event: e.event,
      distinct_id: identity.analId,
      timestamp: e.timestamp,
      ...(e.id ? { uuid: e.id } : {}),
      properties: {
        ...e.properties,
        source: 'desktop',
        app_version: e.appVersion,
        os_version: identity.osVersion,
        arch: identity.arch,
        $geoip_disable: true,
        ...(config ? { $set: config } : {}),
      },
    })),
  }
}

/**
 * Sends the events to PostHog in one request. A no-op without a key (the secret is unset, for
 * example in local dev) or without events. Resolves in every case; failures are logged.
 */
export async function forwardEventsToPostHog(
  apiKey: string | undefined,
  identity: ForwardIdentity,
  events: ForwardEvent[],
  config: Record<string, unknown> | null,
): Promise<void> {
  if (!apiKey || events.length === 0) return
  try {
    const response = await fetch(posthogBatchUrl, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(buildPostHogBatch(apiKey, identity, events, config)),
      signal: AbortSignal.timeout(forwardTimeoutMs),
    })
    if (!response.ok) {
      console.warn(
        `PostHog forward: ${String(events.length)} events refused with HTTP ${String(response.status)}; D1 still has them`,
      )
    }
  } catch (e) {
    console.warn(`PostHog forward: ${String(events.length)} events not sent (${String(e)}); D1 still has them`)
  }
}
