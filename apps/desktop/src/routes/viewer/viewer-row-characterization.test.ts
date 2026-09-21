/**
 * Characterization tests: what select-all and the copy-size arithmetic do TODAY.
 *
 * The viewer is about to stop counting physical lines and start counting bounded rows
 * (`docs/specs/viewer-row-wrap.md`). Invariant I6 says a file whose every line is
 * shorter than the segment size must behave byte-identically afterwards, and these
 * tests are the register that claim is checked against.
 *
 * ❗ These pin REALITY, including behaviour that is wrong. Where today's answer is a
 * bug, the test says so in its name and its comment and asserts the buggy value anyway.
 * A rewrite that changes one of these has to change it deliberately, which is the whole
 * point. ❌ Don't "fix" a test here by deleting it.
 */

import { describe, expect, it, vi } from 'vitest'

import {
  estimateSelectionBytes,
  isWholeFileSelection,
  makeSelectAll,
  makeSelectToEof,
  toRangeEnds,
  EOF_LINE,
  type Selection,
} from './selection.svelte'
import { createViewerKeyboard } from './viewer-keyboard'

type KeyboardDeps = Parameters<typeof createViewerKeyboard>[0]

/**
 * A keyboard dep set wired for ⌘A only: the line count, the per-line text, and a
 * `selectAll` / `selectToEof` pair of spies. Everything else is a no-op, so a test
 * reads as the one question it asks.
 */
function selectAllDeps(overrides: Partial<KeyboardDeps> = {}) {
  const noop = vi.fn()
  const selectAll = vi.fn()
  const selectToEof = vi.fn()
  const deps = {
    getTotalLines: () => 3,
    getTotalBytes: () => 16,
    getLineText: () => 'line',
    getLastRenderedLine: () => 2,
    selection: { selection: null, selectAll, selectToEof, setFocus: noop },
    scroll: {
      scrollByLines: noop,
      scrollByPages: noop,
      scrollToStart: noop,
      scrollToEnd: noop,
      scrollByColumns: noop,
      ensureLineVisible: noop,
      ensureColumnVisible: noop,
    },
    search: {
      searchVisible: false,
      searchStatus: 'idle' as const,
      searchInputRef: null,
      openSearch: noop,
      closeSearch: noop,
      stopSearch: noop,
      findNext: noop,
      findPrev: noop,
      toggleUseRegex: noop,
      toggleCaseSensitive: noop,
    },
    copy: { busy: false, cancelInFlight: () => Promise.resolve() },
    ...overrides,
  } as unknown as KeyboardDeps
  return { keyboard: createViewerKeyboard(deps), selectAll, selectToEof }
}

/**
 * The two per-line lookups `+page.svelte` hands `estimateSelectionBytes`: a UTF-8 byte
 * length with one byte added for the line's newline delimiter, and a UTF-16 length.
 * Reproduced here rather than imported because they live inline in the page component.
 */
function lineLookups(lines: string[]) {
  const cached = (n: number) => n >= 0 && n < lines.length
  const bytes = (n: number) => (cached(n) ? new TextEncoder().encode(lines[n]).length + 1 : null)
  const utf16 = (n: number) => (cached(n) ? lines[n].length : null)
  return { bytes, utf16 }
}

