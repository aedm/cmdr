/**
 * When a stalled transfer should stop showing a confident ETA, and what it
 * should say instead.
 *
 * The backend does the detecting. `TransferActivity` on every `write-progress`
 * event carries how long the byte counter has been still and what the transfer
 * is waiting on, derived from the live in-flight table in
 * `src-tauri/src/file_system/write_operations/transfer/transfer_probe.rs`.
 * ❌ Don't add a frontend timer over event arrivals: it can only see "nothing
 * lately", so it would call every deliberate pause and foreground yield a
 * stall, and a warning that cries wolf gets ignored.
 *
 * This module owns the presentation decisions on top of that: how long to wait
 * before calling a transfer stalled, and what to say instead while bytes are
 * genuinely on their way (`waitLineFor`). It's pure, so the threshold is
 * trivial to tune and to test.
 */

import type { TransferActivity, TransferWaitReason } from '$lib/tauri-commands'

/**
 * How long the byte counter must be still before the dialog stops showing a
 * time-remaining line and says what's happening instead.
 *
 * Ten seconds, against the log watchdog's 20 s (`STALL_AFTER`), and the two are
 * deliberately different. A log line wants to stay rare across a long-running
 * transfer. A countdown, by contrast, is a lie the moment it stops being true,
 * and ten seconds of a frozen bar is already long enough that a person starts
 * wondering whether the app has died. Both read the same `stillForSeconds`, so
 * the dialog and the log can't contradict each other.
 */
export const STALL_NOTICE_SECONDS = 10

/** What the dialog renders when a transfer has stopped moving. */
export interface StallNotice {
  /** Whole seconds since anything moved. */
  stillForSeconds: number
  /** Which side stopped responding, or `unknown` when nothing explains it. */
  reason: Extract<TransferWaitReason, 'destination' | 'source' | 'unknown'>
  /** Files open right now: written to in part, not yet counted as done. */
  inFlight: number
}

/**
 * Decide whether to show a stall notice, and with what.
 *
 * Returns `null` while a transfer is healthy, while it's doing something
 * deliberate, or when the operation reports no activity at all (local copy,
 * delete, and trash keep no in-flight table — silence beats a guess).
 */
export function stallNoticeFor(activity: TransferActivity | null | undefined): StallNotice | null {
  if (!activity) return null
  if (activity.stillForSeconds < STALL_NOTICE_SECONDS) return null
  // Bytes are arriving that the bar can't count yet: slow, not stopped. The
  // wait line says so instead (`waitLineFor`).
  if (isReceiving(activity)) return null

  // `paused` and `conflict` are the transfer behaving correctly: somebody
  // paused it, or somebody is being asked a question. The dialog already says so
  // in its title, and adding "no progress for 5m" would be true and useless.
  // `moving` can't reach here (stillness would be 0), but it's listed so a new
  // reason has to be classified rather than silently falling through to a
  // stall notice.
  switch (activity.waitingOn) {
    case 'moving':
    case 'paused':
    case 'conflict':
      return null
    case 'destination':
    case 'source':
    case 'unknown':
      return {
        stillForSeconds: activity.stillForSeconds,
        reason: activity.waitingOn,
        inFlight: activity.inFlight,
      }
    default:
      // A reason the backend added and nobody classified here. Stay silent
      // rather than guess: an unexplained warning is worse than none.
      return null
  }
}

/**
 * What the time line says, in place of an ETA, while bytes are genuinely on
 * their way but none has landed: the first file still being opened, or a
 * response still arriving.
 *
 * - `receiving`: the source's connection is receiving at `bytesPerSecond`
 *   while the byte counter stands still. Connection-wide and unverified, so it
 *   never reaches a bar; the backend only sends it while the counter is still.
 * - `opening`: nothing has landed and every file in flight is still being
 *   opened, for `forSeconds`. Shown straight away, because before the first byte
 *   there's no countdown to protect. Past `STALL_NOTICE_SECONDS` with nothing
 *   arriving either, the stall notice takes over and names the source.
 */
export type TransferWaitLine = { kind: 'receiving'; bytesPerSecond: number } | { kind: 'opening'; forSeconds: number }

/** Decide whether the time line has a wait to describe, and which. */
export function waitLineFor(activity: TransferActivity | null | undefined): TransferWaitLine | null {
  if (!activity) return null
  // A pause and a prompt say so in the dialog's title; anything still arriving
  // under them isn't the news.
  if (activity.waitingOn === 'paused' || activity.waitingOn === 'conflict') return null
  if (isReceiving(activity)) {
    return { kind: 'receiving', bytesPerSecond: activity.sourceInboundBytesPerSecond ?? 0 }
  }
  if (activity.openingSource && activity.stillForSeconds < STALL_NOTICE_SECONDS) {
    return { kind: 'opening', forSeconds: activity.stillForSeconds }
  }
  return null
}

function isReceiving(activity: TransferActivity): boolean {
  return (activity.sourceInboundBytesPerSecond ?? 0) > 0
}
