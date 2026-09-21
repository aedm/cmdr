import { describe, it, expect } from 'vitest'
import { createRowHeightMap, getLineHeight, type RowHeightMeasurer } from './viewer-line-heights.svelte'

// Run scheduled work synchronously so tests don't juggle timers. The production
// default defers the (blocking) measure pass to an idle callback.
const runNow = (cb: () => void) => {
  cb()
}

/** A measurer that returns a preset height per line, ignoring width. */
function presetMeasure(heights: number[]): RowHeightMeasurer {
  return (lines) => Float64Array.from(lines.map((_, i) => heights[i] ?? 0))
}

/** A width-sensitive measurer: every line is exactly `maxWidth` px tall. Lets a
 *  test prove reflow re-measures at the new width. */
const widthAsHeightMeasure: RowHeightMeasurer = (lines, maxWidth) => Float64Array.from(lines.map(() => maxWidth))

describe('createRowHeightMap', () => {
  const minH = getLineHeight() // 18 at scale 1

  it('is not ready before prepareRows and reports zero positions', () => {
    const map = createRowHeightMap({ measure: presetMeasure([50]), schedule: runNow })
    expect(map.ready).toBe(false)
    expect(map.getRowTop(3)).toBe(0)
    expect(map.getTotalHeight()).toBe(0)
    expect(map.getRowAtPosition(999)).toBe(0)
  })

  it('builds a prefix sum of variable heights and answers positions in O(1)/O(log n)', () => {
    // Real-world shape: some lines wrap to many rows, some to one.
    const map = createRowHeightMap({ measure: presetMeasure([18, 180, 18, 90]), schedule: runNow })
    map.prepareRows(['a', 'b', 'c', 'd'], 800)
    expect(map.ready).toBe(true)

    // cumulative tops: 0, 18, 198, 216 ; total 306
    expect(map.getRowTop(0)).toBe(0)
    expect(map.getRowTop(1)).toBe(18)
    expect(map.getRowTop(2)).toBe(198)
    expect(map.getRowTop(3)).toBe(216)
    expect(map.getTotalHeight()).toBe(306)

    // getRowAtPosition: largest line whose top <= y
    expect(map.getRowAtPosition(0)).toBe(0)
    expect(map.getRowAtPosition(17)).toBe(0)
    expect(map.getRowAtPosition(18)).toBe(1)
    expect(map.getRowAtPosition(197)).toBe(1)
    expect(map.getRowAtPosition(198)).toBe(2)
    expect(map.getRowAtPosition(215)).toBe(2)
    expect(map.getRowAtPosition(216)).toBe(3)
    expect(map.getRowAtPosition(100_000)).toBe(3) // clamped to last line
  })

  it('clamps each line to at least the minimum line height (empty rows keep the gutter open)', () => {
    // Pretext/DOM report 0 for an empty line, but the row still renders one line tall.
    const map = createRowHeightMap({ measure: presetMeasure([0, 5, 40]), schedule: runNow })
    map.prepareRows(['', ' ', 'wraps'], 800)
    expect(map.getRowTop(1)).toBe(minH) // 0 -> minH
    expect(map.getRowTop(2)).toBe(minH * 2) // 5 -> minH
    expect(map.getTotalHeight()).toBe(minH * 2 + 40) // 40 stays
  })

  it('reflow re-measures at the new width and rebuilds the prefix sum', () => {
    const map = createRowHeightMap({ measure: widthAsHeightMeasure, schedule: runNow })
    map.prepareRows(['a', 'b'], 100)
    expect(map.getTotalHeight()).toBe(200) // 2 * 100

    map.reflow(250)
    expect(map.getTotalHeight()).toBe(500) // 2 * 250

    // No-op when width is unchanged.
    map.reflow(250)
    expect(map.getTotalHeight()).toBe(500)
  })

  it('recomputeForLineHeightChange re-measures at the current width', () => {
    let scale = 100
    const measure: RowHeightMeasurer = (lines) => Float64Array.from(lines.map(() => scale))
    const map = createRowHeightMap({ measure, schedule: runNow })
    map.prepareRows(['a', 'b'], 800)
    expect(map.getTotalHeight()).toBe(200)

    scale = 130 // simulate a font-scale settle changing row heights
    map.recomputeForLineHeightChange()
    expect(map.getTotalHeight()).toBe(260)
  })

  it('cancel discards the map and drops back to not-ready', () => {
    const map = createRowHeightMap({ measure: presetMeasure([50, 50]), schedule: runNow })
    map.prepareRows(['a', 'b'], 800)
    expect(map.ready).toBe(true)
    map.cancel()
    expect(map.ready).toBe(false)
    expect(map.getTotalHeight()).toBe(0)
  })

  it('a stale (superseded) preparation does not flip ready', () => {
    // Capture each scheduled callback so we can fire an old one after a newer prepare.
    const scheduled: Array<() => void> = []
    const deferred = (cb: () => void) => {
      scheduled.push(cb)
    }
    const map = createRowHeightMap({ measure: presetMeasure([50]), schedule: deferred })
    map.prepareRows(['a'], 800)
    // A newer preparation supersedes the first before it runs.
    map.prepareRows(['a'], 800)
    scheduled[0]() // fire the old, now-stale callback
    expect(map.ready).toBe(false)
  })

  it('skips the map for empty input and for files past the line cap', () => {
    const map = createRowHeightMap({ measure: presetMeasure([50]), schedule: runNow })
    map.prepareRows([], 800)
    expect(map.ready).toBe(false)

    const many = Array.from({ length: 50_001 }, () => 'x')
    map.prepareRows(many, 800)
    expect(map.ready).toBe(false)
  })
})
