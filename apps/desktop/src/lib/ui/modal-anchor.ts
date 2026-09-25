/**
 * Where a `growDownward` dialog's top edge goes for its current height: the spot
 * it was centered at (`home`), pulled up only as far as keeps its bottom `margin`
 * inside the window, and never above `margin` from the top.
 *
 * ❗ `margin` is the one the dialog's height cap leaves (`--spacing-xl` on each
 * side). Clamping to the window's own edges put a grown Add sheet flush with the
 * window's bottom while its top kept a margin.
 *
 * ❗ Computed from `home` every time, ❌ never from where it last stood: a dialog
 * that grew past the window, got pulled up, and then shrank back used to stay
 * pinned up there instead of returning to where it opened.
 */
export function anchoredTopFor(home: number, overlayHeight: number, dialogHeight: number, margin: number): number {
  return Math.max(margin, Math.min(home, overlayHeight - dialogHeight - margin))
}
