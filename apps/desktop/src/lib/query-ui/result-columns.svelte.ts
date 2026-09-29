/**
 * The results table's grid template, live: measures the rows, watches the container's
 * width, and hands `QueryResults` ONE `grid-template-columns` string for the header and
 * every row. The math is `result-column-widths.ts`; this factory owns the DOM reads.
 *
 * Why it can't oscillate: every input (the entry DATA, the cells' computed fonts, the
 * row's padding and `column-gap`, the container's width) comes from CSS or the dialog,
 * never from the tracks it writes. It measures `entry.name`, never the DOM text
 * `useShortenMiddle` or `PathPills` wrote into a cell. DETAILS.md § Column widths.
 *
 * Call it during component init: it registers `$effect`s.
 */

import { onDestroy } from 'svelte'
import type { SearchResultEntry } from '$lib/tauri-commands'
import { sizeDisplayParts } from '$lib/ui/size-display'
import { formattedDate } from '$lib/settings/reactive-settings.svelte'
import { tString } from '$lib/intl/messages.svelte'
import { createPretextMeasure } from '$lib/utils/shorten-middle'
import { splitPath } from './path-pills-layout'
import {
  measureColumnDemands,
  NAME_COL_MIN_PX,
  PATH_COL_MIN_PX,
  splitNameAndPath,
  trackToCss,
  type ColumnDemands,
  type ResultRowTexts,
} from './result-column-widths'

/** The icon column's fixed track, in CSS pixels. */
const ICON_TRACK_PX = 24

export interface ResultColumnsInputs {
  /** The scrolling list every row fills; `undefined` until mounted. */
  container: HTMLElement | undefined
  results: SearchResultEntry[]
  showPathColumn: boolean
  /** Whether rows are rendered (versus a spinner, empty state, or count). */
  showingRows: boolean
}

export interface ResultColumns {
  /** The full template for the header and every row. */
  readonly gridTemplate: string
  /** Off for the first measured layout, so opening the dialog doesn't animate the columns in. */
  readonly animateTracks: boolean
}

function rowTexts(entry: SearchResultEntry): ResultRowTexts {
  return {
    name: entry.name,
    pathLabels: splitPath(entry.parentPath).map((s) => s.label),
    size:
      entry.size == null
        ? ''
        : sizeDisplayParts(entry.size)
            .map((p) => p.value)
            .join(''),
    modified: formattedDate(entry.modifiedAt).text,
  }
}

function readFont(node: HTMLElement): string {
  const style = getComputedStyle(node)
  return style.font || `${style.fontSize} ${style.fontFamily}`
}

