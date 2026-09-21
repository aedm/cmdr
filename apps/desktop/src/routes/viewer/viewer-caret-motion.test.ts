/**
 * The pure motion model: where each keyboard motion puts the selection's focus.
 *
 * No DOM and no layout engine here, so the fixtures are plain arrays of line text and a
 * `getRowText` that returns `undefined` for anything the line cache hasn't fetched.
 */
import { describe, it, expect } from 'vitest'

import { moveFocus, type CaretMotion } from './viewer-caret-motion'
import { EOF_ROW } from './selection.svelte'

/** `alpha beta` is 10 units, `x` is 1, the empty line 0, `gamma delta` 11. */
const LINES = ['alpha beta', 'x', '', 'gamma delta']

interface DepsOverrides {
  lines?: (string | undefined)[]
  totalRows?: number | null
}

function deps({ lines = LINES, totalRows }: DepsOverrides = {}) {
  return {
    getRowText: (line: number): string | undefined => lines[line],
    getTotalRows: (): number | null => (totalRows === undefined ? lines.length : totalRows),
  }
}

const char = (direction: -1 | 1): CaretMotion => ({ kind: 'char', direction })
const word = (direction: -1 | 1): CaretMotion => ({ kind: 'word', direction })
const row = (direction: -1 | 1): CaretMotion => ({ kind: 'row', direction })
const rowEdge = (direction: -1 | 1): CaretMotion => ({ kind: 'rowEdge', direction })
const docEdge = (direction: -1 | 1): CaretMotion => ({ kind: 'docEdge', direction })

describe('moveFocus: character motion', () => {
  it('steps one unit right inside a line', () => {
    expect(moveFocus({ from: { row: 0, offset: 0 }, motion: char(1), desiredColumn: null, ...deps() })).toEqual({
      focus: { row: 0, offset: 1 },
      targetRow: 0,
      desiredColumn: null,
    })
  })

  it('steps one unit left inside a line', () => {
    expect(moveFocus({ from: { row: 0, offset: 5 }, motion: char(-1), desiredColumn: null, ...deps() })).toEqual({
      focus: { row: 0, offset: 4 },
      targetRow: 0,
      desiredColumn: null,
    })
  })

  it('crosses to the start of the next line at the line end', () => {
    expect(moveFocus({ from: { row: 0, offset: 10 }, motion: char(1), desiredColumn: null, ...deps() })).toEqual({
      focus: { row: 1, offset: 0 },
      targetRow: 1,
      desiredColumn: null,
    })
  })

  it('crosses to the end of the previous line at offset 0', () => {
    expect(moveFocus({ from: { row: 1, offset: 0 }, motion: char(-1), desiredColumn: null, ...deps() })).toEqual({
      focus: { row: 0, offset: 10 },
      targetRow: 0,
      desiredColumn: null,
    })
  })

  it('lands on an empty line as its own stop', () => {
    expect(moveFocus({ from: { row: 3, offset: 0 }, motion: char(-1), desiredColumn: null, ...deps() })).toEqual({
      focus: { row: 2, offset: 0 },
      targetRow: 2,
      desiredColumn: null,
    })
    expect(moveFocus({ from: { row: 2, offset: 0 }, motion: char(1), desiredColumn: null, ...deps() })).toEqual({
      focus: { row: 3, offset: 0 },
      targetRow: 3,
      desiredColumn: null,
    })
  })

  it('stays put at the first character of the file', () => {
    expect(moveFocus({ from: { row: 0, offset: 0 }, motion: char(-1), desiredColumn: null, ...deps() })).toEqual({
      focus: { row: 0, offset: 0 },
      targetRow: 0,
      desiredColumn: null,
    })
  })

  it('stays put at the last character of the file', () => {
    expect(moveFocus({ from: { row: 3, offset: 11 }, motion: char(1), desiredColumn: null, ...deps() })).toEqual({
      focus: { row: 3, offset: 11 },
      targetRow: 3,
      desiredColumn: null,
    })
  })

  it('forgets the desired column', () => {
    const result = moveFocus({ from: { row: 0, offset: 2 }, motion: char(1), desiredColumn: 7, ...deps() })
    expect(result.desiredColumn).toBeNull()
  })

  it('yields no offset when the line it needs has not been fetched', () => {
    const sparse = deps({ lines: ['alpha beta', undefined, 'gamma'] })
    expect(moveFocus({ from: { row: 2, offset: 0 }, motion: char(-1), desiredColumn: null, ...sparse })).toEqual({
      focus: null,
      targetRow: 1,
      desiredColumn: null,
    })
  })
})