describe('select-all, as it behaves today', () => {
  it('ends the selection at the last line, at that line length', () => {
    // "alpha\nbeta\ngamma" with no trailing newline: three lines, the last one 5 long.
    expect(makeSelectAll(3, 5)).toEqual({ anchor: { line: 0, offset: 0 }, focus: { line: 2, offset: 5 } })
  })

  it('ends at offset 0 of a trailing empty line when the file DOES end in a newline', () => {
    // Every backend reports a trailing empty line for a file ending in `\n`, so ⌘A on
    // "alpha\nbeta\ngamma\n" is 4 lines with a 0-length last one. The range that reaches
    // the backend therefore stops at the newline, not past it.
    expect(makeSelectAll(4, 0)).toEqual({ anchor: { line: 0, offset: 0 }, focus: { line: 3, offset: 0 } })
  })

  it('is a no-op on an empty file', () => {
    expect(makeSelectAll(0, 0)).toBeNull()
  })

  it('reaches the backend as two line endpoints, or as `eof` when the count is unknown', () => {
    expect(toRangeEnds(makeSelectAll(3, 5))).toEqual({
      anchor: { kind: 'line', line: 0, offset: 0 },
      focus: { kind: 'line', line: 2, offset: 5 },
    })
    expect(toRangeEnds(makeSelectToEof())).toEqual({
      anchor: { kind: 'line', line: 0, offset: 0 },
      focus: { kind: 'eof' },
    })
  })

  it('takes the eof path when the line count is not known yet', () => {
    const { keyboard, selectAll, selectToEof } = selectAllDeps({ getTotalLines: () => null })
    keyboard.handleSelectAllShortcut()
    expect(selectAll).not.toHaveBeenCalled()
    expect(selectToEof).toHaveBeenCalledOnce()
  })

  it('does nothing at all when there is neither a line count nor a byte count', () => {
    const { keyboard, selectAll, selectToEof } = selectAllDeps({ getTotalLines: () => null, getTotalBytes: () => 0 })
    keyboard.handleSelectAllShortcut()
    expect(selectAll).not.toHaveBeenCalled()
    expect(selectToEof).not.toHaveBeenCalled()
  })

  it('passes the last line length through when the last line IS cached', () => {
    const { keyboard, selectAll } = selectAllDeps({ getLineText: (n: number) => ['alpha', 'beta', 'gamma'][n] })
    keyboard.handleSelectAllShortcut()
    expect(selectAll).toHaveBeenCalledWith({ totalLines: 3, lastLineLength: 5 })
  })

  it('BUG, pinned as-is: an uncached last line silently selects to offset 0 of it', () => {
    // `viewer-keyboard.ts` does `getLineText(totalLines - 1) ?? ''`, and an uncached
    // last line is the common case on a long file (⌘A without having scrolled to the
    // end). ⌘C then copies the file minus its last line, with nothing said. The spec
    // lists this under "pre-existing bugs in the blast radius"; milestone 7 takes the
    // `RangeEnd::Eof` path instead, and this expectation changes with it.
    const { keyboard, selectAll } = selectAllDeps({ getLineText: (n: number) => (n === 2 ? undefined : 'alpha') })
    keyboard.handleSelectAllShortcut()
    expect(selectAll).toHaveBeenCalledWith({ totalLines: 3, lastLineLength: 0 })
  })

  it('recognises a whole-file selection by either of its two shapes', () => {
    expect(isWholeFileSelection(makeSelectAll(3, 5), 3)).toBe(true)
    expect(isWholeFileSelection(makeSelectToEof(), null)).toBe(true)
    // A selection that starts anywhere but the very top is not the whole file, so the
    // copy flow walks lines for its size instead of taking `totalBytes`.
    expect(isWholeFileSelection({ anchor: { line: 0, offset: 1 }, focus: { line: EOF_LINE, offset: 0 } }, 3)).toBe(
      false,
    )
  })
})

describe('copy size arithmetic, as it behaves today', () => {
  // "alpha\nbeta\ngamma", 16 bytes, no trailing newline.
  const noTrailingNewline = ['alpha', 'beta', 'gamma']

  function estimate(sel: Selection, lines: string[]): number | null {
    const { bytes, utf16 } = lineLookups(lines)
    return estimateSelectionBytes(sel, bytes, utf16)
  }

  it('sizes a whole-file selection of a file with NO trailing newline exactly', () => {
    // The per-line "+1 for the newline" and the end-line "-1" cancel out here, so the
    // estimate happens to land on the true 16 bytes. Pinned because the row rewrite
    // makes both terms wrong on a continuation row, and this is the number it moves off.
    expect(estimate({ anchor: { line: 0, offset: 0 }, focus: { line: 2, offset: 5 } }, noTrailingNewline)).toBe(16)
  })

  it('sizes a whole-file selection of a file WITH a trailing newline exactly', () => {
    // Same three lines plus the trailing empty line every backend reports: 17 bytes.
    expect(estimate({ anchor: { line: 0, offset: 0 }, focus: { line: 3, offset: 0 } }, [...noTrailingNewline, ''])).toBe(
      17,
    )
  })

  it('sizes a selection inside one line', () => {
    expect(estimate({ anchor: { line: 0, offset: 1 }, focus: { line: 0, offset: 4 } }, noTrailingNewline)).toBe(3)
  })

  it('sizes a selection spanning lines, newline bytes included', () => {
    // "pha\nbeta\ngam" = 3 + 1 + 4 + 1 + 3.
    expect(estimate({ anchor: { line: 0, offset: 2 }, focus: { line: 2, offset: 3 } }, noTrailingNewline)).toBe(12)
  })

  it('is an approximation on a partial selection of a non-ASCII line, by design', () => {
    // "héllo" is 5 UTF-16 units and 6 UTF-8 bytes; the estimator prorates bytes by the
    // UTF-16 fraction rather than measuring, so "hé" (3 bytes) comes out as 2. The
    // comment in `selection.svelte.ts` calls this out: the number feeds the 10 MiB
    // confirm and 100 MiB refuse tiers, which need order-of-magnitude, not exactness.
    expect(estimate({ anchor: { line: 0, offset: 0 }, focus: { line: 0, offset: 2 } }, ['héllo'])).toBe(2)
  })

  it('returns null when a line the walk needs is not cached', () => {
    const { utf16 } = lineLookups(noTrailingNewline)
    expect(
      estimateSelectionBytes({ anchor: { line: 0, offset: 0 }, focus: { line: 2, offset: 5 } }, () => null, utf16),
    ).toBeNull()
  })

  it('sizes an empty selection as zero', () => {
    expect(estimate({ anchor: { line: 1, offset: 2 }, focus: { line: 1, offset: 2 } }, noTrailingNewline)).toBe(0)
  })
})
