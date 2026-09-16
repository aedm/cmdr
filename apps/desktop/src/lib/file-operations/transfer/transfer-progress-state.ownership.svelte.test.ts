/**
 * Headless tests for `createTransferProgressState`: who owns the operation the
 * progress dialog's view shows. The foreground slot it claims and releases on
 * every route out, an operation it adopted from the queue instead of
 * dispatching, and an adopted reversal whose only ending is leaving the registry.
 *
 * The shared mocks, fixtures, and per-test lifecycle (and why they're shaped
 * that way) live in `test-transfer-progress-harness.svelte.ts`. The rest of the
 * view's life is in `transfer-progress-state.svelte.test.ts`.
 */

import { describe, it, expect, vi } from 'vitest'
import { flushSync } from 'svelte'
import type { OperationSnapshot, WriteOperationStartResult } from '$lib/tauri-commands'

vi.mock('$lib/tauri-commands', async () =>
  (await import('./test-transfer-progress-harness.svelte')).tauriCommandsMock(),
)
vi.mock('$lib/file-operations/queue/queue-window', async () =>
  (await import('./test-transfer-progress-harness.svelte')).queueWindowMock(),
)
vi.mock('$lib/ui/toast', async () => (await import('./test-transfer-progress-harness.svelte')).toastMock())
vi.mock('$lib/settings', async () => (await import('./test-transfer-progress-harness.svelte')).settingsMock())
vi.mock('$lib/intl/messages.svelte', async () =>
  (await import('./test-transfer-progress-harness.svelte')).messagesMock(),
)
vi.mock('../progress-readout', async (importOriginal) =>
  (await import('./test-transfer-progress-harness.svelte')).progressReadoutMock(
    await importOriginal<typeof import('../progress-readout')>(),
  ),
)
vi.mock('$lib/logging/logger', async () => (await import('./test-transfer-progress-harness.svelte')).loggerMock())

import { createTransferProgressState } from './transfer-progress-state.svelte'
import { copyBetweenVolumes, cancelOperation, cancelWriteOperation, listOperations } from '$lib/tauri-commands'
import { openQueueWindow } from '$lib/file-operations/queue/queue-window'
import { addToast } from '$lib/ui/toast'
import { createEtaSmoother } from '../progress-readout'
import { getOperationSessions } from '../operation-session/window-operation-sessions.svelte'
import {
  getForegroundOperationId,
  setForegroundOperationId,
  isForegroundClaimPending,
} from '../foreground-operation.svelte'
import {
  listeners,
  makeConfig,
  progressEvent,
  settle,
  snapshot,
  useTransferProgressState,
} from './test-transfer-progress-harness.svelte'

const { makeState, startedState } = useTransferProgressState(createTransferProgressState)