describe('moveFocus: character motion crosses a grapheme cluster in one step', () => {
  it('treats an emoji as one step', () => {
    // `👍` is two UTF-16 units.
    const emoji = deps({ lines: ['👍x'] })
    expect(moveFocus({ from: { row: 0, offset: 0 }, motion: char(1), desiredColumn: null, ...emoji }).focus).toEqual({
      row: 0,
      offset: 2,
    })
    expect(moveFocus({ from: { row: 0, offset: 2 }, motion: char(-1), desiredColumn: null, ...emoji }).focus).toEqual({
      row: 0,
      offset: 0,
    })
  })

  it('treats a ZWJ family sequence as one step', () => {
    // 👨‍👩‍👧 is three emoji joined by two ZWJs: eight UTF-16 units, one grapheme.
    const family = deps({ lines: ['👨‍👩‍👧!'] })
    expect(moveFocus({ from: { row: 0, offset: 0 }, motion: char(1), desiredColumn: null, ...family }).focus).toEqual({
      row: 0,
      offset: 8,
    })
    expect(moveFocus({ from: { row: 0, offset: 8 }, motion: char(-1), desiredColumn: null, ...family }).focus).toEqual({
      row: 0,
      offset: 0,
    })
  })

  it('treats a base letter plus a combining mark as one step', () => {
    // `e` + U+0301 combining acute: two UTF-16 units, one grapheme.
    const combining = deps({ lines: ['éx'] })
    expect(
      moveFocus({ from: { row: 0, offset: 0 }, motion: char(1), desiredColumn: null, ...combining }).focus,
    ).toEqual({ row: 0, offset: 2 })
    expect(
      moveFocus({ from: { row: 0, offset: 2 }, motion: char(-1), desiredColumn: null, ...combining }).focus,
    ).toEqual({ row: 0, offset: 0 })
  })
})

describe('moveFocus: word motion', () => {
  it('lands on the end of the word to the right (macOS semantics)', () => {
    expect(moveFocus({ from: { row: 0, offset: 0 }, motion: word(1), desiredColumn: null, ...deps() }).focus).toEqual({
      row: 0,
      offset: 5,
    })
    expect(moveFocus({ from: { row: 0, offset: 5 }, motion: word(1), desiredColumn: null, ...deps() }).focus).toEqual({
      row: 0,
      offset: 10,
    })
  })

  it('lands on the start of the word to the left (macOS semantics)', () => {
    expect(moveFocus({ from: { row: 0, offset: 10 }, motion: word(-1), desiredColumn: null, ...deps() }).focus).toEqual(
      { row: 0, offset: 6 },
    )
    expect(moveFocus({ from: { row: 0, offset: 6 }, motion: word(-1), desiredColumn: null, ...deps() }).focus).toEqual({
      row: 0,
      offset: 0,
    })
  })

  it('stops at the line end when only non-word characters are left', () => {
    const tail = deps({ lines: ['foo ...'] })
    expect(moveFocus({ from: { row: 0, offset: 3 }, motion: word(1), desiredColumn: null, ...tail }).focus).toEqual({
      row: 0,
      offset: 7,
    })
  })

  it('stops at the line start when only non-word characters are left', () => {
    const head = deps({ lines: ['... foo'] })
    expect(moveFocus({ from: { row: 0, offset: 4 }, motion: word(-1), desiredColumn: null, ...head }).focus).toEqual({
      row: 0,
      offset: 0,
    })
  })

  it('crosses to the next line’s first word when the line is exhausted', () => {
    expect(moveFocus({ from: { row: 0, offset: 10 }, motion: word(1), desiredColumn: null, ...deps() })).toEqual({
      focus: { row: 1, offset: 1 },
      targetRow: 1,
      desiredColumn: null,
    })
  })

  it('crosses to the previous line’s last word when the line is exhausted', () => {
    expect(moveFocus({ from: { row: 1, offset: 0 }, motion: word(-1), desiredColumn: null, ...deps() })).toEqual({
      focus: { row: 0, offset: 6 },
      targetRow: 0,
      desiredColumn: null,
    })
  })

  it('stops on an empty line rather than skipping over it', () => {
    expect(moveFocus({ from: { row: 1, offset: 1 }, motion: word(1), desiredColumn: null, ...deps() }).focus).toEqual({
      row: 2,
      offset: 0,
    })
    expect(moveFocus({ from: { row: 3, offset: 0 }, motion: word(-1), desiredColumn: null, ...deps() }).focus).toEqual({
      row: 2,
      offset: 0,
    })
  })

  it('stays put at either end of the file', () => {
    expect(moveFocus({ from: { row: 3, offset: 11 }, motion: word(1), desiredColumn: null, ...deps() }).focus).toEqual({
      row: 3,
      offset: 11,
    })
    expect(moveFocus({ from: { row: 0, offset: 0 }, motion: word(-1), desiredColumn: null, ...deps() }).focus).toEqual({
      row: 0,
      offset: 0,
    })
  })

  it('yields no offset when the line it crosses onto has not been fetched', () => {
    const sparse = deps({ lines: ['alpha beta', undefined] })
    expect(moveFocus({ from: { row: 0, offset: 10 }, motion: word(1), desiredColumn: null, ...sparse })).toEqual({
      focus: null,
      targetRow: 1,
      desiredColumn: null,
    })
  })

  it('forgets the desired column', () => {
    const result = moveFocus({ from: { row: 0, offset: 0 }, motion: word(1), desiredColumn: 7, ...deps() })
    expect(result.desiredColumn).toBeNull()
  })
})

