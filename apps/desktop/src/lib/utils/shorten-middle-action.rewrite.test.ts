import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// A fixed 7 px per character stands in for pretext, which needs a canvas happy-dom lacks.
vi.mock('./shorten-middle', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./shorten-middle')>()
  return { ...actual, createPretextMeasure: () => (text: string) => text.length * 7 }
})
vi.mock('@chenglou/pretext', () => ({}))

import { useShortenMiddle } from './shorten-middle-action'

/**
 * A resize storm (the Full list's 300 ms column transition resizes every name cell each frame)
 * must not rewrite a cell whose text comes out the same: those writes were thousands of DOM
 * mutations a minute on an idle pane.
 */
describe('useShortenMiddle DOM writes', () => {
  let resizeCallbacks: (() => void)[] = []

  beforeEach(() => {
    document.body.innerHTML = ''
    resizeCallbacks = []
    vi.stubGlobal(
      'ResizeObserver',
      class {
        constructor(callback: () => void) {
          resizeCallbacks.push(callback)
        }
        observe(): void {}
        disconnect(): void {}
      },
    )
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  /** A cell of `width` px that counts every write to its `textContent`. */
  function makeCell(width: number): { el: HTMLElement; writes: () => number; setWidth: (w: number) => void } {
    const el = document.createElement('span')
    document.body.appendChild(el)
    let currentWidth = width
    Object.defineProperty(el, 'clientWidth', { get: () => currentWidth })
    let count = 0
    let proto: object | null = Object.getPrototypeOf(el) as object
    let descriptor: PropertyDescriptor | undefined
    while (proto && !descriptor) {
      descriptor = Object.getOwnPropertyDescriptor(proto, 'textContent')
      proto = Object.getPrototypeOf(proto) as object | null
    }
    Object.defineProperty(el, 'textContent', {
      get: () => descriptor?.get?.call(el) as string,
      set: (value: string) => {
        count++
        descriptor?.set?.call(el, value)
      },
    })
    return {
      el,
      writes: () => count,
      setWidth: (w) => {
        currentWidth = w
      },
    }
  }

  async function settle(): Promise<void> {
    // The action loads pretext through a dynamic import before its first truncation.
    await new Promise((resolve) => setTimeout(resolve, 0))
  }

  function resize(): void {
    for (const callback of resizeCallbacks) callback()
  }

  it('skips the rewrite when a resize leaves the width unchanged', async () => {
    const cell = makeCell(200)
    const action = useShortenMiddle(cell.el, { text: 'a-long-enough-file-name.txt' })
    await settle()
    const before = cell.writes()

    resize()
    resize()

    expect(cell.writes()).toBe(before)
    action.destroy?.()
  })

  it('skips the rewrite when a new width still fits the same text', async () => {
    // 'short.txt' is 63 px wide: every width in the storm shows it whole.
    const cell = makeCell(200)
    const action = useShortenMiddle(cell.el, { text: 'short.txt' })
    await settle()
    const before = cell.writes()

    for (const width of [190, 180, 170, 160]) {
      cell.setWidth(width)
      resize()
    }

    expect(cell.writes()).toBe(before)
    action.destroy?.()
  })

  it('still rewrites when the width truncates the text differently', async () => {
    const cell = makeCell(400)
    const text = 'a-rather-long-file-name-that-needs-room.txt'
    const action = useShortenMiddle(cell.el, { text })
    await settle()
    expect(cell.el.textContent).toBe(text)

    cell.setWidth(100)
    resize()

    expect(cell.el.textContent).not.toBe(text)
    expect(cell.el.textContent).toContain('…')
    action.destroy?.()
  })
})
