/**
 * `adoptStoredSpelling`: a pane whose path was a spelling its volume doesn't
 * store (a typed path, a restored tab, a pane carried over from the macOS kernel
 * mount) takes the stored one when the listing lands there. The SAME folder, so
 * the tab and its history are re-spelled in place: no Back step to the other
 * spelling, and no pinned-tab fork.
 */
import { describe, it, expect } from 'vitest'
import { adoptStoredSpelling, commitPathFromListing } from './navigate'
import { makeHarness } from './navigate.test-fixtures'
import { getActiveTab } from '../tabs/tab-state-manager.svelte'

/** `fotók` as the kernel mount spells it (decomposed) and as the share stores it. */
const KERNEL = '/Volumes/Ext/fotók'
const STORED = '/Volumes/Ext/fotók'

describe('adoptStoredSpelling', () => {
  it('re-spells the tab and its current history entry in place, and remembers the stored spelling', () => {
    const h = makeHarness({ left: { path: KERNEL, volumeId: 'ext' } })
    const depthBefore = h.tab('left').history.stack.length

    adoptStoredSpelling(h.deps, 'left', { from: KERNEL, to: STORED })

    expect(h.tab('left').path).toBe(STORED)
    const history = h.tab('left').history
    expect(history.stack.length).toBe(depthBefore)
    expect(history.stack[history.currentIndex].path).toBe(STORED)
    expect(h.lastUsedRecords).toContainEqual({ volumeId: 'ext', path: STORED })
  })

  it('then lets the landing commit without a second history entry', () => {
    const h = makeHarness({ left: { path: KERNEL, volumeId: 'ext' } })
    const depthBefore = h.tab('left').history.stack.length

    adoptStoredSpelling(h.deps, 'left', { from: KERNEL, to: STORED })
    expect(commitPathFromListing(h.deps, 'left', STORED)).toBe(true)

    expect(h.tab('left').history.stack.length).toBe(depthBefore)
  })

  it('keeps a pinned tab in place rather than forking a new one for the same folder', () => {
    const h = makeHarness({ left: { path: KERNEL, volumeId: 'ext' } })
    getActiveTab(h.mgr('left')).pinned = true
    const tabsBefore = h.mgr('left').tabs.length

    adoptStoredSpelling(h.deps, 'left', { from: KERNEL, to: STORED })
    commitPathFromListing(h.deps, 'left', STORED)

    expect(h.mgr('left').tabs.length).toBe(tabsBefore)
    expect(h.tab('left').path).toBe(STORED)
  })

  it('leaves a pane that is somewhere else alone', () => {
    const h = makeHarness({ left: { path: '/Volumes/Ext', volumeId: 'ext' } })

    adoptStoredSpelling(h.deps, 'left', { from: KERNEL, to: STORED })

    expect(h.tab('left').path).toBe('/Volumes/Ext')
    expect(h.lastUsedRecords).not.toContainEqual({ volumeId: 'ext', path: STORED })
  })
})