describe('moveFocus: line motion', () => {
  it('takes the desired column from the current offset on the first step', () => {
    expect(moveFocus({ from: { row: 0, offset: 8 }, motion: row(1), desiredColumn: null, ...deps() })).toEqual({
      focus: { row: 1, offset: 1 },
      targetRow: 1,
      desiredColumn: 8,
    })
  })

  it('keeps the desired column across shorter lines and restores it on a long one', () => {
    expect(moveFocus({ from: { row: 1, offset: 1 }, motion: row(1), desiredColumn: 8, ...deps() })).toEqual({
      focus: { row: 2, offset: 0 },
      targetRow: 2,
      desiredColumn: 8,
    })
    expect(moveFocus({ from: { row: 2, offset: 0 }, motion: row(1), desiredColumn: 8, ...deps() })).toEqual({
      focus: { row: 3, offset: 8 },
      targetRow: 3,
      desiredColumn: 8,
    })
  })

  it('walks back up with the same column', () => {
    expect(moveFocus({ from: { row: 3, offset: 8 }, motion: row(-1), desiredColumn: 8, ...deps() })).toEqual({
      focus: { row: 2, offset: 0 },
      targetRow: 2,
      desiredColumn: 8,
    })
  })

  it('stays put on the first line', () => {
    expect(moveFocus({ from: { row: 0, offset: 3 }, motion: row(-1), desiredColumn: null, ...deps() })).toEqual({
      focus: { row: 0, offset: 3 },
      targetRow: 0,
      desiredColumn: 3,
    })
  })

  it('stays put on the last line', () => {
    expect(moveFocus({ from: { row: 3, offset: 3 }, motion: row(1), desiredColumn: null, ...deps() })).toEqual({
      focus: { row: 3, offset: 3 },
      targetRow: 3,
      desiredColumn: 3,
    })
  })

  it('yields no offset but still names the target line when it has not been fetched', () => {
    // Decision 5: the caller consumes the key, leaves the selection alone, and scrolls to
    // `targetRow` anyway, which is what fetches the line so the next press can land.
    const sparse = deps({ lines: ['alpha beta', undefined, 'gamma'] })
    expect(moveFocus({ from: { row: 0, offset: 2 }, motion: row(1), desiredColumn: null, ...sparse })).toEqual({
      focus: null,
      targetRow: 1,
      desiredColumn: 2,
    })
  })

  it('steps into unknown territory when the line count is unknown', () => {
    const unindexed = deps({ lines: ['alpha beta'], totalRows: null })
    expect(moveFocus({ from: { row: 0, offset: 2 }, motion: row(1), desiredColumn: null, ...unindexed })).toEqual({
      focus: null,
      targetRow: 1,
      desiredColumn: 2,
    })
  })
})

