import { SvelteMap } from 'svelte/reactivity'
import { viewerGetLines, asViewerError, type LineChunk, type ViewerRow } from '$lib/tauri-commands'
import { getAppLogger } from '$lib/logging/logger'
import { createRowHeightMap, getLineHeight } from './viewer-line-heights.svelte'
import { onDebouncedScaleChange } from '$lib/text-size.svelte'
import { pluralize } from '$lib/utils/pluralize'
import { dependOn } from '$lib/utils/reactivity'
import { ensureVisibleOffset, recenterOffset } from './viewer-search-scroll'
import { caretRectFor, measureColumnWidth } from './viewer-pointer'
import { EOF_ROW, type RowOffset } from './selection.svelte'

const log = getAppLogger('viewer')

/**
 * How much content to keep drawn on each side of the viewport, in CSS pixels, and the
 * row counts that budget is clamped between.
 *
 * ❗ PIXELS, not rows. 900 px is exactly the 50 rows the viewer has always buffered at
 * the default 18 px line height, so ordinary files are unaffected. The difference shows
 * with word wrap on, where one row can be hundreds of pixels tall: 50 SUCH rows would
 * paint hundreds of thousands of pixels for a 600 px viewport, and the DOM height map
 * that would otherwise catch it only ever engages on `fullLoad` files.
 */
const BUFFER_PX = 900
const BUFFER_ROWS_MAX = 50
const BUFFER_ROWS_MIN = 2

export const FETCH_BATCH = 500

/**
 * Rows this far outside the rendered window survive eviction: one fetch batch on either
 * side, so ordinary scrolling never re-fetches what it just dropped.
 */
const CACHE_KEEP_MARGIN = FETCH_BATCH

/**
 * Cache size that starts an eviction pass. Below it the map isn't worth walking, and the
 * gap to the keep window gives the pass hysteresis: it runs rarely and drops a lot.
 *
 * The number that matters is bytes, not rows. An ordinary row is a few dozen bytes, but
 * a row inside a long line runs to ~40 KB, which puts the ceiling near 80 MB of text held
 * in the renderer. The alternative was unbounded: the cache was cleared only on open,
 * reload, and encoding change, so scrolling through 1% of a 50 GB file parked ~500 MB in
 * the webview with nothing drawing it.
 */
export const CACHE_EVICT_ABOVE = FETCH_BATCH * 4

// WebKit caps element height at ~2^25 px (33.5M). Stay well below to avoid scroll cutoff.
const MAX_SCROLL_HEIGHT = 30_000_000
const FETCH_DEBOUNCE_MS = 100
// A full DOM re-measure of every line (on a width change) is ~70 ms, so it can't
// run on every ResizeObserver frame during a live window drag. Debounce to the
// resize-settle, the same way the text-size slider path debounces (see
// `onDebouncedScaleChange`). Between drag and settle the CSS re-wraps live; only
// the scroll geometry lags a beat, then snaps into place.
const REFLOW_DEBOUNCE_MS = 150

/**
 * One cached row: what the backend served for it.
 *
 * ❗ `continues` and `lineNumber` are FACTS the backend states, ❌ never inferred here.
 * `continues` says Cmdr ended the row at a segment boundary rather than at a newline the
 * file holds, which drives the continuation marker and zeroes the row's delimiter byte.
 * `lineNumber` is the 0-based physical line the row STARTS, or `null` on a continuation
 * row, which is what lets the gutter print a number once per line.
 */
export interface CachedRow {
  text: string
  continues: boolean
  lineNumber: number | null
}

interface ScrollDeps {
  getSessionId: () => string
  /** Counted rows, or `null` while only an estimate exists. */
  getTotalRows: () => number | null
  setTotalRows: (v: number) => void
  getEstimatedRows: () => number
  getBackendType: () => 'fullLoad' | 'byteSeek' | 'lineIndex'
  onTimeoutError: () => void
  /** Every row's text, in order, for the height map. `fullLoad` only. */
  getAllRowTexts: () => string[] | null
  getTextWidth: () => number
}

export { getLineHeight, MAX_SCROLL_HEIGHT }

/** How many rows of `lineHeight` fit in the off-screen pixel buffer, within its clamps. */
export function bufferRows(lineHeight: number): number {
  if (!(lineHeight > 0)) return BUFFER_ROWS_MAX
  return Math.max(BUFFER_ROWS_MIN, Math.min(BUFFER_ROWS_MAX, Math.ceil(BUFFER_PX / lineHeight)))
}

