import { describe, it, expect, afterEach } from 'vitest'
import { contextMenuAnchor, cursorRowAnchor } from './context-menu-anchor'

describe('contextMenuAnchor', () => {
  it('anchors under the row, inset from its left edge (matching the Enter popup)', () => {
    expect(contextMenuAnchor({ left: 100, top: 40, bottom: 60 }, null)).toEqual({ x: 116, y: 60 })
  })

  it('prefers the row even when the surface is also known', () => {
    const surface = { left: 0, top: 0, bottom: 800 }
    expect(contextMenuAnchor({ left: 100, top: 40, bottom: 60 }, surface)).toEqual({ x: 116, y: 60 })
  })

  it('falls back to the surface left edge, vertically centred, when the row is not rendered', () => {
    // The user scrolled the cursor row out of view. ❌ Never scroll it back: pick a
    // sane spot in the pane instead.
    expect(contextMenuAnchor(null, { left: 540, top: 100, bottom: 700 })).toEqual({ x: 540, y: 400 })
  })

  it('has no answer when neither the row nor the surface is measurable', () => {
    expect(contextMenuAnchor(null, null)).toBeNull()
  })

  it('keeps a fractional rect fractional (no rounding before the IPC)', () => {
    expect(contextMenuAnchor({ left: 100.5, top: 40, bottom: 60.25 }, null)).toEqual({ x: 116.5, y: 60.25 })
  })
})

/**
 * `cursorRowAnchor` against a real DOM. `getBoundingClientRect` answers all zeroes in
 * jsdom, so these pin WHICH element is measured, which is the part that can be wrong.
 */
describe('cursorRowAnchor', () => {
  afterEach(() => {
    document.body.innerHTML = ''
  })

  function buildPane(rowIndices: number[]): HTMLElement {
    const pane = document.createElement('div')
    const surface = document.createElement('div')
    surface.setAttribute('data-file-list-surface', '')
    for (const i of rowIndices) {
      const row = document.createElement('div')
      row.id = `file-${String(i)}`
      surface.appendChild(row)
    }
    pane.appendChild(surface)
    document.body.appendChild(pane)
    return pane
  }

  it('measures the row in THIS pane, never the other pane carrying the same row id', () => {
    // Both panes render `file-<index>` ids, so the document holds two `file-3`s.
    // A `document.getElementById` lookup would take the left pane's every time.
    const left = buildPane([0, 1, 2, 3])
    const right = buildPane([0, 1, 2, 3])
    const leftRow = left.querySelector('#file-3')
    const rightRow = right.querySelector('#file-3')
    expect(leftRow).not.toBe(rightRow)

    expect(cursorRowAnchor(right, 3, () => ({ left: 900, top: 40, bottom: 60 }))).toEqual({ x: 916, y: 60 })
    expect(cursorRowAnchor(left, 3, () => ({ left: 100, top: 40, bottom: 60 }))).toEqual({ x: 116, y: 60 })
  })

  it('falls back to the pane scroll surface when the cursor row is off-screen', () => {
    const pane = buildPane([10, 11, 12])
    const measured = cursorRowAnchor(pane, 3, (el) => {
      expect(el.hasAttribute('data-file-list-surface')).toBe(true)
      return { left: 8, top: 100, bottom: 700 }
    })
    expect(measured).toEqual({ x: 8, y: 400 })
  })

  it('has no answer without a pane element', () => {
    expect(cursorRowAnchor(null, 3, () => ({ left: 0, top: 0, bottom: 0 }))).toBeNull()
  })

  it('has no answer when the pane has neither the row nor a scroll surface', () => {
    const pane = document.createElement('div')
    document.body.appendChild(pane)
    expect(cursorRowAnchor(pane, 3, () => ({ left: 0, top: 0, bottom: 0 }))).toBeNull()
  })
})
