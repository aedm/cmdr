// macOS custom-updater commands: check / download / install, preserving TCC /
// Full Disk Access by syncing files into the existing `.app` bundle (see
// `$lib/updates/updater.svelte.ts` for the full flow, including the non-macOS
// Tauri-plugin fallback). Plus the cross-platform background-check schedule.

import { commands, type BundleWriteBlocker } from '$lib/ipc/bindings'
import { throwServerRequestError } from '$lib/error-messages/server-request'
import { throwIpcError } from './ipc-types'

export type { BundleWriteBlocker }

/** Metadata for an available update. */
export interface UpdateCheckResult {
  version: string
  url: string
  signature: string
}

/**
 * Whether the running bundle sits somewhere an update can be written into, or `null` when nothing
 * is in the way. Asked once an update is found and before the download starts: an install that
 * can't write its own bundle would otherwise pull ~63 MB and rewrite nothing, every poll interval.
 */
export async function updateWriteBlocker(): Promise<BundleWriteBlocker | null> {
  const res = await commands.updateWriteBlocker()
  if (res.status === 'error') throwIpcError(res.error)
  return res.data
}

/**
 * Fetches `latest.json` and returns update info if a newer version is available, else `null`. A check that doesn't
 * land throws a `ServerRequestFailure`, which the updater words and logs at the level it earns.
 */
export async function checkForUpdate(): Promise<UpdateCheckResult | null> {
  const res = await commands.checkForUpdate()
  if (res.status === 'error') throwServerRequestError(res.error)
  return res.data
}

/** Downloads the update tarball and verifies its minisign signature. */
export async function downloadUpdate(url: string, signature: string): Promise<void> {
  const res = await commands.downloadUpdate(url, signature)
  if (res.status === 'error') throwIpcError(res.error)
}

/** Installs a previously downloaded update by syncing files into the running `.app` bundle. */
export async function installUpdate(): Promise<void> {
  const res = await commands.installUpdate()
  if (res.status === 'error') throwIpcError(res.error)
}

/**
 * Milliseconds until the background update check is due for an interval of `intervalMs` (0 = now), or `null` when
 * the backend couldn't say. The backend remembers the last answered check across relaunches, so a relaunch within the
 * interval doesn't check again. Every platform, unlike the three commands above.
 */
export async function updateCheckDueIn(intervalMs: number): Promise<number | null> {
  try {
    return await commands.updateCheckDueIn(intervalMs)
  } catch {
    return null
  }
}

/**
 * Tells the backend a check finished: `answered` when the update server replied, whatever it said. Best-effort: a
 * failed record only means the next wake asks a stale schedule.
 */
export async function recordUpdateCheck(answered: boolean): Promise<void> {
  try {
    await commands.recordUpdateCheck(answered)
  } catch {
    // Nothing to do: the schedule is a courtesy to the network, never a gate on updating.
  }
}
