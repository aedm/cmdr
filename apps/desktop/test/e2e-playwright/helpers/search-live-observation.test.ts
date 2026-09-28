/**
 * Regression coverage for the live-walk E2E's transient-state observation.
 *
 * The app may finish the walk between any two webview round trips. The test still
 * has to recognize a valid live snapshot that existed before that transition,
 * rather than combining rows from one render with status from the next.
 */

import { beforeEach, describe, expect, it } from 'vitest'
import type { PageLike } from '../search-helpers.js'
import { showsLiveWalkResult } from '../search-helpers.js'

let countCalls = 0

const page = {
  count: (selector: string): Promise<number> => {
    countCalls += 1
    const count = document.querySelectorAll(selector).length
    // Reproduce the CI race: the first IPC read sees the newly arrived row, then
    // the walk completes before the next IPC read can inspect its live status.
    if (selector.endsWith('.result-row')) {
      document.querySelector('.status-stop')?.remove()
      document.querySelector('.status-progress')?.remove()
      const status = document.querySelector('.status-text')
      if (status) status.textContent = '1 result'
    }
    return Promise.resolve(count)
  },
  evaluate: (js: string): Promise<unknown> => {
    // eslint-disable-next-line @typescript-eslint/no-implied-eval -- the evaluate payload is the webview program under test.
    const run = new Function(`return ${js}`) as () => unknown
    return Promise.resolve(run())
  },
} as unknown as PageLike

describe('showsLiveWalkResult', () => {
  beforeEach(() => {
    countCalls = 0
    document.body.innerHTML = `
      <div class="search-overlay">
        <div class="result-row"></div>
        <button class="status-stop">Stop</button>
        <span class="status-text">1 result so far</span>
        <span class="status-progress">2 folders scanned</span>
      </div>
    `
  })

  it('observes the transient live state in one webview snapshot', async () => {
    expect(await showsLiveWalkResult(page)).toBe(true)
    expect(countCalls).toBe(0)
  })
})