describe('createTransferProgressState: foreground-operation ownership', () => {
  // The slot tells ambient surfaces (the corner chip, the failure notice) which
  // operation the modal is already showing in full. It has to empty on EVERY
  // route out of the dialog, and it has to empty at the moment Queue hands the
  // operation over — that's precisely when the chip must start speaking.
  it('claims the slot once the operation id lands', async () => {
    await startedState()
    expect(getForegroundOperationId()).toBe('op-1')
  })

  it('never claims the slot when the dialog is torn down before the id arrives', async () => {
    let resolveDispatch: (r: WriteOperationStartResult) => void = () => {}
    vi.mocked(copyBetweenVolumes).mockImplementationOnce(
      () => new Promise<WriteOperationStartResult>((res) => (resolveDispatch = res)),
    )
    const state = makeState(makeConfig())
    state.start()
    await settle()
    state.destroy()
    resolveDispatch({ operationId: 'op-1', operationType: 'copy' })
    await settle()
    expect(getForegroundOperationId()).toBeNull()
  })

  it('releases the slot on Queue, so the corner can pick the operation up', async () => {
    const { state } = await startedState()
    state.handleQueue()
    expect(getForegroundOperationId()).toBeNull()
  })

  it('releases the slot when the manager auto-queues the op behind a busy lane', async () => {
    await startedState()
    if (!listeners.opsChanged) throw new Error('operations-changed subscriber never registered')
    listeners.opsChanged({ operations: [snapshot('busy', 'running'), snapshot('op-1', 'queued')] })
    flushSync()
    expect(getForegroundOperationId()).toBeNull()
  })

  it('releases the slot when the dialog unmounts after completing', async () => {
    const { state } = await startedState()
    if (!listeners.complete) throw new Error('complete subscriber never registered')
    listeners.complete({
      operationId: 'op-1',
      operationType: 'copy',
      filesProcessed: 1,
      filesSkipped: 0,
      bytesProcessed: 1,
    })
    flushSync()
    vi.advanceTimersByTime(450)
    state.destroy()
    expect(getForegroundOperationId()).toBeNull()
  })

  it('releases the slot when the dialog unmounts after a cancel', async () => {
    const { state } = await startedState()
    void state.handleCancel(false)
    await settle()
    if (!listeners.cancelled || !listeners.settled) throw new Error('cancel subscribers never registered')
    listeners.cancelled({
      operationId: 'op-1',
      operationType: 'copy',
      filesProcessed: 0,
      rollback: {
        outcome: 'notRolledBack',
        reversed: 0,
        skips: [],
        stagedLeftovers: null,
        originalsStillInPlace: null,
      },
    })
    listeners.settled({ operationId: 'op-1', operationType: 'copy' })
    state.destroy()
    expect(getForegroundOperationId()).toBeNull()
  })

  it('releases the slot when the dialog unmounts after an error', async () => {
    const { state } = await startedState()
    if (!listeners.error) throw new Error('error subscriber never registered')
    listeners.error({
      operationId: 'op-1',
      operationType: 'copy',
      error: { type: 'io_error', path: '/src/file.txt', message: 'boom' },
      progressAtStop: null,
    })
    flushSync()
    state.destroy()
    expect(getForegroundOperationId()).toBeNull()
  })

  it('flags a claim while the dispatch is in flight, and settles it with the id', async () => {
    // The conflict host defers its ownership decision while this is up: a
    // `write-conflict` can beat the start command's response, and deciding
    // against an empty slot would prompt for an operation the modal owns.
    let resolveDispatch: (r: WriteOperationStartResult) => void = () => {}
    vi.mocked(copyBetweenVolumes).mockImplementationOnce(
      () => new Promise<WriteOperationStartResult>((res) => (resolveDispatch = res)),
    )
    const state = makeState(makeConfig())
    state.start()
    await settle()

    expect(isForegroundClaimPending()).toBe(true)
    expect(getForegroundOperationId()).toBeNull()

    resolveDispatch({ operationId: 'op-1', operationType: 'copy' })
    await settle()

    expect(isForegroundClaimPending()).toBe(false)
    expect(getForegroundOperationId()).toBe('op-1')
    state.destroy()
  })

  it('settles the claim when the dispatch itself never succeeds', async () => {
    // Nothing is ever going to own this operation, so a deferred conflict must
    // stop waiting on it rather than sit there forever.
    vi.mocked(copyBetweenVolumes).mockImplementationOnce(() => Promise.reject(new Error('ipc down')))
    const state = makeState(makeConfig())
    state.start()
    await settle()

    expect(isForegroundClaimPending()).toBe(false)
    state.destroy()
  })

  it('settles the claim when the dialog is torn down before the id arrives', async () => {
    let resolveDispatch: (r: WriteOperationStartResult) => void = () => {}
    vi.mocked(copyBetweenVolumes).mockImplementationOnce(
      () => new Promise<WriteOperationStartResult>((res) => (resolveDispatch = res)),
    )
    const state = makeState(makeConfig())
    state.start()
    await settle()
    state.destroy()
    resolveDispatch({ operationId: 'op-1', operationType: 'copy' })
    await settle()

    expect(isForegroundClaimPending()).toBe(false)
  })

  it('a late teardown does not release the slot the next dialog claimed', async () => {
    const { state } = await startedState()
    // The next operation's dialog mounts and claims the slot before this one
    // finishes tearing down.
    setForegroundOperationId('op-2')
    state.destroy()
    expect(getForegroundOperationId()).toBe('op-2')
  })
})