/**
 * The half-open ROW range `[from, to)` to draw for a viewport, sized in PIXELS: a
 * viewport's worth of content plus the off-screen buffer, whatever the rows contain.
 * Both ends clamp to the file, so `from <= to` always holds. (When the row count lands
 * below the current scroll position, a byte-seek estimate replaced by the real index,
 * `scrollTop` points past the end for a beat; an unclamped `from` then overtook `to` and
 * the fetch asked for a negative count, ERR-VDVHD.)
 *
 * ❗ `scrollScale` converts the scroll position and nothing else. Past ~1.6M rows the
 * spacer is squeezed to stay under WebKit's element-height cap, so `scrollTop` is in
 * squeezed pixels while the viewport still shows real ones. Dividing the viewport height
 * by the scale as well is how a huge file ends up drawing tens of thousands of rows at
 * once.
 */
export function renderWindowRows({
  scrollTop,
  viewportHeight,
  scrollScale,
  lineHeight,
  totalRows,
}: {
  scrollTop: number
  viewportHeight: number
  scrollScale: number
  lineHeight: number
  totalRows: number
}): { from: number; to: number } {
  const height = Math.max(1, lineHeight)
  const top = scrollScale < 1 ? scrollTop / scrollScale : scrollTop
  const buffer = bufferRows(height)
  return {
    from: Math.min(totalRows, Math.max(0, Math.floor(top / height) - buffer)),
    to: Math.min(totalRows, Math.ceil((top + viewportHeight) / height) + buffer),
  }
}

/**
 * The cached row indexes outside `keep`, which is half-open `[from, to)`.
 *
 * ❗ The caller owes it a window that covers everything on screen plus its fetch buffer.
 * Evicting a row the render window still wants draws a blank row, and the fetch effect
 * pulls it straight back, so a too-tight window is a churn machine, not just a glitch.
 */
export function rowsToEvict(cached: Iterable<number>, keep: { from: number; to: number }): number[] {
  const evictable: number[] = []
  for (const row of cached) {
    if (row < keep.from || row >= keep.to) evictable.push(row)
  }
  return evictable
}

