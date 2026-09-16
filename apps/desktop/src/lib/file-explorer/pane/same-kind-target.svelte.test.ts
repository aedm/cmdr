/**
 * The palette row for `selection.selectSameKind` says what the command would do
 * RIGHT NOW, so the user reads the effect before pressing Enter.
 *
 * The pane publishes its cursor row's target; the registry's `displayName`
 * resolver renders it. Both ends are pinned here, because a break in either one
 * is invisible: the row silently falls back to the static name and still works.
 */
import { describe, it, expect, beforeEach } from 'vitest'
import { commands } from '$lib/commands/command-registry'
import {
  _resetForTesting,
  getSameKindTarget,
  publishSameKindTarget,
  sameKindCommandLabel,
} from './same-kind-target.svelte'

/** The registry entry the palette renders. */
function selectSameKind() {
  const command = commands.find((c) => c.id === 'selection.selectSameKind')
  if (!command) throw new Error('selection.selectSameKind is missing from the registry')
  return command
}

beforeEach(() => {
  _resetForTesting()
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
