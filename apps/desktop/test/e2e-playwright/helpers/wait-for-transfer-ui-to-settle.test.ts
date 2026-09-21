/**
 * Unit tests for `waitForTransferUiToSettle`, the UI-side sibling of
 * `waitForBackendOperationsToSettle`.
 *
 * They exist because the two are trivially confusable and the wrong one passes on a
 * fast machine: an empty backend registry says nothing about whether the frontend has
 * taken `write-complete` off the event bridge and closed the progress dialog. happy-dom
 * can hold both halves — a fake `list_operations` and a real dialog element that
 * unmounts late — so the gap is anchorable without an app.
 *
 * The payloads under test are the `evaluate` STRINGS the helper ships into the webview,
 * so these tests execute those exact strings; paraphrasing them here would anchor a
 * different program than the suite runs.
 */

import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type { PageLike } from './core.js'
import { waitForTransferUiToSettle } from './app-lifecycle.js'

/** What the fake `list_operations` answers with, mutated per test. */
let operations: { operationId: string }[] = []

/** Held so a test's pending unmount timer can't outlive its happy-dom environment. */
let timer: ReturnType<typeof setTimeout> | undefined

/**
 * A `PageLike` whose `evaluate` runs the helper's real payload against the happy-dom
 * document, with a `__TAURI_INTERNALS__.invoke` that answers `list_operations` the way
 * the backend does.
 */
const page = {
  evaluate: (js: string): Promise<unknown> => {
    // eslint-disable-next-line @typescript-eslint/no-implied-eval -- the evaluate payload IS the code under test; the whole point is to run it verbatim.
    const run = new Function(`return ${js}`) as () => unknown
    return Promise.resolve(run())
  },
} as unknown as PageLike

function mountProgressDialog(): void {
  document.body.innerHTML = '<div class="modal-overlay" data-dialog-id="transfer-progress">Copying…</div>'
}

describe('waitForTransferUiToSettle', () => {
  beforeEach(() => {
    operations = []
    document.body.innerHTML = ''
    ;(window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      invoke: (command: string): Promise<unknown> =>
        command === 'list_operations' ? Promise.resolve(operations) : Promise.resolve(null),
    }
  })

  afterEach(() => {
    if (timer !== undefined) clearTimeout(timer)
    timer = undefined
  })

  it('waits for the dialog to unmount, not just for the backend registry to empty', async () => {
    // The shape that reads as flake: the operation has settled, so a backend-only wait
    // returns at once, while the frontend still has to take `write-complete` off the
    // event bridge and close the dialog (measured at 650 ms in a loaded lane). A spec
    // asserting "no overlay" right after then fails on a dialog that is on its way out.
    mountProgressDialog()
    let unmounted = false
    timer = setTimeout(() => {
      document.body.innerHTML = ''
      unmounted = true
    }, 120)

    await waitForTransferUiToSettle(page)

    expect(unmounted).toBe(true)
  })

  it('names the transfer surface that stayed up', async () => {
    mountProgressDialog()

    await expect(waitForTransferUiToSettle(page, { uiTimeout: 200 })).rejects.toThrow(/transfer-progress/)
  })

  it('fails on the backend half first, so a never-finishing operation says so', async () => {
    operations = [{ operationId: 'op-1' }]

    await expect(waitForTransferUiToSettle(page, { backendTimeout: 200 })).rejects.toThrow()
  })

  it('returns once nothing transfer-shaped is mounted, leaving other overlays alone', async () => {
    // Scoped to the transfer surfaces on purpose: a spec that ends in the rename editor
    // or with an unrelated popover open is not waiting on a transfer.
    document.body.innerHTML = '<div class="modal-overlay" data-dialog-id="mkdir-confirmation"></div>'

    await waitForTransferUiToSettle(page, { uiTimeout: 200 })
  })
})
