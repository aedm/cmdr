/**
 * Regression test for the fraction-seek divisor in `createViewerScroll.fetchLines`.
 *
 * When the backend can't seek by line (`getTotalRows() === null`) the seek is sent as a
 * fraction (`fetchFrom / estimatedTotalRows()`). If the estimate is 0, that division
 * yields `NaN` (0/0) or `Infinity` (>0/0); both serialize to JSON `null` over IPC and the
 * Rust `viewer_get_lines` command rejects the `f64 targetValue` ("invalid type: null,
 * expected f64"). This crashed the line fetch in production (ERR-9XYEF, ERR-6JYVE).
 *
 * The contract: the value sent to the backend must always be a finite number.
 */

import { afterEach, describe, expect, it, vi } from 'vitest'

import {
  CACHE_EVICT_ABOVE,
  createViewerScroll,
  getLineHeight,
  rowsToEvict,
  renderWindowRows,
} from './viewer-scroll.svelte'
import { FETCH_BATCH } from './viewer-row-fetch.svelte'
import { EOF_ROW } from './selection.svelte'
import type { LineChunk, ViewerError, ViewerRow } from '$lib/ipc/bindings'
import { clearIpcMocks, installIpcMock } from '$lib/ipc/test-helpers'
import { getAppLogger } from '$lib/logging/logger'

afterEach(() => {
  clearIpcMocks()
})

/** `n` ordinary rows starting at `first`: one per physical line, none continued. */
function plainRows(first: number, n: number): ViewerRow[] {
  return Array.from({ length: n }, (_, i) => ({
    text: `line ${String(first + i)}`,
    byteOffset: (first + i) * 8,
    continues: false,
    lineNumber: first + i,
  }))
}

/** A backend answer in the row shape. Defaults to one row and end-of-file. */
function rowChunk(overrides: Partial<LineChunk> = {}): LineChunk {
  return {
    rows: plainRows(0, 1),
    firstRowNumber: 0,
    byteOffset: 0,
    endByteOffset: 8,
    end: 'endOfFile',
    totalRows: { kind: 'estimated', rows: 1 },
    totalBytes: 1000,
    ...overrides,
  }
}

const chunk: LineChunk = rowChunk()

describe('createViewerScroll fraction seek', () => {
  it('sends a finite targetValue when the line-count estimate drops to 0 before a scheduled fetch fires', async () => {
    const ipc = installIpcMock()
    ipc.mock('viewer_get_lines', () => chunk)

    // A byte-seek backend that doesn't know its total lines. A 0 estimate on its own draws
    // no rows and so fetches nothing; the divisor still meets 0 when a fetch was scheduled
    // over a real range and the estimate dropped inside the debounce window.
    let estimate = 100
    const scroll = createViewerScroll({
      getSessionId: () => 'sess-1',
      getTotalRows: () => null,
      setTotalRows: () => {},
      getEstimatedRows: () => estimate,
      getBackendType: () => 'byteSeek',
      onTimeoutError: () => {},
      getAllRowTexts: () => null,
      getTextWidth: () => 0,
    })

    scroll.runFetchEffect()
    estimate = 0
    await vi.waitFor(() => {
      expect(ipc.lastCall('viewer_get_lines')).toBeDefined()
    })

    const call = ipc.lastCall('viewer_get_lines')
    expect(call?.payload).toMatchObject({ targetType: 'fraction' })
    const targetValue = (call?.payload as { targetValue: number }).targetValue
    expect(Number.isFinite(targetValue)).toBe(true)
  })
})

describe('createViewerScroll range after the line count shrinks', () => {
  it('never asks for a negative line count when the real total lands below the scroll position', async () => {
    // A byte-seek open estimates 20 000 lines; the user scrolls to line ~10 000; then the
    // line index finishes and the file turns out to have 3 000 (ERR-VDVHD: count -6724).
    const ipc = installIpcMock()
    ipc.mock('viewer_get_lines', () => chunk)
    let totalLines = $state<number | null>(null)
    const scroll = createViewerScroll({
      getSessionId: () => 'sess-1',
      getTotalRows: () => totalLines,
      setTotalRows: (v) => {
        totalLines = v
      },
      getEstimatedRows: () => 20_000,
      getBackendType: () => 'byteSeek',
      onTimeoutError: () => {},
      getAllRowTexts: () => null,
      getTextWidth: () => 0,
    })
    const el = document.createElement('div')
    Object.defineProperty(el, 'clientHeight', { value: 600 })
    Object.defineProperty(el, 'scrollHeight', { value: 20_000 * getLineHeight() })
    el.scrollTop = 10_000 * getLineHeight()
    scroll.contentRef = el
    scroll.handleScroll()

    totalLines = 3_000
    scroll.fetchVisibleNow()
    await new Promise((r) => setTimeout(r, 0))

    const counts = ipc.calls
      .filter((c) => c.command === 'viewer_get_lines')
      .map((c) => (c.payload as { count: number }).count)
    expect(counts.every((count) => count > 0)).toBe(true)
    // Past the end of the file there's nothing to draw, so there's nothing to fetch either.
    expect(counts).toEqual([])
    expect(scroll.visibleFrom).toBeLessThanOrEqual(3_000)
    expect(scroll.visibleRows).toEqual([])
  })
})

