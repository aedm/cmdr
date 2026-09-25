/**
 * Where a `growDownward` dialog's top edge goes for its current height: the spot
 * it was centered at (`home`), pulled up only as far as keeps its bottom on
 * screen, and never above the top.
 *
 * ❗ Computed from `home` every time, ❌ never from where it last stood: a dialog
 * that grew past the window, got pulled up, and then shrank back used to stay
 * pinned up there instead of returning to where it opened.
 */
export function anchoredTopFor(home: number, overlayHeight: number, dialogHeight: number): number {
  return Math.max(0, Math.min(home, overlayHeight - dialogHeight))
}
