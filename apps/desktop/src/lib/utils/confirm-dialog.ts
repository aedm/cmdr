/**
 * Cross-platform confirmation dialog utility.
 * Uses Tauri's native dialog API which works properly in all contexts.
 */

import { ask } from '@tauri-apps/plugin-dialog'
import { commands } from '$lib/ipc/bindings'
import { tString } from '$lib/intl/messages.svelte'

/**
 * Show a confirmation dialog with OK/Cancel buttons.
 * Uses Tauri's native dialog for reliable behavior.
 *
 * @param message - The message to display
 * @param title - Optional title for the dialog (defaults to 'Confirm')
 * @param confirmLabel - The confirming button's word. Name the act ("Forget"), the
 *   same verb the menu item and the title use; "OK" only where nothing better fits
 * @returns Promise that resolves to true if confirmed, false otherwise
 */
export async function confirmDialog(message: string, title = 'Confirm', confirmLabel = 'OK'): Promise<boolean> {
  // cancelLabel must be 'Cancel' so macOS assigns the ESC key equivalent to it.
  // The default 'No' label doesn't get ESC on NSAlert.
  return ask(message, { title, kind: 'warning', okLabel: confirmLabel, cancelLabel: 'Cancel' })
}

/** What `confirmWithCheckbox` asks. Every string is already translated. */
export interface CheckboxQuestion {
  message: string
  title: string
  /** The confirming button's word: the act, as for `confirmDialog`. */
  confirmLabel: string
  checkboxLabel: string
  /** Whether the checkbox starts checked. */
  checked: boolean
}

/**
 * A confirmation with one checkbox under it (Forget server's "Also forget the saved
 * password"), in the native alert (`commands/confirm_dialog.rs`), so the option is
 * answered in the same breath as the question.
 *
 * ❗ Where there is no native alert (Linux) it asks the plain question and answers
 * the option UNCHECKED: a choice nobody saw is never made for them.
 */
export async function confirmWithCheckbox(
  question: CheckboxQuestion,
): Promise<{ confirmed: boolean; checked: boolean }> {
  const answer = await commands.confirmWithCheckbox({ ...question, cancelLabel: tString('ui.confirmDialog.cancel') })
  switch (answer.kind) {
    case 'confirmed':
      return { confirmed: true, checked: answer.checked }
    case 'cancelled':
      return { confirmed: false, checked: false }
    case 'unsupported':
      return { confirmed: await confirmDialog(question.message, question.title, question.confirmLabel), checked: false }
  }
}
