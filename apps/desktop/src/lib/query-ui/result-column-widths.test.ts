/**
 * Unit tests for the results table's column math (`result-column-widths.ts`).
 *
 * Widths are mocked (10 px per character, digits included, so tabular-digit modeling is a
 * no-op here). The real measurement is pretext's job; mocking it keeps the algorithm
 * testable without a canvas.
 */

import { describe, expect, it } from 'vitest'
import {
  measureColumnDemands,
  splitNameAndPath,
  MEASUREMENT_PAD,
  NAME_COL_MIN_PX,
  PATH_COL_MIN_PX,
  type ResultRowTexts,
} from './result-column-widths'
import { PILL_CHROME_PX, PILL_SEPARATOR_GAP_PX } from './path-pills-layout'

const CHAR_PX = 10
const measure = (text: string): number => text.length * CHAR_PX
const headers = { name: 'Name', path: 'Path', size: 'Size', modified: 'Modified' }

function row(name: string, pathLabels: string[], size = '1 kB', modified = '2026-09-29 16:25'): ResultRowTexts {
  return { name, pathLabels, size, modified }
}

function demands(rows: ResultRowTexts[]) {
  return measureColumnDemands({ rows, headers, measureName: measure, measureText: measure })
}

describe('measureColumnDemands', () => {
  it('sizes Name to the widest name across every row, with no ceiling', () => {
    const long = 'a-really-very-extremely-long-file-name.tar.gz'
    const d = demands([row('short.txt', ['~']), row(long, ['~'])])
    expect(d.name).toBe(measure(long) + MEASUREMENT_PAD)
  })

  it('floors Name so a list of tiny names keeps a usable column', () => {
    expect(demands([row('a', ['~'])]).name).toBe(NAME_COL_MIN_PX)
  })

  it('sizes Path to the widest uncollapsed pill strip', () => {
    const labels = ['~', 'Downloads', 'papers']
    const sep = measure('/') + PILL_SEPARATOR_GAP_PX
    const strip = labels.reduce((w, l) => w + measure(l) + PILL_CHROME_PX, 0) + sep * (labels.length - 1)
    const d = demands([row('x.pdf', ['~', 'Downloads']), row('y.pdf', labels)])
    expect(d.path).toBe(Math.max(PATH_COL_MIN_PX, Math.ceil(strip + MEASUREMENT_PAD)))
  })

  it('sizes Size and Modified to their widest cell or header, whichever is wider', () => {
    const d = demands([row('x', ['~'], '134,67 kB', '2026-09-18 14:48'), row('y', ['~'], '1 B', '')])
    expect(d.size).toBe(measure('134,67 kB') + MEASUREMENT_PAD)
    expect(d.modified).toBe(measure('2026-09-18 14:48') + MEASUREMENT_PAD)

    const tiny = demands([row('x', ['~'], '1 B', '1')])
    expect(tiny.size).toBe(measure('Size') + MEASUREMENT_PAD)
    expect(tiny.modified).toBe(measure('Modified') + MEASUREMENT_PAD)
  })

  it('models tabular digits by measuring every digit as the widest one', () => {
    // A font where "1" is narrow: the rendered tabular "11" is as wide as "88".
    const narrowOnes = (text: string): number => {
      const ones = text.length - text.replaceAll('1', '').length
      return ones * 4 + (text.length - ones) * 10
    }
    const d = measureColumnDemands({
      rows: [row('x', ['~'], '111 kB', '11')],
      headers: { ...headers, size: 'S', modified: 'M' },
      measureName: narrowOnes,
      measureText: narrowOnes,
    })
    expect(d.size).toBe(narrowOnes('888 kB') + MEASUREMENT_PAD)
  })
})

describe('splitNameAndPath', () => {
  it('shows both in full when they fit, handing the spare width to Path', () => {
    expect(splitNameAndPath({ available: 1000, name: 300, path: 200 })).toEqual({
      name: { kind: 'fixed', px: 300 },
      path: { kind: 'flex', minPx: PATH_COL_MIN_PX },
    })
  })

  it('gives a short Name exactly what it needs and the rest to a long Path', () => {
    expect(splitNameAndPath({ available: 600, name: 200, path: 900 })).toEqual({
      name: { kind: 'fixed', px: 200 },
      path: { kind: 'flex', minPx: PATH_COL_MIN_PX },
    })
  })

  it('gives a short Path exactly what it needs and the rest to a long Name', () => {
    // David's screenshot: long filenames, short "~ / Downloads" paths.
    expect(splitNameAndPath({ available: 600, name: 900, path: 200 })).toEqual({
      name: { kind: 'flex', minPx: NAME_COL_MIN_PX },
      path: { kind: 'fixed', px: 200 },
    })
  })

  it('splits 50-50 when both need more than half', () => {
    expect(splitNameAndPath({ available: 600, name: 900, path: 400 })).toEqual({
      name: { kind: 'flex', minPx: NAME_COL_MIN_PX },
      path: { kind: 'flex', minPx: PATH_COL_MIN_PX },
    })
  })

  it('splits 50-50 before the container has a width', () => {
    expect(splitNameAndPath({ available: 0, name: 100, path: 100 })).toEqual({
      name: { kind: 'flex', minPx: NAME_COL_MIN_PX },
      path: { kind: 'flex', minPx: PATH_COL_MIN_PX },
    })
  })
})
