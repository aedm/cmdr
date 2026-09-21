/**
 * The index-load hint, as the mounted dialog actually renders it.
 *
 * `index-load-hint.svelte.test.ts` pins the clock; this file pins the two things only a
 * mounted dialog can answer:
 *
 *   1. **A volume with no index never gets a hint.** `prepareSearchIndex` answering
 *      `{ ready: false, loading: false }` is the terminal "there's nothing here" case,
 *      and it must stay `CoverageNote`'s to answer after a run rather than being
 *      pre-announced as a wait that will never end.
 *   2. **The hint yields to the results area.** Once a run has been attempted,
 *      `QueryResults` shows its own loading state, and two sentences saying the same
 *      thing in one dialog is worse than either alone.
 *
 * Shared mount + IPC fixture: `test-search-dialog-harness.ts`.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { flushSync, tick } from 'svelte'
import SearchDialog from './SearchDialog.svelte'
import { tString } from '$lib/intl/messages.svelte'
import { SEARCH_AUTO_APPLY_DEBOUNCE_MS } from '$lib/query-ui/query-filter-state.svelte'
import { setPendingIndexVolumeId } from './search-state.svelte'
import { INDEX_LOAD_HINT_DELAY_MS } from './index-load-hint.svelte'
import {
  mountDialog,
  prepareSearchIndexMock,
  resetSearchDialogTest,
  unmountAllDialogs,
  useSearchDialog,
} from './test-search-dialog-harness'

vi.mock('$lib/tauri-commands', async () => (await import('./test-search-dialog-harness')).tauriCommandsMock())
vi.mock('../../routes/viewer/media-view', async () => (await import('./test-search-dialog-harness')).mediaViewMock())
vi.mock('$lib/settings', async () => (await import('./test-search-dialog-harness')).settingsMock())
vi.mock('$lib/indexing', async () => (await import('./test-search-dialog-harness')).indexingMock())
vi.mock('$lib/icon-cache', async () => (await import('./test-search-dialog-harness')).iconCacheMock())

useSearchDialog(SearchDialog)

/** The backend's answer, held open so the test decides when (and under which clock) it lands. */
interface PrepareAnswer {
  ready: boolean
  entryCount: number
  loading: boolean
}

describe('search dialog: index-load hint', () => {
  let settlePrepare: (answer: PrepareAnswer) => void = () => {}

  beforeEach(async () => {
    await resetSearchDialogTest()
    setPendingIndexVolumeId(null)
    // Hold the pre-load open across the mount, so the wait starts under FAKE timers and
    // the threshold is driven rather than slept through.
    prepareSearchIndexMock.mockImplementationOnce(
      () =>
        new Promise<PrepareAnswer>((resolve) => {
          settlePrepare = resolve
        }),
    )
  })

  afterEach(() => {
    unmountAllDialogs()
    setPendingIndexVolumeId(null)
    vi.useRealTimers()
  })

  /** Mounts under real timers (the harness awaits real macrotasks), then takes the clock. */
  async function mountWaiting(): Promise<Element> {
    const { overlay } = await mountDialog()
    vi.useFakeTimers()
    return overlay
  }

  /** Lands the backend's answer and lets the state writes it triggers settle. */
  async function landAnswer(answer: PrepareAnswer): Promise<void> {
    settlePrepare(answer)
    await tick()
    await tick()
    flushSync()
  }

  /** Moves the clock and lets the effects the timers wake settle. */
  function advance(ms: number): void {
    vi.advanceTimersByTime(ms)
    flushSync()
  }

  /** The hint's rendered sentence, or `''` when it isn't on screen. */
  function hintText(overlay: Element): string {
    return (overlay.querySelector('.index-load-hint .message')?.textContent ?? '').trim()
  }

  it('stays silent while a load is still inside the threshold', async () => {
    const overlay = await mountWaiting()
    await landAnswer({ ready: false, entryCount: 0, loading: true })

    expect(hintText(overlay)).toBe('')
    advance(INDEX_LOAD_HINT_DELAY_MS - 1)
    expect(hintText(overlay)).toBe('')
  })

  it('names the wait once the load outlasts the threshold', async () => {
    const overlay = await mountWaiting()
    await landAnswer({ ready: false, entryCount: 0, loading: true })

    advance(INDEX_LOAD_HINT_DELAY_MS)

    expect(hintText(overlay)).toBe(tString('queryUi.results.loadingIndex'))
  })

  it('never speaks for a volume that has no index to load', async () => {
    const overlay = await mountWaiting()
    // The terminal answer: not ready, and nothing on its way. No `search-index-ready`
    // event is coming, so a "loading" hint here would wait forever.
    await landAnswer({ ready: false, entryCount: 0, loading: false })

    advance(INDEX_LOAD_HINT_DELAY_MS * 10)

    expect(hintText(overlay)).toBe('')
  })

  it('yields once a run has been attempted, so the results area speaks alone', async () => {
    const overlay = await mountWaiting()
    await landAnswer({ ready: false, entryCount: 0, loading: true })
    advance(INDEX_LOAD_HINT_DELAY_MS)
    expect(hintText(overlay)).toBe(tString('queryUi.results.loadingIndex'))

    const input = overlay.querySelector<HTMLInputElement>('.query-bar input')
    if (!input) throw new Error('query input not found')
    input.value = 'invoice'
    input.dispatchEvent(new Event('input', { bubbles: true }))
    advance(SEARCH_AUTO_APPLY_DEBOUNCE_MS)
    await tick()
    flushSync()

    expect(hintText(overlay)).toBe('')
  })
})
