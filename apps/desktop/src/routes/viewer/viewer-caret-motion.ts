/**
 * The pure motion model behind the viewer's keyboard selection: given where the focus is
 * and which motion the user asked for, where does the focus go?
 *
 * Keyboard extension is "keep the anchor, move the focus", so this module answers only
 * the focus half. The five motions are a discriminated union, which makes the whole key
 * map an exhaustive switch the caller can't half-implement.
 *
 * Pure: no DOM, no state, no layout engine. The caller supplies the line cache
 * (`getRowText`) and the file's line count (`getTotalRows`).
 *
 * Three shapes come back:
 * - `focus` set, `targetRow` matching it: the motion landed.
 * - `focus` null with a real `targetRow`: the line the motion wants isn't in the cache.
 *   The caller consumes the key, leaves the selection alone, and **still** scrolls to
 *   `targetRow`, which is what fetches the line so the next press lands. Returning a
 *   bare `null` instead would make the press a permanent no-op, since nothing would ever
 *   fetch the line it wanted. ❌ Never guess an offset on an unfetched line: it crosses
 *   the IPC boundary into `viewer_read_range`.
 * - `focus` equal to `from`: the motion ran into the edge of the file and there was
 *   nowhere to go.
 */

import { EOF_ROW, type RowOffset } from './selection.svelte'
import { findWordEndAfter, findWordStartBefore } from './viewer-word'

/** `-1` is left / up, `+1` is right / down. */
export type MotionDirection = -1 | 1

/**
 * What the pressed chord asked for. `char` steps one grapheme, `word` one word boundary,
 * `line` one LOGICAL line (not one visual row: everything else in the viewer, from
 * `scrollByRows` to the height map to the search jump, counts logical lines, and a
 * second coordinate system here would be the only one), `lineEdge` the start or end of
 * the current line, `docEdge` the start or end of the file.
 */
export type CaretMotion =
  | { kind: 'char'; direction: MotionDirection }
  | { kind: 'word'; direction: MotionDirection }
  | { kind: 'row'; direction: MotionDirection }
  | { kind: 'rowEdge'; direction: MotionDirection }
  | { kind: 'docEdge'; direction: MotionDirection }

export interface MoveFocusArgs {
  /**
   * Where the focus is now.
   *
   * ❌ Never the `EOF_ROW` sentinel: `moveFocus` throws on one. ⌘A in ByteSeek-no-index
   * mode parks the focus there, and the sentinel names no line that can ever be cached,
   * so the keyboard layer resolves it to the last rendered line before calling in. The
   * refusal can't live here: this module knows nothing about what's on screen, so all it
   * could hand back is the sentinel as its own `targetRow`, and the caller would scroll
   * to *that* on every press with no way to shrink the selection again.
   */
  from: RowOffset
  motion: CaretMotion
  /** Reads the cached text of a line, or `undefined` when it hasn't been fetched. */
  getRowText: (line: number) => string | undefined
  /** The file's line count, or `null` in ByteSeek mode before the line index lands. */
  getTotalRows: () => number | null
  /** The column a run of vertical motions is aiming for, or `null` to start a new run. */
  desiredColumn: number | null
}

export interface MoveFocusResult {
  /** Where the focus goes, or `null` when the target line isn't cached. */
  focus: RowOffset | null
  /**
   * The line the caller scrolls to, set on every result including the `null` one.
   *
   * Gotcha: `docEdge` down with no line count yet reports `EOF_ROW`, which names no
   * scrollable row. Treat that value as "scroll to the end of the file"
   * (`scroll.scrollToEnd()`), never as an argument to line arithmetic.
   */
  targetRow: number
  /** The column to carry into the next vertical motion; `null` after a horizontal one. */
  desiredColumn: number | null
}

/** Everything a motion needs to know about the file. */
interface MotionContext {
  getRowText: (line: number) => string | undefined
  getTotalRows: () => number | null
}

/** A motion's answer before the desired column is decided. */
interface Landing {
  focus: RowOffset | null
  targetRow: number
}

