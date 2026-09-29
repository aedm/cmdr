/**
 * Column math for the Search results table (Name | Path | Size | Modified).
 *
 * Size and Modified shrink-wrap to their widest cell. Name and Path share the rest by
 * max-min fairness: each has a demand (its widest row, uncut), they split the width
 * 50-50, and a column that needs less than its half takes only what it needs and hands
 * the remainder to the other. So widening the dialog always widens whichever column is
 * still cut off, and a short column never hoards space.
 *
 * Every row counts, not only those on screen: Search caps at 30 rows, so measuring all
 * of them is cheap, and the columns stay put while scrolling.
 *
 * Both functions are pure so the algorithm is unit-testable with mocked widths; the
 * component owns the DOM reads (container width, row padding, the fonts). See
 * `QueryResults.svelte` and DETAILS.md § Column widths.
 */

import { PILL_CHROME_PX, PILL_SEPARATOR_GAP_PX, totalWidth } from './path-pills-layout'

/**
 * Floor on the Name track, in CSS pixels. A list of very short names shouldn't squeeze
 * the column down to a couple of glyphs, and a narrow dialog still leaves Name readable.
 */
export const NAME_COL_MIN_PX = 80

/** Floor on the Path track, in CSS pixels: room for at least `…` plus a short pill. */
export const PATH_COL_MIN_PX = 120

/**
 * Per-column measurement safety pad, in CSS pixels. Pretext measures via canvas while the
 * row lays out via DOM, and on WKWebView the two can disagree by a fraction of a pixel on
 * the same font. Without the pad a name measured to exactly the track width truncates to
 * `na…me` for no visible reason. Same constant and rationale as
 * `file-explorer/views/measure-column-widths.ts`.
 */
export const MEASUREMENT_PAD = 2

/** One row's cell texts, exactly as the row renders them. */
export interface ResultRowTexts {
  name: string
  /** The Path cell's pill labels, uncollapsed (`splitPath(parentPath)`). */
  pathLabels: string[]
  /** The Size cell's text; empty when the entry has no size. */
  size: string
  /** The Modified cell's text; empty when the entry has no date. */
  modified: string
}

export interface ColumnLabels {
  name: string
  path: string
  size: string
  modified: string
}

/** The width, in CSS pixels, each column needs to show every row uncut. */
export interface ColumnDemands {
  name: number
  path: number
  size: number
  modified: number
}

export interface MeasureColumnDemandsArgs {
  rows: ResultRowTexts[]
  headers: ColumnLabels
  /** Pixel-accurate measurer at the Name cell's (bolder) font. */
  measureName: (text: string) => number
  /** Pixel-accurate measurer at the other cells' regular font. */
  measureText: (text: string) => number
}

/**
 * The font's widest decimal digit. Dates render with `font-variant-numeric: tabular-nums`,
 * which canvas can't measure (the canvas `font` shorthand has no slot for it), so we
 * measure every digit as the widest one: a slight over-estimate, never a clip. Sizes get
 * the same treatment, which only over-reserves by a pixel or two.
 */
function widestDigit(measure: (text: string) => number): string {
  let best = '0'
  let bestWidth = -1
  for (const digit of '0123456789') {
    const w = measure(digit)
    if (w > bestWidth) {
      bestWidth = w
      best = digit
    }
  }
  return best
}

export function measureColumnDemands({ rows, headers, measureName, measureText }: MeasureColumnDemandsArgs): ColumnDemands {
  const digit = widestDigit(measureText)
  const measureTabular = (text: string): number => measureText(text.replace(/[0-9]/g, digit))
  const separator = measureText('/') + PILL_SEPARATOR_GAP_PX

  // Headers are measured at their own place's font where it matters: the Name header is
  // regular weight, so measuring it with the bolder `measureName` over-reserves slightly,
  // the safe direction. The Path header sits inset by half a pill's chrome to line up
  // with the first pill's text.
  let name = measureName(headers.name)
  let path = measureText(headers.path) + PILL_CHROME_PX / 2
  let size = measureText(headers.size)
  let modified = measureText(headers.modified)
  for (const row of rows) {
    name = Math.max(name, measureName(row.name))
    path = Math.max(path, totalWidth(row.pathLabels, measureText, PILL_CHROME_PX, separator))
    size = Math.max(size, measureTabular(row.size))
    modified = Math.max(modified, measureTabular(row.modified))
  }
  return {
    name: Math.ceil(Math.max(NAME_COL_MIN_PX, name + MEASUREMENT_PAD)),
    path: Math.ceil(Math.max(PATH_COL_MIN_PX, path + MEASUREMENT_PAD)),
    size: Math.ceil(size + MEASUREMENT_PAD),
    modified: Math.ceil(modified + MEASUREMENT_PAD),
  }
}

/**
 * A grid track: a fixed pixel width, or a flexible `minmax(<min>px, 1fr)` share of
 * whatever's left. Leaving the "whatever's left" to CSS (instead of computing it in
 * pixels) keeps the right edge flush with the container on every frame of a resize,
 * even before our width reading catches up.
 */
export type Track = { kind: 'fixed'; px: number } | { kind: 'flex'; minPx: number }

export function trackToCss(track: Track): string {
  return track.kind === 'fixed' ? `${String(track.px)}px` : `minmax(${String(track.minPx)}px, 1fr)`
}

export interface SplitNameAndPathArgs {
  /** Width left for Name + Path after the icon, Size, Modified, gaps, and row padding. */
  available: number
  /** Name's demand, from `measureColumnDemands`. */
  name: number
  /** Path's demand, from `measureColumnDemands`. */
  path: number
}

/**
 * Splits `available` between Name and Path. A column that fits in its half (or both,
 * when they fit together) gets its exact demand; the other takes the rest as the flexible
 * track. When both need more than half, both flex, which CSS resolves to an even split.
 * When both fit, the spare width goes to Path, so it sits as blank space before the
 * right-aligned Size column.
 */
export function splitNameAndPath({ available, name, path }: SplitNameAndPathArgs): { name: Track; path: Track } {
  const nameFlex: Track = { kind: 'flex', minPx: NAME_COL_MIN_PX }
  const pathFlex: Track = { kind: 'flex', minPx: PATH_COL_MIN_PX }
  if (available <= 0) return { name: nameFlex, path: pathFlex }
  const half = available / 2
  if (name + path <= available || name <= half) return { name: { kind: 'fixed', px: name }, path: pathFlex }
  if (path <= half) return { name: nameFlex, path: { kind: 'fixed', px: path } }
  return { name: nameFlex, path: pathFlex }
}