describe('createViewerScroll.ensureRowVisible', () => {
  /** A scroll composable wired to a fake scroller of `scrollHeight` in a `clientHeight` box. */
  function wireWithScroller(scrollHeight: number, clientHeight: number) {
    installIpcMock().mock('viewer_get_lines', () => chunk)
    const scroll = createViewerScroll({
      getSessionId: () => 'sess-1',
      // No line index yet, which is exactly when `⌘⇧Down` mints the sentinel.
      getTotalRows: () => null,
      setTotalRows: () => {},
      getEstimatedRows: () => 1000,
      getBackendType: () => 'byteSeek',
      onTimeoutError: () => {},
      getAllRowTexts: () => null,
      getTextWidth: () => 0,
    })
    const el = document.createElement('div')
    // jsdom lays nothing out, so the two geometry reads have to be supplied.
    Object.defineProperty(el, 'scrollHeight', { value: scrollHeight })
    Object.defineProperty(el, 'clientHeight', { value: clientHeight })
    scroll.contentRef = el
    return { scroll, el }
  }

  it('reads the end-of-file sentinel as the end of the file', () => {
    const { scroll, el } = wireWithScroller(50_000, 800)
    el.scrollTop = 0

    scroll.ensureRowVisible(EOF_ROW)

    // Not line arithmetic on `Number.MAX_SAFE_INTEGER`: the bottom of the scroller.
    expect(el.scrollTop).toBe(49_200)
  })

  it('never lands on NaN, which would throw the view to the top of the file', () => {
    const { scroll, el } = wireWithScroller(50_000, 800)
    el.scrollTop = 1234

    scroll.ensureRowVisible(EOF_ROW)

    expect(Number.isNaN(el.scrollTop)).toBe(false)
    expect(el.scrollTop).toBeGreaterThan(0)
  })
})

describe('createViewerScroll.renderedRowText', () => {
  /** A composable over a `totalRows`-row file, unscrolled, at the default 600px viewport. */
  function wire(totalRows: number) {
    return createViewerScroll({
      getSessionId: () => 'sess-1',
      getTotalRows: () => totalRows,
      setTotalRows: () => {},
      getEstimatedRows: () => totalRows,
      getBackendType: () => 'lineIndex',
      onTimeoutError: () => {},
      getAllRowTexts: () => null,
      getTextWidth: () => 0,
    })
  }

  it('hands back the cached text of a rendered row', () => {
    const scroll = wire(11)
    scroll.cacheRows(3, [{ text: 'hello', byteOffset: 24, continues: false, lineNumber: 3 }])

    expect(scroll.renderedRowText(3)).toBe('hello')
  })

  it("reads a rendered row the cache doesn't hold as the empty row the template draws for it", () => {
    // A trailing newline makes the `lineIndex` backend count a last row it never emits:
    // `totalRows` 11 for real rows 0-9. The template still draws a row for row 10.
    const scroll = wire(11)
    scroll.cacheRows(0, plainRows(0, 10))

    expect(scroll.renderedRowText(10)).toBe('')
  })

  it('stays undefined outside the rendered range, so the caller knows to scroll and retry', () => {
    // 600px of viewport plus the buffer reaches row ~84 of 40 001, nowhere near the end.
    const scroll = wire(40_001)

    expect(scroll.renderedRowText(40_000)).toBeUndefined()
  })
})