describe('createTransferProgressState: adopting a running operation', () => {
  // Foreground from the queue: the view binds an operation that started
  // somewhere else and dispatches nothing. Everything on screen comes from the
  // session, which is the same session every other surface in this window reads.

  /** The adopted operation is deliberately NOT the id the dispatch mock hands
   *  back: a view that quietly dispatched would land on `op-1`, so every
   *  assertion below would pass for the wrong reason. */
  const ADOPTED = 'op-9'

  /** Builds an adopting view and drains its startup, as `startedState` does for
   *  a dispatching one. */
  async function adoptedState(id = ADOPTED) {
    vi.mocked(listOperations).mockResolvedValue([snapshot(id, 'running')])
    const config = makeConfig({ adoptOperationId: id })
    const state = makeState(config)
    state.start()
    await settle()
    return { state, config }
  }

  it('leaves the operation alone when it closes before the session takes hold', async () => {
    // The sliver between the id landing and the binder's effect flushing: no
    // click can reach it, but a teardown can. Reporting a cancel from here
    // would run the pane tail over an operation that is still copying, so the
    // detach does what its name says and stops watching. Same refusal
    // `handleCancel` makes with no session to command.
    vi.mocked(listOperations).mockResolvedValue([snapshot(ADOPTED, 'running')])
    const config = makeConfig({ adoptOperationId: ADOPTED })
    const state = makeState(config)
    // ❌ Nothing may `await` between these two lines: the whole point is the
    // frame where `operationId` is set and `bound.current` is still null.
    state.start()
    state.detach()
    vi.advanceTimersByTime(450)

    expect(config.onCancelled).not.toHaveBeenCalled()
    expect(cancelOperation).not.toHaveBeenCalled()
    expect(cancelWriteOperation).not.toHaveBeenCalled()
    await settle()
  })

  it('binds the named operation without starting a new one', async () => {
    const { state } = await adoptedState()

    expect(copyBetweenVolumes).not.toHaveBeenCalled()
    expect(state.operationId).toBe(ADOPTED)
  })

  it('shows the live progress of the operation it adopted', async () => {
    const { state } = await adoptedState()
    if (!listeners.progress) throw new Error('progress subscriber never registered')

    listeners.progress(
      progressEvent({
        operationId: ADOPTED,
        phase: 'copying',
        filesDone: 7,
        filesTotal: 10,
        bytesDone: 700,
        bytesTotal: 1000,
      }),
    )

    expect(state.phase).toBe('copying')
    expect(state.filesDone).toBe(7)
    expect(state.bytesDone).toBe(700)
  })

  it('joins the session another surface already holds rather than estimating twice', async () => {
    // The whole reason the registry exists: a smoother started twenty minutes
    // in disagrees with the queue's for as long as it takes to converge. This is
    // the ordinary case for adoption — the corner chip is already watching.
    const registry = getOperationSessions()
    if (!registry) throw new Error('the window has no session registry')
    registry.acquire(ADOPTED)
    expect(vi.mocked(createEtaSmoother)).toHaveBeenCalledTimes(1)

    const { state } = await adoptedState()
    if (!listeners.progress) throw new Error('progress subscriber never registered')
    listeners.progress(progressEvent({ operationId: ADOPTED, etaSeconds: 80 }))

    expect(vi.mocked(createEtaSmoother)).toHaveBeenCalledTimes(1)
    expect(state.etaSecondsDisplay).toBe(80)
    registry.release(ADOPTED)
  })

  it('claims the foreground slot, so ambient surfaces stop repeating it', async () => {
    await adoptedState()
    expect(getForegroundOperationId()).toBe(ADOPTED)
  })

  it('hands the operation back, still running, when the view closes again', async () => {
    const { state, config } = await adoptedState()
    if (!listeners.progress) throw new Error('progress subscriber never registered')
    listeners.progress(progressEvent({ operationId: ADOPTED }))

    state.detach()

    expect(config.onQueue).toHaveBeenCalledTimes(1)
    expect(getForegroundOperationId()).toBeNull()
    state.destroy()
    expect(cancelOperation).not.toHaveBeenCalled()
    expect(cancelWriteOperation).not.toHaveBeenCalled()
  })

  it('keeps showing an operation the manager reports as queued', async () => {
    // Auto-queue is a decision a DISPATCHING view makes: don't stack a second
    // modal over the one already up. A view that was opened precisely to watch
    // this operation would instead bounce it straight back out of sight.
    vi.mocked(listOperations).mockResolvedValue([snapshot(ADOPTED, 'queued')])
    const config = makeConfig({ adoptOperationId: ADOPTED })
    const state = makeState(config)
    state.start()
    await settle()
    flushSync()

    expect(state.operationId).toBe(ADOPTED)
    expect(config.onQueue).not.toHaveBeenCalled()
  })

  it("says nothing about a phase it hasn't heard, rather than inventing the scan", async () => {
    // A dispatching view opens on `scanning`, because that is what a confirmed
    // transfer is about to do. An adopted operation could be anywhere, and a
    // window that has heard nothing (a reload, with the operation paused so no
    // tick is coming) would otherwise title a 21%-written copy "Verifying before
    // copy…" over an empty scan readout.
    const { state } = await adoptedState()

    expect(state.phase).toBeNull()

    if (!listeners.progress) throw new Error('progress subscriber never registered')
    listeners.progress(progressEvent({ operationId: ADOPTED, phase: 'copying' }))
    expect(state.phase).toBe('copying')
  })

  it('offers Rollback only where the operation says it can be reversed', async () => {
    // The snapshot is the authority: this view has no birth context to reason
    // about volumes from, and `supportsRollback` is a promise about the
    // operation itself.
    const { state } = await adoptedState()
    if (!listeners.opsChanged) throw new Error('operations-changed subscriber never registered')

    listeners.opsChanged({ operations: [{ ...snapshot(ADOPTED, 'running'), supportsRollback: false }] })
    expect(state.rollbackUnavailable).toBe(true)

    listeners.opsChanged({ operations: [snapshot(ADOPTED, 'running')] })
    expect(state.rollbackUnavailable).toBe(false)
  })
})

