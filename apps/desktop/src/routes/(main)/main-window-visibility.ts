import { setMainWindowVisible } from '$lib/tauri-commands'

/**
 * Reports the main window's visibility to the backend: once now, then on every `visibilitychange`.
 *
 * WebKit flips `document.visibilityState` for every way the window stops being seen (minimized, app
 * hidden, another Space, fully covered), so this is the backend's one source for "is anyone looking"
 * (`src-tauri/src/main_window_visibility.rs`). Returns the teardown.
 */
export function startMainWindowVisibilityReport(): () => void {
  const report = (): void => {
    // Fire-and-forget: a lost report leaves the backend on its last answer, which the next change fixes.
    setMainWindowVisible(document.visibilityState === 'visible').catch(() => {})
  }
  report()
  document.addEventListener('visibilitychange', report)
  return () => {
    document.removeEventListener('visibilitychange', report)
  }
}
