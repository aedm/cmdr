// The native confirmation with one checkbox (`commands/confirm_dialog.rs`). `$lib/utils/confirm-dialog.ts`'s
// `confirmWithCheckbox` is what callers use; this is the IPC seam under it.

import { commands, type CheckboxConfirm, type CheckboxConfirmRequest } from '$lib/ipc/bindings'
import { getAppLogger } from '$lib/logging/logger'

const log = getAppLogger('ui')

/**
 * Asks `request` in a native alert on this window; `unsupported` where there is none (Linux).
 *
 * ❗ A bridge that breaks down answers `cancelled`: every caller asks before something it can't undo, and a question
 * nobody saw is never a yes.
 */
export async function confirmWithCheckbox(request: CheckboxConfirmRequest): Promise<CheckboxConfirm> {
  try {
    return await commands.confirmWithCheckbox(request)
  } catch (e) {
    log.warn('The confirmation "{title}" broke down: {error}', { title: request.title, error: String(e) })
    return { kind: 'cancelled' }
  }
}