const stay = (from: RowOffset): Landing => ({ focus: from, targetRow: from.row })
const at = (focus: RowOffset): Landing => ({ focus, targetRow: focus.row })
const unfetched = (targetRow: number): Landing => ({ focus: null, targetRow })

/** Every motion but `line` ends a run of vertical steps, so it drops the desired column. */
const dropColumn = (landing: Landing): MoveFocusResult => ({ ...landing, desiredColumn: null })

/**
 * Moves the selection's focus by one motion.
 *
 * Vertical motions keep a desired column so that walking down through a short line and
 * back returns to the original column; every other motion clears it. The column is a
 * logical UTF-16 offset, matching the logical-line rule above.
 */
export function moveFocus({ from, motion, getRowText, getTotalRows, desiredColumn }: MoveFocusArgs): MoveFocusResult {
  if (from.row === EOF_ROW) {
    throw new Error(
      'moveFocus was given the end-of-file sentinel line as its origin. Resolve it to a real rendered line first (see MoveFocusArgs.from).',
    )
  }
  const context: MotionContext = { getRowText, getTotalRows }

  switch (motion.kind) {
    case 'char':
      return dropColumn(moveByChar(context, from, motion.direction))
    case 'word':
      return dropColumn(moveByWord(context, from, motion.direction))
    case 'row': {
      const column = desiredColumn ?? from.offset
      return { ...moveByRow(context, from, motion.direction, column), desiredColumn: column }
    }
    case 'rowEdge':
      return dropColumn(moveToRowEdge(context, from, motion.direction))
    case 'docEdge':
      return dropColumn(moveToDocEdge(context, from, motion.direction))
  }
}

/**
 * Whether `line` can be a line of this file. A known count settles it; with no count yet
 * (ByteSeek before the index lands) nothing here can say no, and the line cache decides
 * instead by handing back `undefined`.
 */
function withinFile(context: MotionContext, line: number): boolean {
  if (line < 0) return false
  const total = context.getTotalRows()
  return total === null || line < total
}

/**
 * Moves onto the line one step in `direction` and lands where `landing` says. Bails to
 * `stay` at the edge of the file, and to `unfetched` when the neighbouring line isn't
 * cached, so no offset is ever invented for a line whose text we haven't seen.
 */
function crossRow(
  context: MotionContext,
  from: RowOffset,
  direction: MotionDirection,
  landing: (rowText: string) => number,
): Landing {
  const target = from.row + direction
  if (!withinFile(context, target)) return stay(from)
  const text = context.getRowText(target)
  if (text === undefined) return unfetched(target)
  return at({ row: target, offset: landing(text) })
}

function moveByChar(context: MotionContext, from: RowOffset, direction: MotionDirection): Landing {
  const text = context.getRowText(from.row)
  if (text === undefined) return unfetched(from.row)

  if (direction === 1) {
    if (from.offset < text.length) return at({ row: from.row, offset: nextGraphemeBoundary(text, from.offset) })
    return crossRow(context, from, 1, () => 0)
  }
  if (from.offset > 0) return at({ row: from.row, offset: previousGraphemeBoundary(text, from.offset) })
  return crossRow(context, from, -1, (rowText) => rowText.length)
}

function moveByWord(context: MotionContext, from: RowOffset, direction: MotionDirection): Landing {
  const text = context.getRowText(from.row)
  if (text === undefined) return unfetched(from.row)

  const within = direction === 1 ? wordStopAfter(text, from.offset) : wordStopBefore(text, from.offset)
  if (within !== null) return at({ row: from.row, offset: within })

  // Nothing left to reach on this line. An empty or wordless neighbour is still a stop,
  // so a blank line between paragraphs doesn't get skipped over.
  if (direction === 1) return crossRow(context, from, 1, (rowText) => wordStopAfter(rowText, 0) ?? 0)
  return crossRow(context, from, -1, (rowText) => wordStopBefore(rowText, rowText.length) ?? rowText.length)
}

/**
 * The next stop to the right inside one line: the end of the next word, or the line end
 * when only punctuation and whitespace are left. `null` once the offset is already there.
 */