describe('createViewerScroll.visibleRows', () => {
  function wire(totalRows: number) {
    return createViewerScroll({
      getSessionId: () => '',
      getTotalRows: () => totalRows,
      setTotalRows: () => {},
      getEstimatedRows: () => totalRows,
      getBackendType: () => 'lineIndex',
      onTimeoutError: () => {},
      getAllRowTexts: () => null,
      getTextWidth: () => 0,
    })
  }

  it('carries the gutter number and the continuation flag the BACKEND stated, never an inference', () => {
    // One 3-row line: the gutter prints its number once, on the row that starts it, and
    // the two rows Cmdr broke out of it print nothing and carry the marker instead.
    const scroll = wire(4)
    scroll.cacheRows(0, [
      { text: 'aaa', byteOffset: 0, continues: true, lineNumber: 0 },
      { text: 'bbb', byteOffset: 3, continues: true, lineNumber: null },
      { text: 'ccc', byteOffset: 6, continues: false, lineNumber: null },
      { text: 'next', byteOffset: 10, continues: false, lineNumber: 1 },
    ])

    expect(scroll.visibleRows).toEqual([
      { rowNumber: 0, text: 'aaa', continues: true, lineNumber: 0 },
      { rowNumber: 1, text: 'bbb', continues: true, lineNumber: null },
      { rowNumber: 2, text: 'ccc', continues: false, lineNumber: null },
      { rowNumber: 3, text: 'next', continues: false, lineNumber: 1 },
    ])
  })

  it('draws an uncached row blank, with NO gutter number to invent a line it cannot know', () => {
    const scroll = wire(2)
    scroll.cacheRows(0, plainRows(0, 1))

    expect(scroll.visibleRows[1]).toEqual({ rowNumber: 1, text: '', continues: false, lineNumber: null })
  })
})

describe('renderWindowRows', () => {
  it('spans the viewport plus its buffer at the ordinary line height', () => {
    // 600 px of viewport is 34 lines of 18 px, plus 50 buffer lines either side: the
    // window the viewer has always drawn.
    expect(renderWindowRows({ scrollTop: 0, viewportHeight: 600, scrollScale: 1, lineHeight: 18, totalRows: 40_001 }))
      .toEqual({ from: 0, to: 84 })
  })

  it('holds a viewport of PIXELS, not a count of rows, when the rows are tall', () => {
    // A 20 KB row with word wrap on is ~200 visual lines, about 3 600 px. Counting rows
    // would draw 84 of them for one 600 px viewport: ~300 000 px of DOM.
    const { from, to } = renderWindowRows({
      scrollTop: 0,
      viewportHeight: 600,
      scrollScale: 1,
      lineHeight: 3600,
      totalRows: 10_000,
    })

    expect((to - from) * 3600).toBeLessThan(30_000)
  })

  it('reads the scroll position through the scale, so a squeezed spacer does not widen the window', () => {
    // Past ~1.6M lines the spacer is scaled down to stay under WebKit's height cap. The
    // viewport still shows 600 real pixels; dividing THAT by the scale as well is how a
    // huge file ends up rendering tens of thousands of lines at once.
    const scaled = renderWindowRows({
      scrollTop: 1_000,
      viewportHeight: 600,
      scrollScale: 0.01,
      lineHeight: 18,
      totalRows: 5_000_000,
    })

    expect(scaled.to - scaled.from).toBeLessThan(200)
    // And it still lands where the user is looking: 1 000 px into a spacer squeezed 100x.
    expect(scaled.from).toBe(Math.floor(100_000 / 18) - 50)
  })

  it('never reaches past the end of the file, or before its start', () => {
    expect(renderWindowRows({ scrollTop: 0, viewportHeight: 600, scrollScale: 1, lineHeight: 18, totalRows: 5 }))
      .toEqual({ from: 0, to: 5 })
  })
})

describe('rowsToEvict', () => {
  it('keeps everything inside the keep window and drops everything outside it', () => {
    expect(rowsToEvict([0, 99, 100, 500, 899, 900, 1000], { from: 100, to: 900 })).toEqual([0, 99, 900, 1000])
  })

  it('keeps a line exactly on the lower bound and drops one exactly on the upper, which is exclusive', () => {
    expect(rowsToEvict([100, 899, 900], { from: 100, to: 900 })).toEqual([900])
  })

  it('evicts nothing when the keep window covers the cache', () => {
    expect(rowsToEvict([3, 4, 5], { from: 0, to: 10 })).toEqual([])
  })
})

