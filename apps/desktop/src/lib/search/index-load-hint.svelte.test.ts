/**
 * The index-load hint's clock.
 *
 * The regression anchor is the THRESHOLD: a load that lands quickly (the common case)
 * must leave the dialog silent, because a line that appears and vanishes inside a couple
 * of frames reads as a glitch rather than an explanation. The other half is that the
 * hint never outlives what it describes: a landed arena, a switch to another volume, or
 * a volume that has no index at all (which sets no pending volume in the first place).
 *
 * Runes, so the filename carries the `.svelte.` infix vite-plugin-svelte looks for.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { flushSync } from 'svelte'
import { createIndexLoadHint, INDEX_LOAD_HINT_DELAY_MS } from './index-load-hint.svelte'
import { setPendingIndexVolumeId } from './search-state.svelte'

describe('createIndexLoadHint', () => {
  let dispose: (() => void) | undefined
  let hint: { readonly visible: boolean }

  /** Mounts the hint in a standalone reactive root, the way the dialog's init does. */
  function start(): void {
    dispose = $effect.root(() => {
      hint = createIndexLoadHint()
    })
    flushSync()
  }

  /** Moves the clock and lets the effects that the timer wakes settle. */
  function advance(ms: number): void {
    vi.advanceTimersByTime(ms)
    flushSync()
  }

  beforeEach(() => {
    vi.useFakeTimers()
    setPendingIndexVolumeId(null)
  })

  afterEach(() => {
    dispose?.()
    dispose = undefined
    setPendingIndexVolumeId(null)
    vi.useRealTimers()
  })

  it('says nothing while the load is still inside the threshold', () => {
    setPendingIndexVolumeId('nas')
    start()

    expect(hint.visible).toBe(false)
    advance(INDEX_LOAD_HINT_DELAY_MS - 1)
    expect(hint.visible).toBe(false)
  })

  it('speaks once the load outlasts the threshold', () => {
    setPendingIndexVolumeId('nas')
    start()

    advance(INDEX_LOAD_HINT_DELAY_MS)
    expect(hint.visible).toBe(true)
  })

  it('goes quiet the moment the arena lands', () => {
    setPendingIndexVolumeId('nas')
    start()
    advance(INDEX_LOAD_HINT_DELAY_MS)
    expect(hint.visible).toBe(true)

    setPendingIndexVolumeId(null)
    flushSync()

    expect(hint.visible).toBe(false)
  })

  it('restarts the clock when the pending volume changes', () => {
    setPendingIndexVolumeId('nas')
    start()
    advance(INDEX_LOAD_HINT_DELAY_MS - 1)

    // The focused pane moved to another volume mid-dialog: this is a fresh wait, so the
    // leftover 1 ms of the old one must not push a hint onto the screen.
    setPendingIndexVolumeId('usb')
    flushSync()
    advance(1)
    expect(hint.visible).toBe(false)

    advance(INDEX_LOAD_HINT_DELAY_MS)
    expect(hint.visible).toBe(true)
  })

  it('stays silent when nothing is pending, however long the dialog is open', () => {
    // A volume with no index at all: `warmVolume` records no pending volume, so there is
    // nothing to wait for and nothing to say. `CoverageNote` owns that answer, after a run.
    start()

    advance(INDEX_LOAD_HINT_DELAY_MS * 10)
    expect(hint.visible).toBe(false)
  })

  it('leaves no timer behind when its scope is disposed', () => {
    setPendingIndexVolumeId('nas')
    start()

    dispose?.()
    dispose = undefined

    expect(vi.getTimerCount()).toBe(0)
  })
})