export function createViewerScroll(deps: ScrollDeps) {
  const rowCache = new SvelteMap<number, CachedRow>()
  const heightMap = createRowHeightMap()

  let scrollTop = $state(0)
  let viewportHeight = $state(600)
  let contentRef: HTMLDivElement | undefined = $state()
  let containerRef: HTMLElement | undefined = $state()
  let linesContainerRef: HTMLDivElement | undefined = $state()

  let contentWidth = $state(0)

  let wordWrap = $state(false)
  let avgWrappedLineHeight = $state(getLineHeight())
  // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- wordWrap is reactive $state
  const effectiveLineHeight = $derived(wordWrap ? avgWrappedLineHeight : getLineHeight())

  let fetchDebounceTimer: ReturnType<typeof setTimeout> | undefined
  let currentFetchId = 0

  function estimatedTotalRows(): number {
    const totalRows = deps.getTotalRows()
    if (totalRows !== null) return totalRows
    return deps.getEstimatedRows()
  }

  const scrollScale = $derived.by(() => {
    const totalHeight = heightMap.ready ? heightMap.getTotalHeight() : estimatedTotalRows() * effectiveLineHeight
    return totalHeight > MAX_SCROLL_HEIGHT ? MAX_SCROLL_HEIGHT / totalHeight : 1
  })
  const scrollLineHeight = $derived(effectiveLineHeight * scrollScale)

  /**
   * The rendered range, from measured heights when the height map is ready and from the
   * (also measured) average row height otherwise. Both paths spend a fixed pixel budget
   * rather than a fixed number of rows: `BUFFER_PX` above and below the viewport.
   */
  const renderWindow = $derived.by(() => {
    const total = estimatedTotalRows()
    if (heightMap.ready) {
      const unscaledY = scrollScale < 1 ? scrollTop / scrollScale : scrollTop
      return {
        from: Math.min(total, Math.max(0, heightMap.getRowAtPosition(Math.max(0, unscaledY - BUFFER_PX)))),
        to: Math.min(total, heightMap.getRowAtPosition(unscaledY + viewportHeight + BUFFER_PX) + 1),
      }
    }
    return renderWindowRows({
      scrollTop,
      viewportHeight,
      scrollScale,
      lineHeight: effectiveLineHeight,
      totalRows: total,
    })
  })

  const visibleFrom = $derived(renderWindow.from)
  const visibleTo = $derived(renderWindow.to)

  const spacerHeight = $derived(
    heightMap.ready ? heightMap.getTotalHeight() * scrollScale : estimatedTotalRows() * scrollLineHeight,
  )

  const rowsOffset = $derived(
    heightMap.ready ? heightMap.getRowTop(visibleFrom) * scrollScale : visibleFrom * scrollLineHeight,
  )

  /** One past the last row the template draws. `visibleTo` alone can overshoot the file. */
  const renderedTo = $derived(Math.min(visibleTo, estimatedTotalRows()))

  const visibleRows = $derived(getVisibleRows())
  /**
   * Gutter width in `ch`, sized off the ROW total. A physical line number never exceeds
   * the row index it sits on, so the row total is a safe upper bound and is the one
   * number that's always known.
   */
  const gutterWidth = $derived(String(estimatedTotalRows()).length)

  /**
   * The rows the template draws. A row the cache missed draws as an empty one with NO
   * gutter number: the physical line it belongs to is a fact only the backend has, and
   * printing the row index there would be a wrong line number on any wrapped file.
   */
  function getVisibleRows(): Array<{ rowNumber: number; text: string; continues: boolean; lineNumber: number | null }> {
    const result: Array<{ rowNumber: number; text: string; continues: boolean; lineNumber: number | null }> = []
    for (let i = visibleFrom; i < renderedTo; i++) {
      const row = rowCache.get(i)
      result.push({
        rowNumber: i,
        text: row?.text ?? '',
        continues: row?.continues ?? false,
        lineNumber: row?.lineNumber ?? null,
      })
    }
    return result
  }

  /**
   * The text the template SHOWS for a row, which is what the caret motion model has to
   * reason about, or `undefined` when no row is drawn for it yet.
   *
   * Inside the rendered range a cache miss draws as an empty row (`getVisibleRows`
   * applies the same `?? ''`), so a caller reading the cache directly would disagree with
   * the user's screen about which rows exist. That divergence is fatal for a file ending
   * in a newline on the `lineIndex` backend: it counts a last row `viewer_get_lines`
   * never emits, and ⌘⇧Down would ask for it forever.
   *
   * OUTSIDE the range `undefined` keeps its other meaning: not fetched yet, scroll and
   * retry. `moveFocus` turns that into `{ focus: null, targetRow }`, and the keyboard's
   * scroll is what pulls the row in so the next press lands. Widening the `''` past the
   * rendered range would break that two-press flow, and invent an offset for a row
   * nobody has seen.
   */
  function renderedRowText(row: number): string | undefined {
    const cached = rowCache.get(row)
    if (cached !== undefined) return cached.text
    return row >= visibleFrom && row < renderedTo ? '' : undefined
  }

  /** Returns the scaled Y offset for row n. Used by search for scroll-to-match. */
  function getRowTop(n: number): number {
    if (heightMap.ready) {
      return heightMap.getRowTop(n) * scrollScale
    }
    return n * scrollLineHeight
  }

  /** Returns the row at the current viewport top, using the height map (not the DOM buffer). */
  function getAnchorRow(): number {
    const unscaledY = scrollScale < 1 ? scrollTop / scrollScale : scrollTop
    return heightMap.getRowAtPosition(unscaledY)
  }

  /**
   * The first row the backend answered nothing for, with the row count it answered
   * under. Past that row there is nothing to fetch, so asking again would be the spin
   * this guards: `needsFetch` would stay true on a row that doesn't exist and a refetch
   * would fire every `FETCH_DEBOUNCE_MS` forever. The recorded total is the expiry: a
   * reload, an encoding switch, or a tail append moves it, and the row may exist then.
   *
   * ❗ Only a SUCCESSFUL answer of zero rows sets this. A read that failed throws, and a
   * failure must stay retryable.
   */
  let noRowsBeyond: { row: number; underTotal: number } | null = null

  /** One past the last row worth asking for: the backend has nothing at or after it. */
  function fetchableTo(): number {
    if (noRowsBeyond === null) return Infinity
    if (noRowsBeyond.underTotal !== estimatedTotalRows()) {
      noRowsBeyond = null
      return Infinity
    }
    return noRowsBeyond.row
  }

  function needsFetch(from: number, to: number): boolean {
    const limit = fetchableTo()
    const samplesToCheck = [from, Math.floor((from + to) / 2), to - 1]
    for (const row of samplesToCheck) {
      if (row >= 0 && row < limit && !rowCache.has(row)) {
        return true
      }
    }
    return false
  }

  /**
   * Writes `rows` into the cache starting at `firstRow`, exactly as the backend served
   * them. The page uses it for the open result's first chunk; `cacheChunk` for the rest.
   */
  function cacheRows(firstRow: number, rows: ViewerRow[]) {
    for (let i = 0; i < rows.length; i++) {
      const row = rows[i]
      rowCache.set(firstRow + i, { text: row.text, continues: row.continues, lineNumber: row.lineNumber })
    }
  }

  /** Drops every cached row, and with it what we knew about where the file ends. */
  function clearCache() {
    rowCache.clear()
    noRowsBeyond = null
  }

  /**
   * Drops cached rows far from the viewport, keeping a generous margin around what's
   * rendered. Runs after a fetch, since that's the only thing that grows the cache.
   *
   * ❌ Not on `fullLoad`: the height map measures EVERY row of such a file, and
   * `getAllRowTexts` hands it the cache. Evicting there would silently disable
   * variable-height word wrap. Those files are under a megabyte, so there's nothing to
   * reclaim anyway.
   */
  function evictDistantRows() {
    if (deps.getBackendType() === 'fullLoad') return
    if (rowCache.size <= CACHE_EVICT_ABOVE) return
    const keep = { from: visibleFrom - CACHE_KEEP_MARGIN, to: renderedTo + CACHE_KEEP_MARGIN }
    const evictable = rowsToEvict(rowCache.keys(), keep)
    for (const row of evictable) rowCache.delete(row)
    log.debug('evicted {count} cached {rowsNoun} outside [{from}, {to})', {
      count: evictable.length,
      rowsNoun: pluralize(evictable.length, 'row'),
      from: keep.from,
      to: keep.to,
    })
  }

  function scheduleFetch(from: number, to: number) {
    if (fetchDebounceTimer) {
      clearTimeout(fetchDebounceTimer)
    }
    fetchDebounceTimer = setTimeout(() => {
      void fetchRows(from, to)
    }, FETCH_DEBOUNCE_MS)
  }

  function updateTotalRows(newTotal: number) {
    const oldEstimate = estimatedTotalRows()
    if (!contentRef || oldEstimate === 0 || newTotal === oldEstimate) {
      deps.setTotalRows(newTotal)
      return
    }
    const oldHeight = Math.min(oldEstimate * effectiveLineHeight, MAX_SCROLL_HEIGHT)
    const scrollFraction = contentRef.scrollTop / oldHeight
    log.debug('totalRows changed: {oldEstimate} -> {newTotal}, preserving scroll fraction {fraction}', {
      oldEstimate,
      newTotal,
      fraction: scrollFraction.toFixed(3),
    })
    deps.setTotalRows(newTotal)
    const newHeight = Math.min(newTotal * effectiveLineHeight, MAX_SCROLL_HEIGHT)
    const newScrollTop = Math.round(scrollFraction * newHeight)
    const ref = contentRef
    requestAnimationFrame(() => {
      ref.scrollTop = newScrollTop
    })
  }

  /** Where a follow-up request picks the walk back up. */
  interface ContinueFrom {
    /** The row the next chunk starts at, for the "is the range covered yet" test. */
    row: number
    /**
     * The exact source byte to resume at, or `null` to resume by row index.
     *
     * ❗ Which one is right depends on who owns the numbering. A backend that can seek by
     * ROW hands a row back as itself, so continuing by row keeps the cache's indexes
     * exactly where the earlier chunks put them. A backend that can't (`byteSeek` with no
     * index) derives a row index from a byte offset through its bytes-per-row sample, so
     * the BYTE is the fact and the row number is the estimate: resume from the byte and
     * cache wherever the answer says.
     */
    byteOffset: number | null
  }

  /**
   * What to ask the backend for to fill the rendered range `[from, to)`, or `null` when that
   * range is empty (past the end of the file, or no rows yet): nothing to draw, so nothing to
   * fetch. The count is always positive, which `viewer_get_lines`'s `usize` insists on.
   *
   * `startAt` continues a range an earlier chunk left unfinished: it starts the request
   * where that chunk ended rather than at the range's own start, so the chain always moves
   * forward.
   */
  function rowRequest({
    from,
    to,
    startAt,
  }: {
    from: number
    to: number
    startAt?: ContinueFrom
  }): { seekType: 'line' | 'byte' | 'fraction'; seekValue: number; fetchFrom: number; fetchCount: number } | null {
    if (to <= from) return null
    // The same pixel budget the render window spends, so a file of tall rows doesn't ask
    // for a hundred of them to sit off-screen.
    const prefetch = bufferRows(effectiveLineHeight)
    const fetchFrom = startAt?.row ?? Math.max(0, from - prefetch)
    if (fetchFrom >= to + prefetch) return null
    const fetchCount = Math.min(FETCH_BATCH, to - fetchFrom + prefetch * 2)
    if (fetchCount <= 0) return null
    if (startAt?.byteOffset != null) {
      return { seekType: 'byte', seekValue: startAt.byteOffset, fetchFrom, fetchCount }
    }
    if (deps.getTotalRows() !== null) return { seekType: 'line', seekValue: fetchFrom, fetchFrom, fetchCount }
    // A 0 estimate would make the fraction division NaN (0/0) or Infinity (>0/0); both
    // serialize to JSON null, which the Rust f64 `targetValue` rejects. With no row count
    // to go on, seek to the start of the file (fraction 0).
    const estimated = estimatedTotalRows()
    return { seekType: 'fraction', seekValue: estimated > 0 ? fetchFrom / estimated : 0, fetchFrom, fetchCount }
  }

  /**
   * Fills the rendered range `[from, to)` from the backend, continuing from `startAt` when
   * an earlier answer stopped short.
   *
   * ❗ A SHORT ANSWER IS NORMAL, not an error: one `viewer_get_lines` carries at most
   * `CHUNK_BUDGET_BYTES`, so a range of long rows comes back in several pieces. The chunk
   * SAYS which case it is (`chunk.end`), and that is what drives the walk.
   */
  async function fetchRows(from: number, to: number, startAt?: ContinueFrom) {
    const sessionId = deps.getSessionId()
    if (!sessionId) return
    const request = rowRequest({ from, to, startAt })
    if (!request) return
    const { seekType, seekValue, fetchCount } = request

    const fetchId = ++currentFetchId

    try {
      log.debug('fetchRows[{fetchId}]: requesting {seekType}={seekValue} count={fetchCount}', {
        fetchId,
        seekType,
        seekValue,
        fetchCount,
      })

      const chunk = await viewerGetLines(sessionId, seekType, seekValue, fetchCount)

      if (fetchId !== currentFetchId) {
        log.debug('fetchRows[{fetchId}]: discarding stale response (current={currentFetchId})', {
          fetchId,
          currentFetchId,
        })
        return
      }

      const continueAt = cacheChunk({ chunk, request, fetchId })
      // `currentFetchId` again: a newer fetch may have started while this answer was in
      // flight (the user scrolled), and its range is the one worth continuing, not ours.
      if (continueAt !== null && continueAt.row < to && fetchId === currentFetchId) {
        await fetchRows(from, to, continueAt)
      }
    } catch (e) {
      reportFetchFailure(e, fetchId)
    }
  }

  /**
   * Caches a chunk's rows and says where a follow-up request should pick up, or `null` when
   * this answer finished the job.
   *
   * ❗ Two facts the chunk STATES, neither of which may be inferred:
   * - `chunk.firstRowNumber` is where the rows go. Caching at the row the request asked
   *   for instead put a `byteSeek` continuation's rows at indexes its own next answer
   *   disagreed with, so the same bytes could be drawn twice at different heights.
   * - `chunk.end` says whether to continue. "Fewer rows than I asked for" is NOT the same
   *   question: `CHUNK_BUDGET_BYTES` makes a short chunk ordinary, and a full-length chunk
   *   can still have stopped on the budget. Reading a short chunk as the end of the file
   *   silently truncates; reading a budget-capped chunk as finished leaves the range
   *   unfilled and the fetch effect re-firing on it every debounce.
   */
  function cacheChunk({
    chunk,
    request,
    fetchId,
  }: {
    chunk: LineChunk
    request: NonNullable<ReturnType<typeof rowRequest>>
    fetchId: number
  }): ContinueFrom | null {
    const received = chunk.rows.length
    const cacheStartRow = chunk.firstRowNumber

    log.debug(
      'fetchRows[{fetchId}]: received {rowCount} {rowsNoun} at row {firstRow} ({end}), asked from {askedFrom}',
      {
        fetchId,
        rowCount: received,
        rowsNoun: pluralize(received, 'row'),
        firstRow: cacheStartRow,
        end: chunk.end,
        askedFrom: request.fetchFrom,
      },
    )

    cacheRows(cacheStartRow, chunk.rows)
    evictDistantRows()

    if (chunk.totalRows.kind === 'exact' && chunk.totalRows.rows !== deps.getTotalRows()) {
      updateTotalRows(chunk.totalRows.rows)
    }

    if (received === 0) {
      // The backend has nothing here: the file ends before this row, whatever the row
      // count claims. Remember it so the effect stops asking for a row that isn't there.
      noRowsBeyond = { row: request.fetchFrom, underTotal: estimatedTotalRows() }
      return null
    }
    if (chunk.end === 'endOfFile') {
      // ❗ The chunk just NAMED the end of the file, so the row past its last one is the
      // first that will never arrive. Recording it here is what stops `needsFetch` from
      // staying true on a row count that overshoots what the backend emits (the
      // `lineIndex` phantom trailing row, or a `byteSeek` estimate), which would refire
      // the fetch every debounce forever.
      noRowsBeyond = { row: cacheStartRow + received, underTotal: estimatedTotalRows() }
      return null
    }
    if (chunk.end !== 'budgetReached') return null
    return {
      row: cacheStartRow + received,
      byteOffset: request.seekType === 'line' ? null : chunk.endByteOffset,
    }
  }

  function reportFetchFailure(e: unknown, fetchId: number) {
    if (fetchId !== currentFetchId) return
    // `viewerGetLines` throws the backend's typed `ViewerError` with its fields copied onto
    // the Error, so the timeout is a VARIANT, never a flag beside a sentence.
    const kind = asViewerError(e)?.kind
    if (kind === 'timedOut') {
      deps.onTimeoutError()
      // The window shows the timeout with Retry, so it's a handled outcome: a warn. An
      // error log counts toward an auto-sent error report.
      log.warn('fetchRows[{fetchId}]: timed out', { fetchId })
    } else {
      log.error("fetchRows[{fetchId}]: didn't come back ({reason})", { fetchId, reason: kind ?? String(e) })
    }
  }

  function handleScroll() {
    if (contentRef) {
      scrollTop = contentRef.scrollTop
      viewportHeight = contentRef.clientHeight
    }
  }

  function scrollByRows(rows: number) {
    if (!contentRef) return

    if (heightMap.ready) {
      // Find current row at the top of viewport, move by `rows` rows, look up new position
      const unscaledY = scrollScale < 1 ? contentRef.scrollTop / scrollScale : contentRef.scrollTop
      const currentRow = heightMap.getRowAtPosition(unscaledY)
      const targetRow = Math.max(0, Math.min(estimatedTotalRows() - 1, currentRow + rows))
      contentRef.scrollTop = Math.max(0, heightMap.getRowTop(targetRow) * scrollScale)
    } else {
      contentRef.scrollTop = Math.max(0, contentRef.scrollTop + rows * scrollLineHeight)
    }
  }

  function scrollByPages(pages: number) {
    if (!contentRef) return

    if (heightMap.ready) {
      // Move by approximately one viewport worth of content
      const pageHeight = contentRef.clientHeight
      const newScrollTop = Math.max(0, contentRef.scrollTop + pages * pageHeight)
      contentRef.scrollTop = newScrollTop
    } else {
      const rowsPerPage = Math.floor(contentRef.clientHeight / effectiveLineHeight) - 1
      contentRef.scrollTop = Math.max(0, contentRef.scrollTop + pages * rowsPerPage * scrollLineHeight)
    }
  }

  function scrollToStart() {
    if (contentRef) {
      contentRef.scrollTop = 0
    }
  }

  function scrollToEnd() {
    if (contentRef) {
      contentRef.scrollTop = contentRef.scrollHeight - contentRef.clientHeight
    }
  }

  /**
   * The rendered height of row `n`, in the same scaled space `getRowTop` speaks. The
   * height map holds the real per-row height once it's ready (a row wrapped by CSS is
   * several text lines tall); before that every row is one text line.
   */
  function rowHeightAt(n: number): number {
    if (heightMap.ready) {
      const measured = (heightMap.getRowTop(n + 1) - heightMap.getRowTop(n)) * scrollScale
      if (measured > 0) return measured
    }
    return scrollLineHeight
  }

  /**
   * Scrolls row `n` just into view, with one row of breathing room, and leaves an
   * already-visible row alone. Drives keyboard selection extension: every extend press
   * calls this, including the one whose target row isn't cached yet, because the scroll
   * is what pulls the row into the render window and triggers its fetch.
   *
   * ❌ The `EOF_ROW` branch is NOT redundant, however much the arithmetic below looks
   * like it would cope. `⌘⇧Down` on a file with no index reports the sentinel as its
   * target, and it only survives `getRowTop` today through integer overflow plus the
   * browser clamping an absurd `scrollTop` — don't lean on that. Worse, the obvious later
   * tidy-up `Math.min(n, totalRows - 1)` yields `NaN` on exactly this branch (the row
   * count is `null` precisely when the sentinel appears), and `scrollTop = NaN` throws the
   * view to the TOP of the file. Branching here keeps that visible to whoever reaches for
   * the clamp.
   */
  function ensureRowVisible(n: number) {
    if (!contentRef) return
    if (n === EOF_ROW) {
      scrollToEnd()
      return
    }
    const next = ensureVisibleOffset({
      rowTop: getRowTop(n),
      rowHeight: rowHeightAt(n),
      scrollTop: contentRef.scrollTop,
      viewportHeight: contentRef.clientHeight,
      margin: scrollLineHeight,
    })
    if (next !== null) contentRef.scrollTop = next
  }

  /**
   * Brings the character at `point` into view horizontally, the way search does for a
   * match: measure the real rect, recentre against the content box, set `scrollLeft`.
   * Without it, repeated Shift+Right on a long unwrapped line walks the focus past the
   * right edge with nothing following it. Word wrap has no horizontal overflow, so it's
   * a no-op there.
   */
  function ensureColumnVisible(point: RowOffset) {
    if (!contentRef || wordWrap) return
    const caret = caretRectFor(contentRef, point)
    if (caret === null) return
    const view = contentRef.getBoundingClientRect()
    const left = recenterOffset({
      markStart: caret.left,
      markEnd: caret.right,
      viewStart: view.left,
      viewEnd: view.right,
      currentScroll: contentRef.scrollLeft,
    })
    if (left !== null && Math.abs(left - contentRef.scrollLeft) > 2) contentRef.scrollLeft = left
  }

  /**
   * One column's advance width, measured once off a rendered row and dropped when the
   * text scale settles (the only thing that changes it). `null` until a row with text
   * exists, which is also when there's nothing to scroll.
   */
  let columnWidth: number | null = null
  function scrollByColumns(columns: number) {
    if (!contentRef || wordWrap) return
    columnWidth ??= measureColumnWidth(contentRef)
    if (columnWidth === null) return
    contentRef.scrollLeft = Math.max(0, contentRef.scrollLeft + columns * columnWidth)
  }

  function runFetchEffect() {
    const from = visibleFrom
    const to = visibleTo
    const sessionId = deps.getSessionId()
    if (sessionId && needsFetch(from, to)) {
      scheduleFetch(from, to)
    }
  }

  /**
   * Force a fetch of the current visible range, bypassing the cache check. Used
   * when the cache was deliberately invalidated (encoding switch) so the next
   * render shows freshly-decoded rows without waiting for the user to scroll.
   */
  function fetchVisibleNow() {
    const sessionId = deps.getSessionId()
    if (!sessionId) return
    // Whatever we knew about where the file ends belongs to the old decoding of it.
    noRowsBeyond = null
    void fetchRows(visibleFrom, visibleTo)
  }

  function runContentWidthEffect() {
    if (wordWrap) return
    dependOn(visibleRows)
    const rafId = requestAnimationFrame(() => {
      if (linesContainerRef) {
        const w = linesContainerRef.scrollWidth
        if (w > contentWidth) {
          contentWidth = w
        }
      }
    })
    return () => {
      cancelAnimationFrame(rafId)
    }
  }

  function runWrappedLineHeightEffect() {
    if (!wordWrap) return

    if (heightMap.ready) return // Height map replaces DOM-based averaging
    dependOn(scrollTop)
    const rafId = requestAnimationFrame(() => {
      if (!linesContainerRef) return
      const rowCount = linesContainerRef.children.length
      if (rowCount === 0) return
      const renderedHeight = linesContainerRef.getBoundingClientRect().height
      if (renderedHeight > 0) {
        const measured = renderedHeight / rowCount
        if (Math.abs(measured - avgWrappedLineHeight) > 1) {
          avgWrappedLineHeight = measured
        }
      }
    })
    return () => {
      cancelAnimationFrame(rafId)
    }
  }

  let prevScrollLineHeight = getLineHeight()
  function runScrollCompensationEffect() {
    const newSLH = scrollLineHeight
    if (!contentRef || prevScrollLineHeight === newSLH) {
      prevScrollLineHeight = newSLH
      return
    }

    if (heightMap.ready) {
      // With height map: find the row at current viewport top, look up its new position
      const anchorRow = getAnchorRow()
      contentRef.scrollTop = heightMap.getRowTop(anchorRow) * scrollScale
    } else {
      // Without height map: scale proportionally (existing behavior)
      const ratio = newSLH / prevScrollLineHeight
      contentRef.scrollTop = Math.round(contentRef.scrollTop * ratio)
    }
    prevScrollLineHeight = newSLH
  }

  /**
   * Watches wordWrap + getAllRowTexts + getTextWidth and triggers height map preparation
   * when all conditions are met (word wrap on, fullLoad rows available, width known).
   * Does NOT re-prepare if the height map is already ready; width changes are handled
   * by runHeightMapReflowEffect via reflow() instead.
   */
  function runHeightMapInitEffect() {
    // Read reactive deps to establish tracking

    const ww = wordWrap
    const rows = deps.getAllRowTexts()
    const textWidth = deps.getTextWidth()

    if (!ww) {
      heightMap.cancel()
      return
    }

    if (heightMap.ready) return // Width changes handled by reflow, not re-preparation

    if (rows !== null && rows.length > 0 && textWidth > 0) {
      heightMap.prepareRows(rows, textWidth)
    }
  }

  /**
   * Re-measures all row heights at the new width and preserves the scroll
   * fraction across the change. Uses the container's real `scrollHeight` (the
   * rendered spacer) for the fraction rather than a derived value, and applies
   * the new position after the spacer DOM has settled.
   */
  function doReflow(newWidth: number) {
    if (!heightMap.ready || !contentRef) return
    const oldScrollHeight = contentRef.scrollHeight
    const fraction = oldScrollHeight > 0 ? contentRef.scrollTop / oldScrollHeight : 0
    heightMap.reflow(newWidth)
    const ref = contentRef
    requestAnimationFrame(() => {
      ref.scrollTop = fraction * ref.scrollHeight
    })
  }

  /**
   * Watches textWidth and re-measures the height map when it changes. Debounced
   * to the resize-settle: a full DOM re-measure is ~70 ms, too slow to run on
   * every ResizeObserver frame during a live window drag.
   */
  let prevTextWidth = 0
  let reflowDebounceTimer: ReturnType<typeof setTimeout> | undefined
  function runHeightMapReflowEffect() {
    const textWidth = deps.getTextWidth()

    // Only react to actual textWidth changes. The prevTextWidth guard prevents this
    // effect from re-running due to other reactive dependencies (heightMap.ready, version).
    if (textWidth <= 0 || textWidth === prevTextWidth) return
    if (!heightMap.ready) {
      prevTextWidth = textWidth
      return
    }
    prevTextWidth = textWidth

    if (reflowDebounceTimer) clearTimeout(reflowDebounceTimer)
    reflowDebounceTimer = setTimeout(() => {
      doReflow(textWidth)
    }, REFLOW_DEBOUNCE_MS)
  }

  // After the user settles on a new text scale, the line height the height
  // map baked in is stale. Re-layout (without changing width) so wrapped-row
  // heights match the new font. This runs inside the same debounced "settled"
  // event the file-list column-width path uses, so we don't thrash mid-drag.
  const unsubscribeScaleChange = onDebouncedScaleChange(() => {
    heightMap.recomputeForLineHeightChange()
    // A new font size means a new column advance; re-measure it on the next press.
    columnWidth = null
  })

  function destroy() {
    if (fetchDebounceTimer) clearTimeout(fetchDebounceTimer)
    if (reflowDebounceTimer) clearTimeout(reflowDebounceTimer)
    heightMap.cancel()
    unsubscribeScaleChange()
  }

  return {
    rowCache,
    get scrollTop() {
      return scrollTop
    },
    get viewportHeight() {
      return viewportHeight
    },
    get contentRef() {
      return contentRef
    },
    set contentRef(v: HTMLDivElement | undefined) {
      contentRef = v
    },
    get containerRef() {
      return containerRef
    },
    set containerRef(v: HTMLElement | undefined) {
      containerRef = v
    },
    get linesContainerRef() {
      return linesContainerRef
    },
    set linesContainerRef(v: HTMLDivElement | undefined) {
      linesContainerRef = v
    },
    get contentWidth() {
      return contentWidth
    },
    set contentWidth(v: number) {
      contentWidth = v
    },
    get wordWrap() {
      return wordWrap
    },
    set wordWrap(v: boolean) {
      wordWrap = v
    },
    get effectiveLineHeight() {
      return effectiveLineHeight
    },
    get scrollLineHeight() {
      return scrollLineHeight
    },
    get visibleFrom() {
      return visibleFrom
    },
    get visibleRows() {
      return visibleRows
    },
    get gutterWidth() {
      return gutterWidth
    },
    get spacerHeight() {
      return spacerHeight
    },
    get rowsOffset() {
      return rowsOffset
    },
    get heightMapReady() {
      return heightMap.ready
    },
    cacheRows,
    clearCache,
    estimatedTotalRows,
    renderedRowText,
    getRowTop,
    handleScroll,
    scrollByRows,
    scrollByPages,
    scrollToStart,
    scrollToEnd,
    scrollByColumns,
    ensureRowVisible,
    ensureColumnVisible,
    runFetchEffect,
    fetchVisibleNow,
    runContentWidthEffect,
    runWrappedLineHeightEffect,
    runScrollCompensationEffect,
    runHeightMapInitEffect,
    runHeightMapReflowEffect,
    destroy,
  }
}