describe('createViewerScroll cache growth over a long scroll', () => {
  /** A backend that serves any requested line range in full. */
  function fullAnsweringBackend(available: number) {
    const ipc = installIpcMock()
    ipc.mock('viewer_get_lines', (payload) => {
      const { targetValue, count } = payload as { targetValue: number; count: number }
      const first = Math.max(0, Math.round(targetValue))
      const served = Math.max(0, Math.min(first + count, available) - first)
      return rowChunk({
        rows: plainRows(first, served),
        firstRowNumber: first,
        byteOffset: first * 8,
        endByteOffset: (first + served) * 8,
        end: first + served >= available ? 'endOfFile' : 'countReached',
        totalRows: { kind: 'exact', rows: available },
        totalBytes: available * 8,
      })
    })
    return ipc
  }

  /** Drags down `stops` times, `screensPerStop` viewports at a time, fetching at each stop. */
  async function scrollThrough(
    scroll: ReturnType<typeof createViewerScroll>,
    el: HTMLElement,
    { stops, screensPerStop }: { stops: number; screensPerStop: number },
  ) {
    for (let stop = 1; stop <= stops; stop++) {
      el.scrollTop = stop * screensPerStop * 600
      scroll.handleScroll()
      scroll.fetchVisibleNow()
      await new Promise((r) => setTimeout(r, 0))
    }
  }

  it('drops lines far from the viewport instead of parking the whole file in the renderer', async () => {
    // 40 KB a row is where this stops being theoretical: scrolling through 1% of a 50 GB
    // file would otherwise hold ~500 MB of text in the webview with nothing drawing it.
    const ipc = fullAnsweringBackend(100_000)
    const scroll = createViewerScroll({
      getSessionId: () => 'sess-1',
      getTotalRows: () => 100_000,
      setTotalRows: () => {},
      getEstimatedRows: () => 100_000,
      getBackendType: () => 'lineIndex',
      onTimeoutError: () => {},
      getAllRowTexts: () => null,
      getTextWidth: () => 0,
    })
    const el = document.createElement('div')
    Object.defineProperty(el, 'clientHeight', { value: 600 })
    scroll.contentRef = el

    await scrollThrough(scroll, el, { stops: 40, screensPerStop: 20 })
    expect(ipc.callCount('viewer_get_lines')).toBe(40)

    expect(scroll.rowCache.size).toBeLessThanOrEqual(CACHE_EVICT_ABOVE + FETCH_BATCH)
    // And what's on screen survived: eviction that drops a rendered row draws blank rows.
    for (const { rowNumber } of scroll.visibleRows) {
      expect(scroll.rowCache.has(rowNumber)).toBe(true)
    }
  })

  it('keeps every line on a fullLoad file, whose height map measures the whole thing', async () => {
    const lines = 6_000
    fullAnsweringBackend(lines)
    const scroll = createViewerScroll({
      getSessionId: () => 'sess-1',
      getTotalRows: () => lines,
      setTotalRows: () => {},
      getEstimatedRows: () => lines,
      getBackendType: () => 'fullLoad',
      onTimeoutError: () => {},
      getAllRowTexts: () => null,
      getTextWidth: () => 0,
    })
    const el = document.createElement('div')
    Object.defineProperty(el, 'clientHeight', { value: 600 })
    scroll.contentRef = el

    await scrollThrough(scroll, el, { stops: 40, screensPerStop: 4 })

    expect(scroll.rowCache.size).toBeGreaterThan(CACHE_EVICT_ABOVE)
  })
})

