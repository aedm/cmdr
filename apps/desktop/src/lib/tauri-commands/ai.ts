// AI lifecycle event listeners. Typed `on*` wrappers over the `tauri-specta`
// `events.ai*` helpers. The install flow emits these in sequence
// (`ai-extracting` → repeated `ai-download-progress` → `ai-verifying` →
// `ai-installing` → `ai-install-complete`); `ai-starting` / `ai-server-ready`
// bracket a server boot on a returning launch.

import { type UnlistenFn } from '@tauri-apps/api/event'
import { commands, events, type CloudAiConsentStatus, type DownloadProgress } from '$lib/ipc/bindings'
import { throwIpcError } from './ipc-types'

export type { CloudAiConsentStatus }

/** Model-download progress (bytes, total, speed, ETA), throttled to ~200 ms. */
export function onAiDownloadProgress(handler: (payload: DownloadProgress) => void): Promise<UnlistenFn> {
  return events.aiDownloadProgress.listen((event) => {
    handler(event.payload)
  })
}

/** The local server is starting up (boot on a returning launch). */
export function onAiStarting(handler: () => void): Promise<UnlistenFn> {
  return events.aiStarting.listen(() => {
    handler()
  })
}

/** The local server became healthy and ready. */
export function onAiServerReady(handler: () => void): Promise<UnlistenFn> {
  return events.aiServerReady.listen(() => {
    handler()
  })
}

/** Post-download file-size verification started. */
export function onAiVerifying(handler: () => void): Promise<UnlistenFn> {
  return events.aiVerifying.listen(() => {
    handler()
  })
}

/** Server startup (health-check polling) began. */
export function onAiInstalling(handler: () => void): Promise<UnlistenFn> {
  return events.aiInstalling.listen(() => {
    handler()
  })
}

/** The server is healthy and the install completed. */
export function onAiInstallComplete(handler: () => void): Promise<UnlistenFn> {
  return events.aiInstallComplete.listen(() => {
    handler()
  })
}

/** Binary extraction from the bundled archive started (usually instant). */
export function onAiExtracting(handler: () => void): Promise<UnlistenFn> {
  return events.aiExtracting.listen(() => {
    handler()
  })
}

// ── Cloud AI consent ────────────────────────────────────────────────────────────
// The "Allow cloud AI" record lives in `main.db`, behind these commands. The backend enforces it
// in `ai::manager::resolve_backend` for every cloud call; the frontend only reads it to render the
// switch and the gates. State and the held "no" live in `$lib/ai/cloud-consent.svelte.ts`.

/** Whether the user allowed cloud AI. A missing or unreadable store reads as not accepted. */
export async function cloudAiConsentStatus(): Promise<CloudAiConsentStatus> {
  return commands.cloudAiConsentStatus()
}

/**
 * Record "Allow cloud AI". Throws when the store didn't take it (unavailable or refused).
 *
 * ❌ Only the switch's own click may reach this, through `$lib/ai/cloud-consent.svelte.ts`,
 * whose accept only `AiCloudConsentToggle.svelte` imports (`cloud-consent-call-sites.test.ts`).
 */
export async function acceptCloudAiConsent(): Promise<void> {
  const res = await commands.acceptCloudAiConsent()
  if (res.status === 'error') throwIpcError(res.error)
}

/** Turn cloud AI off and stop every in-flight cloud call. Throws when the store refused the clear. */
export async function revokeCloudAiConsent(): Promise<void> {
  const res = await commands.revokeCloudAiConsent()
  if (res.status === 'error') throwIpcError(res.error)
}

/** Tell the cloud gates a held "no" (`ai.cloudConsentRevokePending`) was just set or let go of. */
export async function cloudAiConsentRevokePendingChanged(): Promise<void> {
  await commands.cloudAiConsentRevokePendingChanged()
}

/**
 * Every consent write and held-"no" change, from any window. No payload: re-read the status.
 * Every window that shows a cloud gate subscribes, so the Settings switch and the main window's
 * gates stay in step.
 */
export function onCloudAiConsentChanged(handler: () => void): Promise<UnlistenFn> {
  return events.cloudAiConsentChanged.listen(() => {
    handler()
  })
}
