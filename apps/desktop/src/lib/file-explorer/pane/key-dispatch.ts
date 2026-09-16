/**
 * Top-level keyboard + focus event routing for the dual-pane explorer, lifted out
 * of `DualPaneExplorer`. These are the handlers bound on the explorer container
 * (`onkeydown` / `onkeyup` / `onfocusin`); they own no state, they route events
 * to the focused pane and enforce the focus guard.
 *
 * `handleKeyDown` dispatch order is load-bearing and unchanged:
 *   1. Escape while loading → cancel the load (and swallow the key).
 *   2. A header menu (volume switcher or favorites) open on either pane → swallow
 *      (the panes behind it stay inert, Fix E). The menu itself already caught what
 *      it wanted.
 *   3. Type-to-jump intercept → route printable keys into the active pane's buffer
 *      BEFORE any shortcut sees them. Once a jump is active the captured set
 *      widens to any printable key (L9 — mirror of `pane-commands.routePanelKey`).
 *      Reset keys clear the buffer and fall through.
 *   4. Otherwise forward to the focused pane's `handleKeyDown`.
 */

import { isTextInputTarget } from '$lib/utils/text-input-focus'
import { isPrintableJumpContinuation, isTypeToJumpChar, isTypeToJumpResetKey } from './type-to-jump-keys'
import type { FilePaneAPI } from './types'

export interface KeyDispatchDeps {
  getPaneRef: (pane: 'left' | 'right') => FilePaneAPI | undefined
  getFocusedPane: () => 'left' | 'right'
  getContainerElement: () => HTMLElement | undefined
}

export interface KeyDispatch {
  handleKeyDown: (e: KeyboardEvent) => void
  handleKeyUp: (e: KeyboardEvent) => void
  handleFocusGuard: (e: FocusEvent) => void
}

/** True if the key went to a text-entry control (rename, search dialog, login form, etc.). */
export function isTypingInInput(e: KeyboardEvent): boolean {
  return isTextInputTarget(e.target)
}

export function createKeyDispatch(deps: KeyDispatchDeps): KeyDispatch {
  /**
   * SWALLOWS every key from the panes while a header menu — the volume switcher or the
   * favorites menu — is open on either one (⌥F1/⌥F2 can open one on the non-focused pane,
   * so we scan both).
   *
   * ❗ No routing: both are a house `Menu`, which catches keys on its own document-level
   * CAPTURE listener before this handler runs, and stops the ones it uses. What reaches
   * here is what the menu deliberately let through — the inline favorite-rename
   * `<input>`'s keystrokes — and those must still never move the pane cursor behind it.
   */
  function swallowedByHeaderMenu(): boolean {
    return (
      (deps.getPaneRef('left')?.isHeaderMenuOpen() ?? false) || (deps.getPaneRef('right')?.isHeaderMenuOpen() ?? false)
    )
  }

  function handleEscapeDuringLoading(): boolean {
    const paneRef = deps.getPaneRef(deps.getFocusedPane())
    if (paneRef?.isLoading()) {
      paneRef.handleCancelLoading()
      return true
    }
    return false
  }

  /**
   * Prevents focus from escaping to buttons/links inside the explorer. Inputs (rename,
   * network login) and dialog content are exempt. The dialog exemption is load-bearing:
   * the rename dialogs mount INSIDE FilePane, and without it this guard yanks focus off
   * the dialog overlay while `use:trapFocus` pulls it back — an endless focus ping-pong
   * of microtasks that starves the event loop and freezes the whole webview (pinned by
   * the "rename to existing name is rejected on MTP" E2E). Focus containment inside a
   * dialog is the trap's job, not this guard's; the exemption also makes the dialogs'
   * buttons keyboard-reachable.
   */
  function handleFocusGuard(e: FocusEvent): void {
    const containerElement = deps.getContainerElement()
    const target = e.target as HTMLElement
    if (
      target === containerElement ||
      target instanceof HTMLInputElement ||
      target instanceof HTMLTextAreaElement ||
      target instanceof HTMLSelectElement ||
      target.isContentEditable ||
      target.closest('[role="dialog"], [role="alertdialog"]') !== null
    )
      return
    containerElement?.focus()
  }

  function handleKeyDown(e: KeyboardEvent): void {
    // ESC during loading = cancel and go back
    if (e.key === 'Escape' && handleEscapeDuringLoading()) {
      e.preventDefault()
      return
    }

    // A header menu owns the keyboard while it's open
    if (swallowedByHeaderMenu()) {
      return
    }

    // Type-to-jump intercept: route printable letters/digits into the
    // active pane's buffer before any other shortcut sees them. Reset keys
    // (arrows, page nav, enter, tab, backspace, esc) clear an active buffer
    // and then fall through to their existing handlers.
    //
    // Once a jump is ACTIVE (buffer non-empty), the captured set widens to
    // every printable key (`isPrintableJumpContinuation`): while you're typing
    // a name, `-`, Space, etc. extend the buffer instead of firing their own
    // single-char command. The widening ends when the reset timeout empties
    // the buffer, so a lone `-` deselects again. (Mirror this in
    // `pane-commands.ts` `routePanelKey` — landmine L9.)
    const activePaneRef = deps.getPaneRef(deps.getFocusedPane())
    if (activePaneRef && !isTypingInInput(e) && !activePaneRef.isRenaming()) {
      if (isTypeToJumpChar(e) || (activePaneRef.isJumpActive() && isPrintableJumpContinuation(e))) {
        activePaneRef.handleJumpKeystroke(e.key)
        e.preventDefault()
        e.stopPropagation()
        return
      }
      if (isTypeToJumpResetKey(e)) {
        activePaneRef.clearJumpState()
        // Fall through; Enter/arrows/Backspace/ESC keep their existing meaning.
      }
    }

    // Forward arrow keys and Enter to the focused pane
    activePaneRef?.handleKeyDown(e)
  }

  function handleKeyUp(e: KeyboardEvent): void {
    // Forward to the focused pane for range selection finalization
    const activePaneRef = deps.getPaneRef(deps.getFocusedPane())
    activePaneRef?.handleKeyUp(e)
  }

  return { handleKeyDown, handleKeyUp, handleFocusGuard }
}
