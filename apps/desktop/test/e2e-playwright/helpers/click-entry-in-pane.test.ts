/**
 * Unit tests for `clickEntryInPane`, the click `ensureAppReady` puts on the first
 * row of the left pane before every E2E test.
 *
 * The race they anchor: a pane replaces every row when a listing lands, and
 * `ensureAppReady`'s own navigation lands one right after a route remount. Measured
 * in the E2E lane (2026-09-12): every row of both panes was removed ~25 ms before
 * the click on an idle machine, so the helper has to wait a row out rather than
 * report a click it never made.
 *
 * Like `click-button-by-text.test.ts`, these execute the helper's real `evaluate`
 * payload against happy-dom, so the program under test is the one the suite ships.
 *
 * ❗ Every timer a test schedules goes through `scheduleRender`, so `afterEach` can
 * cancel one that never fired. A pending timer outlives the file's happy-dom
 * environment and fires into a torn-down world, which vitest reports as an
 * unhandled `ReferenceError: document is not defined` and a red lane, with every
 * test still passing.
 */

import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type { PageLike } from './core.js'
import { clickEntryInPane } from './core.js'

/** `data-filename` of every row whose click handler ran, in order. */
let clicks: string[] = []

/** Render timers still pending, cleared in `afterEach` so none outlives the DOM. */
let timers: ReturnType<typeof setTimeout>[] = []

/** Every `evaluate` payload the helper sent, so its round trips can be counted. */
let payloads: string[] = []

/** Replaces pane 0's rows with fresh elements, the way a landed listing re-renders them. */
function renderRows(): void {
  const list = document.querySelector('.file-pane .rows')
  if (!list) return
  list.innerHTML = ''
  for (const name of ['..', 'file-a.txt']) {
    const row = document.createElement('div')
    row.className = 'file-entry'
    row.setAttribute('data-filename', name)
    row.addEventListener('click', () => {
      clicks.push(name)
    })
    list.appendChild(row)
  }
}

/** `renderRows` a beat later, with the handle held so `afterEach` can cancel it. */
function scheduleRender(delayMs: number): void {
  timers.push(setTimeout(renderRows, delayMs))
}

/** A `PageLike` running each payload against happy-dom, the way the webview would. */
const page = {
  evaluate: (js: string): Promise<unknown> => {
    payloads.push(js)
    // eslint-disable-next-line @typescript-eslint/no-implied-eval -- the evaluate payload IS the code under test; the whole point is to run it verbatim.
    const run = new Function(`return ${js}`) as () => unknown
    return Promise.resolve(run())
  },
} as unknown as PageLike

describe('clickEntryInPane', () => {
  beforeEach(() => {
    clicks = []
    timers = []
    payloads = []
    document.body.innerHTML = '<div class="file-pane"><div class="rows"></div></div>'
  })

  afterEach(() => {
    for (const timer of timers) clearTimeout(timer)
    timers = []
  })

  it('clicks the row the caller asked for, and only that one', async () => {
    renderRows()

    await clickEntryInPane(page, 0)

    expect(clicks).toEqual(['..'])
  })

  it('finds the row and clicks it in one round trip, so a re-render cannot land between the two', async () => {
    // The bug this forbids: checking the row exists, then clicking it in a SECOND
    // `evaluate`. A listing landing in the gap leaves the click aimed at a row that
    // has been replaced, which flaked the i18n staging spec on a loaded run.
    renderRows()

    await clickEntryInPane(page, 0)

    expect(payloads).toHaveLength(1)
    expect(clicks).toEqual(['..'])
  })

  it('waits for a row that renders only once its listing lands', async () => {
    scheduleRender(80)

    await clickEntryInPane(page, 0, 1)

    expect(clicks).toEqual(['file-a.txt'])
  })
})
