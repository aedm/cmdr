/**
 * What an open that didn't succeed shows and logs. The viewer's three open sites (mount,
 * Retry, and the view-as-text / view-as-media reopen) all go through `handleOpenFailure`,
 * so they render one consistent, per-variant message.
 */

import { asViewerError } from '$lib/tauri-commands'
import { tString } from '$lib/intl/messages.svelte'
import type { Logger } from '$lib/logging/logger'

export interface OpenFailure {
  message: string
  canRetry: boolean
}

/**
 * Maps a caught viewer-open failure to the display copy + whether Retry applies. Every
 * branch reads the typed `ViewerError`; anything that never reached the typed path at all
 * reads as the generic copy rather than as the backend's own English.
 */
function openFailureCopy(e: unknown): OpenFailure {
  const ve = asViewerError(e)
  if (ve) {
    if (ve.kind === 'timedOut') return { message: tString('viewer.error.timeout'), canRetry: true }
    if (ve.kind === 'stoppedResponding') {
      return { message: tString('viewer.error.stoppedResponding'), canRetry: true }
    }
    if (ve.kind === 'tooLargeToPreview') return { message: tString('viewer.error.tooLargeToPreview'), canRetry: false }
    if (ve.kind === 'archive') return { message: tString('viewer.error.archiveUnreadable'), canRetry: false }
  }
  return { message: tString('viewer.error.readFailed'), canRetry: false }
}

/**
 * Logs a failed open (`action` names which site) and returns what the window shows for it.
 *
 * A typed `ViewerError` is the backend's answer (a timeout, a file that's gone, a read the
 * OS refused), and the window renders it with its own copy plus Retry where that helps, so
 * it logs at warn. An error log counts toward an auto-sent error report, so it's kept for a
 * failure that never reached the typed path at all: that one is a defect on our side.
 */
export function handleOpenFailure(log: Logger, action: string, e: unknown): OpenFailure {
  if (asViewerError(e)) {
    log.warn('{action} failed: {error}', { action, error: String(e) })
  } else {
    log.error('{action} failed: {error}', { action, error: String(e) })
  }
  return openFailureCopy(e)
}