describe('createViewerScroll when the backend answers with fewer lines than asked for', () => {
  /**
   * A `viewer_get_lines` that never returns more than `cap` lines at a time, out of a file
   * that really holds `available` of them. That's what a per-chunk byte budget looks like
   * from here: ~104 rows of 20 000 bytes against the ~267 the frontend asked for.
   */
  function shortAnsweringBackend(cap: number, available: number) {
    const ipc = installIpcMock()
    ipc.mock('viewer_get_lines', (payload) => {
      const { targetValue, count } = payload as { targetValue: number; count: number }
      const first = Math.max(0, Math.round(targetValue))
      const served = Math.max(0, Math.min(first + Math.min(cap, count), available) - first)
      const atEnd = first + served >= available
      return rowChunk({
        rows: plainRows(first, served),
        firstRowNumber: first,
        byteOffset: first * 8,
        endByteOffset: (first + served) * 8,
        // A chunk cut short by the per-answer byte budget SAYS so; one that ran out of
        // file says that instead. The frontend may read only this, never the row count.
        end: atEnd ? 'endOfFile' : served < count ? 'budgetReached' : 'countReached',
        totalRows: { kind: 'exact', rows: available },
        totalBytes: available * 8,
      })
    })
    return ipc
  }

  /** A `lineIndex`-backed composable over a `totalRows`-row file at the default viewport. */
  function wire(totalRows: number) {
    return createViewerScroll({
      getSessionId: () => 'sess-1',
      getTotalRows: () => totalRows,
      setTotalRows: () => {},
      getEstimatedRows: () => totalRows,
      getBackendType: () => 'lineIndex',
      onTimeoutError: () => {},
      getAllRowTexts: () => null,
      getTextWidth: () => 0,
    })
  }

  /**
   * Runs the fetch effect until a round adds no IPC call, the way the page's `$effect`
   * re-runs on every cache change. Returns the total call count. Throws when it never
   * settles, which is the spin: `needsFetch` stays true on the last row of a range the
   * backend won't fill, and a refetch fires every debounce forever.
   */
  async function settleFetches(scroll: ReturnType<typeof createViewerScroll>, ipc: ReturnType<typeof installIpcMock>) {
    for (let round = 0; round < 12; round++) {
      const before = ipc.callCount('viewer_get_lines')
      scroll.runFetchEffect()
      // The fetch debounce is 100 ms; the continuation chain after it runs unqueued.
      await new Promise((r) => setTimeout(r, 160))
      if (ipc.callCount('viewer_get_lines') === before) return ipc.callCount('viewer_get_lines')
    }
    throw new Error(`the fetch loop never settled: ${String(ipc.callCount('viewer_get_lines'))} calls and still going`)
  }

  it('asks again from the last row it actually received, and stops once the range is filled', async () => {
    const ipc = shortAnsweringBackend(10, 40)
    const scroll = wire(40)

    const calls = await settleFetches(scroll, ipc)

    // Four ten-row answers cover the rendered range; nothing fires after that.
    expect(calls).toBe(4)
    for (let i = 0; i < 40; i++) expect(scroll.rowCache.get(i)?.text).toBe(`line ${String(i)}`)
  })

  it('treats a range the backend cannot fill as answered, instead of asking forever', async () => {
    // The row count says 40, the backend only ever yields 35: the `lineIndex` phantom
    // trailing row, or an estimate that overshot. The last rendered row never arrives.
    //
    // ❗ Four calls, not five. The fourth chunk SAYS it reached the end of the file, so
    // the frontend records where the file stops and asks nothing more. Inferring the
    // same thing from "fewer rows than I asked for" costs an extra round trip to be told
    // zero rows — and is the reading that, on a chunk cut short by `CHUNK_BUDGET_BYTES`
    // instead, would silently truncate a copy.
    const ipc = shortAnsweringBackend(10, 35)
    const scroll = wire(40)

    const calls = await settleFetches(scroll, ipc)

    expect(calls).toBe(4)
    expect(scroll.rowCache.get(34)?.text).toBe('line 34')
    expect(scroll.rowCache.has(35)).toBe(false)
  })

  it('keeps walking a chunk that was cut short by the byte budget', async () => {
    // The mirror of the test above: every chunk here is short for the OTHER reason, and
    // stopping on any of them would leave the rendered range half-drawn.
    const ipc = shortAnsweringBackend(10, 25)
    const scroll = wire(30)

    const calls = await settleFetches(scroll, ipc)

    // 10 + 10 + 5, the third saying `endOfFile`: three calls, and all 25 rows cached.
    expect(calls).toBe(3)
    for (let i = 0; i < 25; i++) expect(scroll.rowCache.get(i)?.text).toBe(`line ${String(i)}`)
  })
})

describe("createViewerScroll a read that didn't come back", () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  /** A 100-line file, unscrolled, whose every `viewer_get_lines` fails with `viewerError`. */
  function wireFailingRead(viewerError: ViewerError) {
    installIpcMock().mock('viewer_get_lines', () => {
      throw viewerError
    })
    const onTimeoutError = vi.fn()
    const scroll = createViewerScroll({
      getSessionId: () => 'sess-1',
      getTotalRows: () => 100,
      setTotalRows: () => {},
      getEstimatedRows: () => 100,
      getBackendType: () => 'lineIndex',
      onTimeoutError,
      getAllRowTexts: () => null,
      getTextWidth: () => 0,
    })
    return { scroll, onTimeoutError }
  }

  it('logs a timed-out read at warn, since the window shows the timeout with Retry', async () => {
    // At error level each one filed an error report of its own.
    const warn = vi.spyOn(getAppLogger('viewer'), 'warn')
    const error = vi.spyOn(getAppLogger('viewer'), 'error')
    const { scroll, onTimeoutError } = wireFailingRead({ kind: 'timedOut' })

    scroll.fetchVisibleNow()
    await vi.waitFor(() => {
      expect(onTimeoutError).toHaveBeenCalledTimes(1)
    })

    expect(error).not.toHaveBeenCalled()
    expect(warn).toHaveBeenCalledTimes(1)
  })

  it('keeps a read that failed any other way at error, since nothing on screen says so', async () => {
    const warn = vi.spyOn(getAppLogger('viewer'), 'warn')
    const error = vi.spyOn(getAppLogger('viewer'), 'error')
    const { scroll, onTimeoutError } = wireFailingRead({ kind: 'io', message: 'boom' })

    scroll.fetchVisibleNow()
    await vi.waitFor(() => {
      expect(error).toHaveBeenCalledTimes(1)
    })

    expect(onTimeoutError).not.toHaveBeenCalled()
    expect(warn).not.toHaveBeenCalled()
  })
})