export function createResultColumns(inputs: () => ResultColumnsInputs): ResultColumns {
  /** Width of the container (scrollbar excluded), which every row fills. */
  let containerWidth = $state(0)
  /** A row's horizontal padding plus its four column gaps, read off a real row. */
  let rowChrome = $state(0)
  /** Pixel-accurate measurer at the Name cell's font; null until pretext resolves. */
  let measureName = $state<((text: string) => number) | null>(null)
  /** Same, at the regular font the Path, Size, and Modified cells share. */
  let measureText = $state<((text: string) => number) | null>(null)
  let animateTracks = $state(false)
  /** The fonts the current measurers were built for; a change (text size) rebuilds them. */
  let measuredFonts = ''
  let firstLayoutApplied = false
  let widthObserver: ResizeObserver | undefined

  /** What each column needs to show every row uncut. */
  const demands = $derived.by<ColumnDemands | null>(() => {
    const { results, showPathColumn } = inputs()
    if (!showPathColumn || !measureName || !measureText || results.length === 0) return null
    return measureColumnDemands({
      rows: results.map(rowTexts),
      headers: {
        name: tString('queryUi.results.col.name'),
        path: tString('queryUi.results.col.path'),
        size: tString('queryUi.results.col.size'),
        modified: tString('queryUi.results.col.modified'),
      },
      measureName,
      measureText,
    })
  })

  const gridTemplate = $derived.by(() => {
    const icon = `${String(ICON_TRACK_PX)}px`
    const nameFlex = `minmax(${String(NAME_COL_MIN_PX)}px, 1fr)`
    // No Path column (Selection): Name is the flex track, so it absorbs the width Path
    // would have taken instead of leaving a gap before Size.
    if (!inputs().showPathColumn) return `${icon} ${nameFlex} 10ch 16ch`
    // Before measurement (or without canvas): an even Name / Path split.
    if (!demands) return `${icon} ${nameFlex} minmax(${String(PATH_COL_MIN_PX)}px, 1fr) 10ch 16ch`
    const available = containerWidth - rowChrome - ICON_TRACK_PX - demands.size - demands.modified
    const { name, path } = splitNameAndPath({ available, name: demands.name, path: demands.path })
    return `${icon} ${trackToCss(name)} ${trackToCss(path)} ${String(demands.size)}px ${String(demands.modified)}px`
  })

  /**
   * Builds (or rebuilds) both measurers from real rendered cells' fonts. Keying on the
   * computed font strings means a text-size change re-measures on its own.
   */
  async function ensureMeasurers(nameEl: HTMLElement, textEl: HTMLElement): Promise<void> {
    const nameFont = readFont(nameEl)
    const textFont = readFont(textEl)
    const fonts = `${nameFont}|${textFont}`
    if (fonts === measuredFonts) return
    // Remember the attempt (success or failure) so we don't retry per render, and drop
    // the old measurers: they were built for fonts we're no longer rendering.
    measuredFonts = fonts
    measureName = null
    measureText = null
    try {
      const pretext = await import('@chenglou/pretext')
      const nameCandidate = createPretextMeasure(nameFont, pretext)
      const textCandidate = createPretextMeasure(textFont, pretext)
      // Probe before adopting: pretext needs Canvas 2D and only fails on first use.
      nameCandidate('0')
      textCandidate('0')
      measureName = nameCandidate
      measureText = textCandidate
    } catch {
      // No canvas, or the chunk failed to load: stay on the even-split fallback rather
      // than throwing on every render.
    }
  }

  // Keeps `containerWidth` live. The container's width comes from the dialog.
  $effect(() => {
    const el = inputs().container
    if (!el) return
    containerWidth = el.clientWidth
    const observer = new ResizeObserver(() => {
      containerWidth = el.clientWidth
    })
    observer.observe(el)
    widthObserver = observer
    return () => {
      observer.disconnect()
      widthObserver = undefined
    }
  })

  // Reads the fonts and the row chrome off the first rendered row.
  $effect(() => {
    const { container, results, showPathColumn, showingRows } = inputs()
    if (!container || !showPathColumn || !showingRows || results.length === 0) return
    const rowEl = container.querySelector<HTMLElement>('.result-row')
    const nameEl = rowEl?.querySelector<HTMLElement>('.result-name')
    const sizeEl = rowEl?.querySelector<HTMLElement>('.result-size')
    if (!rowEl || !nameEl || !sizeEl) return
    const style = getComputedStyle(rowEl)
    const gap = parseFloat(style.columnGap) || 0
    rowChrome = (parseFloat(style.paddingLeft) || 0) + (parseFloat(style.paddingRight) || 0) + 4 * gap
    void ensureMeasurers(nameEl, sizeEl)
  })

  // Turns the track transition on after the first measured layout has painted.
  $effect(() => {
    if (!demands || firstLayoutApplied) return
    firstLayoutApplied = true
    requestAnimationFrame(() => {
      animateTracks = true
    })
  })

  onDestroy(() => {
    widthObserver?.disconnect()
  })

  return {
    get gridTemplate() {
      return gridTemplate
    },
    get animateTracks() {
      return animateTracks
    },
  }
}