describe('moveFocus: line-edge motion', () => {
  it('extends to the end of the line', () => {
    expect(moveFocus({ from: { row: 0, offset: 3 }, motion: rowEdge(1), desiredColumn: 9, ...deps() })).toEqual({
      focus: { row: 0, offset: 10 },
      targetRow: 0,
      desiredColumn: null,
    })
  })

  it('extends to the start of the line', () => {
    expect(moveFocus({ from: { row: 0, offset: 3 }, motion: rowEdge(-1), desiredColumn: 9, ...deps() })).toEqual({
      focus: { row: 0, offset: 0 },
      targetRow: 0,
      desiredColumn: null,
    })
  })

  it('yields no offset for the line end when the line has not been fetched', () => {
    const sparse = deps({ lines: ['alpha beta', undefined, 'gamma'] })
    expect(moveFocus({ from: { row: 1, offset: 0 }, motion: rowEdge(1), desiredColumn: null, ...sparse })).toEqual({
      focus: null,
      targetRow: 1,
      desiredColumn: null,
    })
  })
})

describe('moveFocus: document-edge motion', () => {
  it('extends to the first character of the file', () => {
    expect(moveFocus({ from: { row: 3, offset: 5 }, motion: docEdge(-1), desiredColumn: 5, ...deps() })).toEqual({
      focus: { row: 0, offset: 0 },
      targetRow: 0,
      desiredColumn: null,
    })
  })

  it('extends to the last character of the file in one press when the last line is cached', () => {
    expect(moveFocus({ from: { row: 0, offset: 2 }, motion: docEdge(1), desiredColumn: null, ...deps() })).toEqual({
      focus: { row: 3, offset: 11 },
      targetRow: 3,
      desiredColumn: null,
    })
  })

  it('names the last line without an offset when it is not cached, so a second press lands it', () => {
    // Decision 6: the line cache only holds fetched windows, so the last line of a large
    // file essentially never is. The scroll the caller does next fetches it.
    const sparse = deps({ lines: ['alpha beta', undefined, undefined] })
    expect(moveFocus({ from: { row: 0, offset: 2 }, motion: docEdge(1), desiredColumn: null, ...sparse })).toEqual({
      focus: null,
      targetRow: 2,
      desiredColumn: null,
    })
  })

  it('mints the end-of-file sentinel when the file has no line count yet', () => {
    // ByteSeek before the line index lands: no last line to name, so the focus goes to
    // `EOF_ROW` and the backend resolves the real end at the IPC boundary.
    const unindexed = deps({ lines: ['alpha beta'], totalRows: null })
    expect(moveFocus({ from: { row: 0, offset: 2 }, motion: docEdge(1), desiredColumn: null, ...unindexed })).toEqual({
      focus: { row: EOF_ROW, offset: 0 },
      targetRow: EOF_ROW,
      desiredColumn: null,
    })
  })

  it('stays put in an empty file', () => {
    const empty = deps({ lines: [] })
    expect(moveFocus({ from: { row: 0, offset: 0 }, motion: docEdge(1), desiredColumn: null, ...empty })).toEqual({
      focus: { row: 0, offset: 0 },
      targetRow: 0,
      desiredColumn: null,
    })
  })
})

describe('moveFocus: the no-sentinel precondition', () => {
  // Decision 7: the keyboard layer resolves a sentinel focus to a real rendered line
  // before calling in. If it ever didn't, `targetRow` could only be the sentinel too,
  // and the caller would scroll to it on every press with no way back.
  it.each<[string, CaretMotion]>([
    ['char', char(-1)],
    ['word', word(-1)],
    ['row', row(-1)],
    ['rowEdge', rowEdge(1)],
    ['docEdge', docEdge(-1)],
  ])('refuses an end-of-file sentinel as the origin of %s motion', (_name, motion) => {
    expect(() => moveFocus({ from: { row: EOF_ROW, offset: 0 }, motion, desiredColumn: null, ...deps() })).toThrow(
      /sentinel/i,
    )
  })
})
