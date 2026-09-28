/**
 * The two ADB settings and the one push that applies them.
 *
 * `fileOperations.adbEnabled` and `fileOperations.adbBinaryPath` travel
 * TOGETHER: the backend restarts the device tracker under whichever binary the
 * path names, so pushing one without the other would restart it under a stale
 * one. Either change re-pushes both, the shape the low-disk-space pair takes.
 */

import { getSetting } from '$lib/settings'
import { setAdbSettings } from '$lib/tauri-commands'
import { getAppLogger } from '$lib/logging/logger'

const log = getAppLogger('adb')

/** Setting keys. Mirror the registry entries. */
export const ADB_ENABLED_SETTING_KEY = 'fileOperations.adbEnabled' as const
export const ADB_BINARY_PATH_SETTING_KEY = 'fileOperations.adbBinaryPath' as const

/** A path is typed character by character, while applying it restarts the ADB tracker. */
const ADB_CONFIG_PUSH_DEBOUNCE_MS = 500
let scheduledPush: ReturnType<typeof setTimeout> | undefined

export function cancelScheduledAdbConfigPush(): void {
  if (scheduledPush === undefined) return
  clearTimeout(scheduledPush)
  scheduledPush = undefined
}

export function scheduleAdbConfigPush(): void {
  cancelScheduledAdbConfigPush()
  scheduledPush = setTimeout(() => {
    scheduledPush = undefined
    void pushAdbConfigToBackend()
  }, ADB_CONFIG_PUSH_DEBOUNCE_MS)
}

/**
 * Pushes both ADB settings, read fresh. Call it after either one changes.
 *
 * A blank path means "look for `adb` the usual way", so it crosses as `null`
 * rather than an empty string.
 */
export async function pushAdbConfigToBackend(): Promise<void> {
  // Browse applies immediately so it can re-check the selected binary. Cancel
  // the store listener's pending debounce rather than restarting the tracker twice.
  cancelScheduledAdbConfigPush()
  const enabled = getSetting(ADB_ENABLED_SETTING_KEY)
  const binaryPath = getSetting(ADB_BINARY_PATH_SETTING_KEY).trim()
  try {
    await setAdbSettings(enabled, binaryPath === '' ? null : binaryPath)
  } catch (error) {
    log.warn('Could not apply the ADB settings: {error}', { error: String(error) })
  }
}
