/**
 * Selection granularity for the viewer: how far a gesture snaps its endpoints out.
 *
 * A double-press selects a word and a triple-press a line, and dragging or shift-clicking
 * afterwards keeps working in those units, the way native text views do. Both endpoints
 * of the resulting `Selection` are edges of granularity ranges, so the drag re-derives
 * the whole selection from the pressed range on every move instead of nudging one end.
 *
 * That re-derivation is what makes a hand twitch harmless: a pointer that hasn't left the
 * pressed word yields the same union the press did, so there is nothing to collapse.
 *
 * Pure: no DOM, no state, no layout engine. The caller resolves the pointer to a
 * `RowOffset` (`viewer-pointer.ts`) and hands the line text in.
 */

import { findWordBoundsAt } from './viewer-word'
import type { RowOffset, Selection } from './selection.svelte'

/** How far a gesture snaps its endpoints out: to the caret, the word, or the whole line. */
export type SelectionGranularity = 'character' | 'word' | 'row'

/** A half-open `[start, end)` UTF-16 span inside one logical line. */
export interface RowRange {
  /** Zero-based line the span sits on. */
  row: number
  /** UTF-16 start offset, included. */
  start: number
  /** UTF-16 end offset, excluded. */
  end: number
}

interface RangeAtCaretArgs {
  /** The resolved pointer position. */
  caret: RowOffset
  granularity: SelectionGranularity
  /** Reads the cached text of a line, or `undefined` when it hasn't been fetched. */
  getRowText: (line: number) => string | undefined
}

interface ExtendArgs extends Omit<RangeAtCaretArgs, 'caret'> {
  /** The range the gesture started from, snapped at the same granularity. */
  anchorRange: RowRange
  /** Where the pointer is now. */
  focus: RowOffset
}

/**
 * Returns the range `caret` belongs to at `granularity`: itself for `character`, the
 * surrounding word for `word`, the whole logical line for `line` (offset 0 to its UTF-16
 * length, so a word-wrapped line still covers in full).
 *
 * A line the cache hasn't fetched yet reads as empty and collapses to `[0, 0)`. That
 * happens during a fast autoscroll into unfetched rows, where the row renders empty
 * anyway, so the selection matches what the user sees.
 */
export function rangeAtCaret({ caret, granularity, getRowText }: RangeAtCaretArgs): RowRange {
  if (granularity === 'character') return { row: caret.row, start: caret.offset, end: caret.offset }

  const rowText = getRowText(caret.row) ?? ''
  if (granularity === 'row') return { row: caret.row, start: 0, end: rowText.length }

  const { start, end } = findWordBoundsAt(rowText, caret.offset)
  return { row: caret.row, start, end }
}

/**
 * Returns the selection spanning `anchorRange` and the range `focus` falls in, with the
 * gesture's direction preserved: dragging forward anchors at the start of the pressed
 * range, dragging back past it anchors at the end. `normaliseSelection` orders either one
 * for rendering and for the IPC boundary, so the direction only has to stay honest for
 * the next move to read it.
 */
export function extendRangeToGranularity({ anchorRange, focus, granularity, getRowText }: ExtendArgs): Selection {
  const focusRange = rangeAtCaret({ caret: focus, granularity, getRowText })
  const backwards =
    focusRange.row < anchorRange.row || (focusRange.row === anchorRange.row && focusRange.start < anchorRange.start)

  if (backwards) {
    return {
      anchor: { row: anchorRange.row, offset: anchorRange.end },
      focus: { row: focusRange.row, offset: focusRange.start },
    }
  }
  return {
    anchor: { row: anchorRange.row, offset: anchorRange.start },
    focus: { row: focusRange.row, offset: focusRange.end },
  }
}
