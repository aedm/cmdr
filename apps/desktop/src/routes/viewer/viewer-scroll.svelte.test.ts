/**
 * Regression test for the fraction-seek divisor in `createViewerScroll.fetchLines`.
 *
 * When the backend can't seek by line (`getTotalLines() === null`) the seek is sent as a
 * fraction (`fetchFrom / estimatedTotalLines()`). If the estimate is 0, that division
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
  FETCH_BATCH,
  getLineHeight,
  linesToEvict,
  renderWindowLines,
} from './viewer-scroll.svelte'
import { EOF_LINE } from './selection.svelte'
import type { LineChunk, ViewerError } from '$lib/ipc/bindings'
import { clearIpcMocks, installIpcMock } from '$lib/ipc/test-helpers'
import { getAppLogger } from '$lib/logging/logger'

afterEach(() => {
  clearIpcMocks()
})

const chunk: LineChunk = {
  lines: ['x'],
  firstLineNumber: 0,
  byteOffset: 0,
  totalLines: null,
  totalBytes: 1000,
}

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
      getTotalLines: () => null,
      setTotalLines: () => {},
      getEstimatedLines: () => estimate,
      getBackendType: () => 'byteSeek',
      onTimeoutError: () => {},
      getAllLines: () => null,
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
      getTotalLines: () => totalLines,
      setTotalLines: (v) => {
        totalLines = v
      },
      getEstimatedLines: () => 20_000,
      getBackendType: () => 'byteSeek',
      onTimeoutError: () => {},
      getAllLines: () => null,
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
    expect(scroll.visibleLines).toEqual([])
  })
})

describe('createViewerScroll.ensureLineVisible', () => {
  /** A scroll composable wired to a fake scroller of `scrollHeight` in a `clientHeight` box. */
  function wireWithScroller(scrollHeight: number, clientHeight: number) {
    installIpcMock().mock('viewer_get_lines', () => chunk)
    const scroll = createViewerScroll({
      getSessionId: () => 'sess-1',
      // No line index yet, which is exactly when `⌘⇧Down` mints the sentinel.
      getTotalLines: () => null,
      setTotalLines: () => {},
      getEstimatedLines: () => 1000,
      getBackendType: () => 'byteSeek',
      onTimeoutError: () => {},
      getAllLines: () => null,
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

    scroll.ensureLineVisible(EOF_LINE)

    // Not line arithmetic on `Number.MAX_SAFE_INTEGER`: the bottom of the scroller.
    expect(el.scrollTop).toBe(49_200)
  })

  it('never lands on NaN, which would throw the view to the top of the file', () => {
    const { scroll, el } = wireWithScroller(50_000, 800)
    el.scrollTop = 1234

    scroll.ensureLineVisible(EOF_LINE)

    expect(Number.isNaN(el.scrollTop)).toBe(false)
    expect(el.scrollTop).toBeGreaterThan(0)
  })
})

describe('createViewerScroll.renderedLineText', () => {
  /** A composable over a `totalLines`-line file, unscrolled, at the default 600px viewport. */
  function wire(totalLines: number) {
    return createViewerScroll({
      getSessionId: () => 'sess-1',
      getTotalLines: () => totalLines,
      setTotalLines: () => {},
      getEstimatedLines: () => totalLines,
      getBackendType: () => 'lineIndex',
      onTimeoutError: () => {},
      getAllLines: () => null,
      getTextWidth: () => 0,
    })
  }

  it('hands back the cached text of a rendered line', () => {
    const scroll = wire(11)
    scroll.lineCache.set(3, 'hello')

    expect(scroll.renderedLineText(3)).toBe('hello')
  })

  it("reads a rendered line the cache doesn't hold as the empty row the template draws for it", () => {
    // A trailing newline makes the `lineIndex` backend count a last line it never emits:
    // `totalLines` 11 for real lines 0-9. The template still draws a row for line 10.
    const scroll = wire(11)
    for (let i = 0; i < 10; i++) scroll.lineCache.set(i, 'line')

    expect(scroll.renderedLineText(10)).toBe('')
  })

  it('stays undefined outside the rendered range, so the caller knows to scroll and retry', () => {
    // 600px of viewport plus the buffer reaches line ~84 of 40 001, nowhere near the end.
    const scroll = wire(40_001)

    expect(scroll.renderedLineText(40_000)).toBeUndefined()
  })
})

describe('renderWindowLines', () => {
  it('spans the viewport plus its buffer at the ordinary line height', () => {
    // 600 px of viewport is 34 lines of 18 px, plus 50 buffer lines either side: the
    // window the viewer has always drawn.
    expect(renderWindowLines({ scrollTop: 0, viewportHeight: 600, scrollScale: 1, lineHeight: 18, totalLines: 40_001 }))
      .toEqual({ from: 0, to: 84 })
  })

  it('holds a viewport of PIXELS, not a count of rows, when the rows are tall', () => {
    // A 20 KB row with word wrap on is ~200 visual lines, about 3 600 px. Counting rows
    // would draw 84 of them for one 600 px viewport: ~300 000 px of DOM.
    const { from, to } = renderWindowLines({
      scrollTop: 0,
      viewportHeight: 600,
      scrollScale: 1,
      lineHeight: 3600,
      totalLines: 10_000,
    })

    expect((to - from) * 3600).toBeLessThan(30_000)
  })

  it('reads the scroll position through the scale, so a squeezed spacer does not widen the window', () => {
    // Past ~1.6M lines the spacer is scaled down to stay under WebKit's height cap. The
    // viewport still shows 600 real pixels; dividing THAT by the scale as well is how a
    // huge file ends up rendering tens of thousands of lines at once.
    const scaled = renderWindowLines({
      scrollTop: 1_000,
      viewportHeight: 600,
      scrollScale: 0.01,
      lineHeight: 18,
      totalLines: 5_000_000,
    })

    expect(scaled.to - scaled.from).toBeLessThan(200)
    // And it still lands where the user is looking: 1 000 px into a spacer squeezed 100x.
    expect(scaled.from).toBe(Math.floor(100_000 / 18) - 50)
  })

  it('never reaches past the end of the file, or before its start', () => {
    expect(renderWindowLines({ scrollTop: 0, viewportHeight: 600, scrollScale: 1, lineHeight: 18, totalLines: 5 }))
      .toEqual({ from: 0, to: 5 })
  })
})

