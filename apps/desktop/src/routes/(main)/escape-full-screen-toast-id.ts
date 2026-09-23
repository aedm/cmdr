/**
 * Toast id for the one-time Escape full-screen hint. Its own file so `escape-key.ts`
 * (which shows the toast) and `EscapeFullScreenToastContent.svelte` (which dismisses
 * it) can both reference it without a cycle.
 */
export const ESCAPE_FULL_SCREEN_TOAST_ID = 'escape-full-screen-hint'