describe('createViewerScroll on a backend that owns the row numbering', () => {
  /**
   * A `byteSeek`-shaped backend: it has no row index, so it resolves a fraction or a byte
   * offset to a row boundary ITSELF and reports which row that turned out to be. That
   * number never matches what the frontend guessed, which is the whole point.
   */
  function numberingBackend({ shift, available }: { shift: number; available: number }) {
    const ipc = installIpcMock()
    ipc.mock('viewer_get_lines', (payload) => {
      const { targetType, targetValue, count } = payload as {
        targetType: string
        targetValue: number
        count: number
      }
      // A fraction lands wherever the backend's own grid puts it; a byte offset resolves
      // exactly. Either way the ANSWER carries the row number, not the request.
      const first =
        targetType === 'byte' ? Math.round(targetValue / 8) : Math.round(targetValue * available) + shift
      const served = Math.max(0, Math.min(count, available - first))
      return rowChunk({
        rows: plainRows(first, served),
        firstRowNumber: first,
        byteOffset: first * 8,
        endByteOffset: (first + served) * 8,
        end: first + served >= available ? 'endOfFile' : 'budgetReached',
        totalRows: { kind: 'estimated', rows: available },
        totalBytes: available * 8,
      })
    })
    return ipc
  }

  function wireByteSeek(estimated: number) {
    const scroll = createViewerScroll({
      getSessionId: () => 'sess-1',
      // No index, so no counted total: every seek goes out as a fraction.
      getTotalRows: () => null,
      setTotalRows: () => {},
      getEstimatedRows: () => estimated,
      getBackendType: () => 'byteSeek',
      onTimeoutError: () => {},
      getAllRowTexts: () => null,
      getTextWidth: () => 0,
    })
    const el = document.createElement('div')
    Object.defineProperty(el, 'clientHeight', { value: 600 })
    scroll.contentRef = el
    return { scroll, el }
  }

  it("caches a fraction-seek answer at the chunk's own first row, not at the row it asked for", async () => {
    // ❗ The bug this pins: caching at `fetchFrom` puts the rows at indexes the backend
    // disagrees with, so its next answer overlaps them and the same bytes get drawn twice
    // at two different scroll positions.
    const ipc = numberingBackend({ shift: 13, available: 2_000 })
    const { scroll, el } = wireByteSeek(2_000)
    el.scrollTop = 500 * getLineHeight()
    scroll.handleScroll()

    scroll.fetchVisibleNow()
    await new Promise((r) => setTimeout(r, 50))

    const call = ipc.lastCall('viewer_get_lines')
    const asked = Math.round((call?.payload as { targetValue: number }).targetValue * 2_000)
    expect(scroll.rowCache.has(asked)).toBe(false)
    expect(scroll.rowCache.get(asked + 13)?.text).toBe(`line ${String(asked + 13)}`)
  })

  it('continues such a chunk from the byte it ended at, so the walk cannot drift', async () => {
    const ipc = numberingBackend({ shift: 13, available: 2_000 })
    const { scroll, el } = wireByteSeek(2_000)
    el.scrollTop = 500 * getLineHeight()
    scroll.handleScroll()

    scroll.fetchVisibleNow()
    await new Promise((r) => setTimeout(r, 50))

    // The first answer went out as a fraction; every continuation after it is a byte
    // seek at the previous chunk's `endByteOffset`.
    const seeks = ipc.calls.filter((c) => c.command === 'viewer_get_lines').map((c) => c.payload as { targetType: string })
    expect(seeks[0].targetType).toBe('fraction')
    expect(seeks.slice(1).every((s) => s.targetType === 'byte')).toBe(true)
  })
})