describe('createTransferProgressState: an adopted reversal', () => {
  // The operation-log reversal (an undo, adopted through Show on its queue row)
  // emits `write-progress` and no terminal event: no `write-cancelled`, no
  // `write-settled`, no `write-complete`. Dropping out of the registry is the only
  // word its end ever gets, so the view has to read `leftRegistry` for it.

  const REVERSAL = 'op-undo'

  function reversalSnapshot(): OperationSnapshot {
    return { ...snapshot(REVERSAL, 'running', 'delete'), reverses: 'copy' }
  }

  /** An adopted reversal that holds its registry row, optionally with a tick. */
  async function adoptedReversal({ withTick = true }: { withTick?: boolean } = {}) {
    vi.mocked(listOperations).mockResolvedValue([reversalSnapshot()])
    const config = makeConfig({ adoptOperationId: REVERSAL, operationType: 'delete' })
    const state = makeState(config)
    state.start()
    await settle()
    if (!listeners.opsChanged || !listeners.progress) throw new Error('subscribers never registered')
    listeners.opsChanged({ operations: [reversalSnapshot()] })
    if (withTick) {
      listeners.progress(
        progressEvent({ operationId: REVERSAL, operationType: 'delete', phase: 'rolling_back', filesDone: 3 }),
      )
    }
    flushSync()
    return { state, config }
  }

  it('closes as soon as a Cancel stops it, rather than waiting out the fallback', async () => {
    const { state, config } = await adoptedReversal()
    void state.handleCancel(false)
    await settle()
    expect(state.isCancelling).toBe(true)

    if (!listeners.opsChanged) throw new Error('operations-changed subscriber never registered')
    listeners.opsChanged({ operations: [] })
    flushSync()
    vi.advanceTimersByTime(450)

    expect(config.onCancelled).toHaveBeenCalledWith(3)
  })

  it('closes when it finishes on its own', async () => {
    const { config } = await adoptedReversal()

    if (!listeners.opsChanged) throw new Error('operations-changed subscriber never registered')
    listeners.opsChanged({ operations: [] })
    flushSync()
    vi.advanceTimersByTime(450)

    expect(config.onCancelled).toHaveBeenCalledWith(3)
  })

  it('never hands a reversal that already left the registry to the queue', async () => {
    // A window that heard no tick (a reload, with the reversal paused) has no
    // phase to read `rolling_back` from, so only `leftRegistry` says it's gone.
    const { state } = await adoptedReversal({ withTick: false })
    if (!listeners.opsChanged) throw new Error('operations-changed subscriber never registered')
    listeners.opsChanged({ operations: [] })
    flushSync()

    state.detach()

    expect(openQueueWindow).not.toHaveBeenCalled()
    expect(addToast).not.toHaveBeenCalled()
  })

  it('never reads an ORDINARY transfer leaving the registry as its ending', async () => {
    // The removal travels on a different channel than `write-complete`, so it can
    // arrive first. Closing on it would report a cancel for a copy that finished,
    // and run the wrong tail over the user's panes.
    const { config } = await startedState()
    if (!listeners.opsChanged || !listeners.complete) throw new Error('subscribers never registered')
    listeners.opsChanged({ operations: [snapshot('op-1', 'running')] })
    listeners.opsChanged({ operations: [] })
    flushSync()
    vi.advanceTimersByTime(450)
    expect(config.onCancelled).not.toHaveBeenCalled()

    listeners.complete({
      operationId: 'op-1',
      operationType: 'copy',
      filesProcessed: 2,
      filesSkipped: 0,
      bytesProcessed: 20,
    })
    flushSync()
    vi.advanceTimersByTime(450)
    expect(config.onComplete).toHaveBeenCalledTimes(1)
    expect(config.onCancelled).not.toHaveBeenCalled()
  })
})