function wordStopAfter(rowText: string, offset: number): number | null {
  const end = findWordEndAfter(rowText, offset)
  if (end !== null) return end
  return offset < rowText.length ? rowText.length : null
}

/** The mirror of `wordStopAfter`: the start of the previous word, else the line start. */
function wordStopBefore(rowText: string, offset: number): number | null {
  const start = findWordStartBefore(rowText, offset)
  if (start !== null) return start
  return offset > 0 ? 0 : null
}

function moveByRow(context: MotionContext, from: RowOffset, direction: MotionDirection, column: number): Landing {
  const target = from.row + direction
  if (!withinFile(context, target)) return stay(from)
  const text = context.getRowText(target)
  if (text === undefined) return unfetched(target)
  return at({ row: target, offset: Math.min(column, text.length) })
}

function moveToRowEdge(context: MotionContext, from: RowOffset, direction: MotionDirection): Landing {
  if (direction === -1) return at({ row: from.row, offset: 0 })
  const text = context.getRowText(from.row)
  if (text === undefined) return unfetched(from.row)
  return at({ row: from.row, offset: text.length })
}

/**
 * Extends to the start or end of the file. Going down has three answers, and the middle
 * one is why there is no second end-of-file representation:
 * - the last line is cached → its exact end, in one press;
 * - it isn't (the common case on a large file, since the cache only holds fetched
 *   windows) → no offset, but the caller scrolls there, and a second press lands it;
 * - there is no line count at all (ByteSeek before the index) → the `EOF_ROW` sentinel,
 *   which `toRangeEnds` maps to `RangeEnd::Eof` so the backend resolves the true end.
 *   ❌ Don't reach for the sentinel merely because the last line isn't cached: that turns
 *   it into a live, movable focus, which is exactly what `MoveFocusArgs.from` refuses.
 *
 * Consequence worth knowing: ⌘+Shift+Down from mid-file then copy shows the "unknown
 * size" confirm on a large file, because `selectionBytesFromFileSize` answers only for a
 * selection anchored at `(0, 0)` and the per-row estimator hits a row with no known byte
 * length.
 */
function moveToDocEdge(context: MotionContext, from: RowOffset, direction: MotionDirection): Landing {
  if (direction === -1) return at({ row: 0, offset: 0 })

  const total = context.getTotalRows()
  if (total === null) return at({ row: EOF_ROW, offset: 0 })
  if (total <= 0) return stay(from)

  const lastRow = total - 1
  const text = context.getRowText(lastRow)
  if (text === undefined) return unfetched(lastRow)
  return at({ row: lastRow, offset: text.length })
}

/**
 * The offset just past the grapheme cluster starting at `offset`, so an emoji, a ZWJ
 * sequence, or a base letter with a combining mark is one step rather than two to five.
 *
 * Offsets stay UTF-16 code units throughout; grapheme stepping only decides how many of
 * them one press covers. `viewer-pointer.ts` keeps caret geometry on codepoint
 * boundaries, and grapheme boundaries are a strict refinement of those, so the geometry
 * invariant still holds.
 */
function nextGraphemeBoundary(rowText: string, offset: number): number {
  const cluster = graphemesOf(rowText).containing(offset)
  if (cluster === undefined) return Math.min(offset + 1, rowText.length)
  return cluster.index + cluster.segment.length
}

/** The mirror of `nextGraphemeBoundary`: the start of the cluster ending at `offset`. */
function previousGraphemeBoundary(rowText: string, offset: number): number {
  const cluster = graphemesOf(rowText).containing(offset - 1)
  if (cluster === undefined) return Math.max(offset - 1, 0)
  return cluster.index
}

/**
 * Grapheme segments of one line. `containing()` rather than materialising every segment,
 * so a 100k-character log line doesn't allocate an array per arrow press.
 */
function graphemesOf(rowText: string): Intl.Segments {
  return new Intl.Segmenter(undefined, { granularity: 'grapheme' }).segment(rowText)
}
