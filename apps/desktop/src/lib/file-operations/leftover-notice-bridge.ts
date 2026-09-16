/**
 * Kept-leftovers notice bridge.
 *
 * Turns the backend's `move-leftovers-kept` event into an info toast: a drive
 * came back carrying the working folder of a move that never finished, and Cmdr
 * left every file inside it exactly where it was.
 *
 * **Why this isn't part of a transfer's own toasts.** It belongs to no
 * operation. The move that made the folder ended in an earlier session, or
 * before the drive was unplugged, so there is no `operationId` to hang it off
 * and nothing on screen for it to join. The sweep speaks for itself
 * (`write_operations/in_flight_sweep.rs`).
 *
 * Keyed per folder, so two folders found on one drive each get their own notice
 * and a re-plug can't stack a second copy of the same one. Mounted from
 * `routes/(main)/window-services.ts` beside the other event bridges.
 */

import { type UnlistenFn } from '@tauri-apps/api/event'
import { addToast } from '$lib/ui/toast'
import { getAppLogger } from '$lib/logging/logger'
import { tString } from '$lib/intl/messages.svelte'
import { onMoveLeftoversKept } from '$lib/tauri-commands'
import type { MoveLeftoversKeptEvent } from '$lib/ipc/bindings'

const log = getAppLogger('fileOperations')

/** How long the notice stays up. Longer than a routine toast: it names a folder
 * the person may want to go and look at. */
const NOTICE_TIMEOUT_MS = 12_000

/** Mounts the listener. Returns its unsubscribe. */
export async function startLeftoverNoticeBridge(): Promise<UnlistenFn> {
  const unlisten = await onMoveLeftoversKept(raiseNotice)
  log.debug('Kept-leftovers notice bridge mounted')
  return unlisten
}

function raiseNotice(payload: MoveLeftoversKeptEvent): void {
  log.info('An unfinished move left files on {volumeName}, in {folderName}', {
    volumeName: payload.volumeName,
    folderName: payload.folderName,
  })
  addToast(
    tString('fileOperations.leftovers.stagingFolderKept', {
      volumeName: payload.volumeName,
      folderName: payload.folderName,
    }),
    {
      level: 'info',
      timeoutMs: NOTICE_TIMEOUT_MS,
      id: `move-leftovers-kept-${payload.folderName}`,
    },
  )
}
