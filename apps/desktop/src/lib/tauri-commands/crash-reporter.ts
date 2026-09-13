// Crash reporter commands

import { commands } from '$lib/ipc/bindings'
import type { CrashReport } from '$lib/ipc/bindings'
import { throwServerRequestError } from '$lib/error-messages/server-request'

export type { CrashReport }

/** Checks for a pending crash report from a previous session. */
export async function checkPendingCrashReport(): Promise<CrashReport | null> {
  return commands.checkPendingCrashReport()
}

/** Deletes the crash report without sending. */
export async function dismissCrashReport(): Promise<void> {
  await commands.dismissCrashReport()
}

/**
 * Sends the crash report to the server, then deletes the local file. A send that doesn't land
 * throws a `ServerRequestFailure` and keeps the file, so the report comes back next launch.
 */
export async function sendCrashReport(report: CrashReport): Promise<void> {
  const result = await commands.sendCrashReport(report)
  if (result.status === 'error') throwServerRequestError(result.error)
}