describe('linesToEvict', () => {
  it('keeps everything inside the keep window and drops everything outside it', () => {
    expect(linesToEvict([0, 99, 100, 500, 899, 900, 1000], { from: 100, to: 900 })).toEqual([0, 99, 900, 1000])
  })

  it('keeps a line exactly on the lower bound and drops one exactly on the upper, which is exclusive', () => {
    expect(linesToEvict([100, 899, 900], { from: 100, to: 900 })).toEqual([900])
  })

  it('evicts nothing when the keep window covers the cache', () => {
    expect(linesToEvict([3, 4, 5], { from: 0, to: 10 })).toEqual([])
  })
})

describe('createViewerScroll cache growth over a long scroll', () => {
  /** A backend that serves any requested line range in full. */
  function fullAnsweringBackend(available: number) {
    const ipc = installIpcMock()
    ipc.mock('viewer_get_lines', (payload) => {
      const { targetValue, count } = payload as { targetValue: number; count: number }
      const first = Math.max(0, Math.round(targetValue))
      const lines: string[] = []
      for (let i = first; i < Math.min(first + count, available); i++) lines.push(`line ${String(i)}`)
      return { lines, firstLineNumber: first, byteOffset: first * 8, totalLines: available, totalBytes: available * 8 }
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
      getTotalLines: () => 100_000,
      setTotalLines: () => {},
      getEstimatedLines: () => 100_000,
      getBackendType: () => 'lineIndex',
      onTimeoutError: () => {},
      getAllLines: () => null,
      getTextWidth: () => 0,
    })
    const el = document.createElement('div')
    Object.defineProperty(el, 'clientHeight', { value: 600 })
    scroll.contentRef = el

    await scrollThrough(scroll, el, { stops: 40, screensPerStop: 20 })
    expect(ipc.callCount('viewer_get_lines')).toBe(40)

    expect(scroll.lineCache.size).toBeLessThanOrEqual(CACHE_EVICT_ABOVE + FETCH_BATCH)
    // And what's on screen survived: eviction that drops a rendered line draws blank rows.
    for (const { lineNumber } of scroll.visibleLines) {
      expect(scroll.lineCache.has(lineNumber)).toBe(true)
    }
  })

  it('keeps every line on a fullLoad file, whose height map measures the whole thing', async () => {
    const lines = 6_000
    fullAnsweringBackend(lines)
    const scroll = createViewerScroll({
      getSessionId: () => 'sess-1',
      getTotalLines: () => lines,
      setTotalLines: () => {},
      getEstimatedLines: () => lines,
      getBackendType: () => 'fullLoad',
      onTimeoutError: () => {},
      getAllLines: () => null,
      getTextWidth: () => 0,
    })
    const el = document.createElement('div')
    Object.defineProperty(el, 'clientHeight', { value: 600 })
    scroll.contentRef = el

    await scrollThrough(scroll, el, { stops: 40, screensPerStop: 4 })

    expect(scroll.lineCache.size).toBeGreaterThan(CACHE_EVICT_ABOVE)
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
      const lines: string[] = []
      for (let i = first; i < Math.min(first + Math.min(cap, count), available); i++) lines.push(`line ${String(i)}`)
      return {
        lines,
        firstLineNumber: first,
        byteOffset: first * 8,
        totalLines: available,
        totalBytes: available * 8,
      } satisfies LineChunk
    })
    return ipc
  }

  /** A `lineIndex`-backed composable over a `totalLines`-line file at the default viewport. */
  function wire(totalLines: number) {
    return createViewerScroll({
      getSessionId: () => 'sess-1',
      getTotalLines: () => totalLines,
      setTotalLines: () => {},
      getEstimatedLines: () => totalLines,
      getBackendType: () => 'lineIndex',
      onTimeoutError: () => {},
      getAllLines: () => null,
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

  it('asks again from the last line it actually received, and stops once the range is filled', async () => {
    const ipc = shortAnsweringBackend(10, 40)
    const scroll = wire(40)

    const calls = await settleFetches(scroll, ipc)

    // Four ten-line answers cover the rendered range; nothing fires after that.
    expect(calls).toBe(4)
    for (let i = 0; i < 40; i++) expect(scroll.lineCache.get(i)).toBe(`line ${String(i)}`)
  })

  it('treats a range the backend cannot fill as answered, instead of asking forever', async () => {
    // The line count says 40, the backend only ever yields 35: the `lineIndex` phantom
    // trailing line, or an estimate that overshot. The last rendered row never arrives.
    const ipc = shortAnsweringBackend(10, 35)
    const scroll = wire(40)

    const calls = await settleFetches(scroll, ipc)

    expect(calls).toBeLessThanOrEqual(6)
    expect(scroll.lineCache.get(34)).toBe('line 34')
    expect(scroll.lineCache.has(35)).toBe(false)
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
      getTotalLines: () => 100,
      setTotalLines: () => {},
      getEstimatedLines: () => 100,
      getBackendType: () => 'lineIndex',
      onTimeoutError,
      getAllLines: () => null,
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
