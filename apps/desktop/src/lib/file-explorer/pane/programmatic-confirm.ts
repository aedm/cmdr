/**
 * The MCP `dialog confirm`: confirming an open dialog from outside it.
 *
 * Each dialog kind is confirmed the way that dialog confirms itself, which for
 * the transfer dialog means pressing its OWN confirm. The mounted
 * `TransferDialog` registers the function its button runs, and this presses it
 * under the conflict policy the agent named, ❌ never a payload built from the
 * props the dialog opened with. The props hold what the dialog OPENED with; the
 * dialog holds what it will actually send (the edited path, the picked volume,
 * the scan preview). For a compress the two differ from the first frame: the box
 * names `<folder>/<name>.zip` and the props only the folder, which the backend
 * refuses as a read-only destination.
 *
 * The factory owns the registered confirm and nothing else. What's open, and how
 * a delete or a password prompt is answered, stays with the dialog state that
 * builds it (`dialog-state.svelte.ts`), handed in as `deps`.
 */

import { conflictPolicyFromMcpName } from '$lib/file-operations/transfer/conflict-policy'
import { getAppLogger } from '$lib/logging/logger'
import type { DeleteDialogPropsData, TransferConfirmer } from './dialog-props'

const log = getAppLogger('fileExplorer')

export interface ProgrammaticConfirmDeps {
  /** Whether the transfer confirmation is on screen. */
  isTransferDialogOpen: () => boolean
  /** The delete confirmation's props while it's on screen, else `null`. */
  getOpenDeleteDialog: () => DeleteDialogPropsData | null
  confirmDelete: (previewId: string | null, isPermanent: boolean) => void
  /** Whether the archive-password prompt is on screen. */
  isArchivePasswordOpen: () => boolean
  supplyStoredPassword: () => void
}

export function createProgrammaticConfirm(deps: ProgrammaticConfirmDeps) {
  /** The mounted transfer dialog's own confirm, the one its button runs. */
  let pressTransferConfirm: TransferConfirmer | null = null

  return {
    /**
     * Called by the transfer dialog as it mounts, with the function its own
     * confirm button runs. Returns the unregister for its teardown, which leaves
     * a newer dialog's registration alone.
     */
    registerTransferConfirmer(confirm: TransferConfirmer): () => void {
      pressTransferConfirm = confirm
      return () => {
        if (pressTransferConfirm === confirm) pressTransferConfirm = null
      }
    },

    /** Programmatically confirm an open dialog (for MCP confirm action). */
    confirmOpenDialog(dialogType: string, onConflict?: string) {
      const deleteDialog = deps.getOpenDeleteDialog()
      if (dialogType === 'transfer-confirmation' && deps.isTransferDialogOpen()) {
        // A policy the backend accepted but the map doesn't know would quietly
        // become `skip`, so an agent that asked to be asked per file would
        // instead watch every clash get skipped. Say so; the backend validates
        // the name, so this can only fire when the two lists have drifted.
        const mapped = conflictPolicyFromMcpName(onConflict)
        if (onConflict !== undefined && mapped === undefined) {
          log.warn('Unknown conflict policy {onConflict} on a programmatic confirm; falling back to skip', {
            onConflict,
          })
        }
        if (!pressTransferConfirm) {
          log.warn('A programmatic confirm found the transfer dialog open but not mounted yet; nothing confirmed')
          return
        }
        pressTransferConfirm(mapped ?? 'skip')
      } else if (dialogType === 'delete-confirmation' && deleteDialog) {
        // previewId not available when confirming programmatically.
        // For MCP auto-confirm, honor whatever the props initialized with.
        deps.confirmDelete(null, deleteDialog.isPermanent || !deleteDialog.supportsTrash)
      } else if (dialogType === 'archive-password' && deps.isArchivePasswordOpen()) {
        // The `unlock_archive` tool already stored the password on the backend;
        // this is the follow-up. ⚠️ It settles a transfer rather than
        // re-dispatching it — see `supplyStoredPassword`.
        deps.supplyStoredPassword()
      }
    },
  }
}
