/**
 * The palette row for `selection.selectSameKind` says what the command would do
 * RIGHT NOW, so the user reads the effect before pressing Enter.
 *
 * The pane publishes its cursor row's target; the registry's `displayName`
 * resolver renders it. Both ends are pinned here, because a break in either one
 * is invisible: the row silently falls back to the static name and still works.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { commands } from '$lib/commands/command-registry'
import {
  _resetForTesting,
  getSameKindTarget,
  publishSameKindTarget,
  resyncSameKindMenu,
  sameKindCommandLabel,
} from './same-kind-target.svelte'

const updateSelectSameKindMenu = vi.hoisted(() => vi.fn().mockResolvedValue(undefined))
vi.mock('$lib/tauri-commands', () => ({ updateSelectSameKindMenu }))

/** The registry entry the palette renders. */
function selectSameKind() {
  const command = commands.find((c) => c.id === 'selection.selectSameKind')
  if (!command) throw new Error('selection.selectSameKind is missing from the registry')
  return command
}

beforeEach(() => {
  vi.useFakeTimers()
  _resetForTesting()
  updateSelectSameKindMenu.mockClear()
})

afterEach(() => {
  vi.useRealTimers()
})

describe('sameKindCommandLabel', () => {
  it('falls back to the static name when the cursor row has no kind (the `..` row, an empty pane)', () => {
    expect(getSameKindTarget()).toBeNull()
    expect(sameKindCommandLabel()).toBe('Select all of the same kind')
  })

  it('names the extension the cursor row carries', () => {
    publishSameKindTarget({ kind: 'sameExtension', extension: 'pdf' })
    expect(sameKindCommandLabel()).toBe('Select all with extension *.pdf')
  })

  it('says folders when the cursor is on a folder', () => {
    publishSameKindTarget({ kind: 'allFolders' })
    expect(sameKindCommandLabel()).toBe('Select all folders')
  })

  it('says extension-less when the cursor is on a file without one', () => {
    publishSameKindTarget({ kind: 'noExtension' })
    expect(sameKindCommandLabel()).toBe('Select all files with no extension')
  })
})

describe('the registry entry', () => {
  it('shows the live label in the palette and the static one everywhere else', () => {
    publishSameKindTarget({ kind: 'sameExtension', extension: 'gz' })
    expect(selectSameKind().displayName).toBe('Select all with extension *.gz')
    // Settings > Shortcuts, the help window, the conflict toast, and the MCP
    // bridge read `name`; a live label there would make the shortcut row move.
    expect(selectSameKind().name).toBe('Select all of the same kind')
  })

  it('follows the cursor: a second publish re-renders the row', () => {
    publishSameKindTarget({ kind: 'sameExtension', extension: 'pdf' })
    expect(selectSameKind().displayName).toBe('Select all with extension *.pdf')
    publishSameKindTarget({ kind: 'allFolders' })
    expect(selectSameKind().displayName).toBe('Select all folders')
  })
})

describe('the menu-bar push', () => {
  it('waits out the debounce, then sends the LAST target of the burst', () => {
    publishSameKindTarget({ kind: 'sameExtension', extension: 'pdf' })
    publishSameKindTarget({ kind: 'noExtension' })
    publishSameKindTarget({ kind: 'allFolders' })
    expect(updateSelectSameKindMenu).not.toHaveBeenCalled()

    vi.advanceTimersByTime(200)
    expect(updateSelectSameKindMenu).toHaveBeenCalledTimes(1)
    expect(updateSelectSameKindMenu).toHaveBeenCalledWith({ kind: 'allFolders' })
  })

  it('stays quiet when the words would not change: arrowing down a run of .pdf files', () => {
    publishSameKindTarget({ kind: 'sameExtension', extension: 'pdf' })
    vi.advanceTimersByTime(200)
    expect(updateSelectSameKindMenu).toHaveBeenCalledTimes(1)

    publishSameKindTarget({ kind: 'sameExtension', extension: 'pdf' })
    vi.advanceTimersByTime(200)
    expect(updateSelectSameKindMenu).toHaveBeenCalledTimes(1)
  })

  it('pushes null for a row with no kind, so the item falls back to its neutral label', () => {
    publishSameKindTarget({ kind: 'allFolders' })
    vi.advanceTimersByTime(200)
    publishSameKindTarget(null)
    vi.advanceTimersByTime(200)
    expect(updateSelectSameKindMenu).toHaveBeenLastCalledWith(null)
  })

  it('re-pushes after a language rebuild, which threw the old item away', () => {
    publishSameKindTarget({ kind: 'sameExtension', extension: 'pdf' })
    vi.advanceTimersByTime(200)
    expect(updateSelectSameKindMenu).toHaveBeenCalledTimes(1)

    resyncSameKindMenu()
    expect(updateSelectSameKindMenu).toHaveBeenCalledTimes(2)
    expect(updateSelectSameKindMenu).toHaveBeenLastCalledWith({ kind: 'sameExtension', extension: 'pdf' })
  })
})
