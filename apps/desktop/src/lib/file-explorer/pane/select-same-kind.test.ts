import { describe, it, expect } from 'vitest'
import type { FileEntry } from '../types'
import { sameKindIndices, sameKindTargetFor } from './select-same-kind'
import { createSelectionState } from './selection-state.svelte'

/** A listing row, terse enough that a case reads as the folder it describes. */
function entry(name: string, isDirectory = false): FileEntry {
  return {
    name,
    path: `/tmp/${name}`,
    isDirectory,
    isSymlink: false,
    permissions: 0o644,
    owner: '',
    group: '',
    iconId: isDirectory ? 'dir' : 'file',
    extendedMetadataLoaded: true,
  }
}

/** The synthetic `..` row a pane puts at index 0 when it has a parent. */
const parentRow: FileEntry = { ...entry('..', true), path: '/' }

describe('sameKindTargetFor', () => {
  it('reads a folder as "every folder", whatever dots its name carries', () => {
    expect(sameKindTargetFor(entry('Documents', true))).toEqual({ kind: 'allFolders' })
    expect(sameKindTargetFor(entry('name.like.this', true))).toEqual({ kind: 'allFolders' })
  })

  it('reads a file with an extension as that extension, lowercased', () => {
    expect(sameKindTargetFor(entry('report.pdf'))).toEqual({ kind: 'sameExtension', extension: 'pdf' })
    expect(sameKindTargetFor(entry('SCAN.PDF'))).toEqual({ kind: 'sameExtension', extension: 'pdf' })
  })

  it('reads `archive.tar.gz` as `gz`, the last segment the Ext column shows', () => {
    expect(sameKindTargetFor(entry('archive.tar.gz'))).toEqual({ kind: 'sameExtension', extension: 'gz' })
  })

  it('reads a dotfile with no second dot as extension-less, like the Ext column does', () => {
    expect(sameKindTargetFor(entry('.gitignore'))).toEqual({ kind: 'noExtension' })
  })

  it('reads a dotfile WITH a second dot by its last segment', () => {
    expect(sameKindTargetFor(entry('.eslintrc.json'))).toEqual({ kind: 'sameExtension', extension: 'json' })
  })

  it('reads a trailing-dot name as extension-less, matching the Ext column (not `getExtension`)', () => {
    expect(sameKindTargetFor(entry('file.'))).toEqual({ kind: 'noExtension' })
  })

  it('reads a plain extension-less file as extension-less', () => {
    expect(sameKindTargetFor(entry('README'))).toEqual({ kind: 'noExtension' })
  })

  it('has no target on the `..` row', () => {
    expect(sameKindTargetFor(parentRow)).toBeNull()
  })

  it('has no target when nothing is under the cursor', () => {
    expect(sameKindTargetFor(null)).toBeNull()
    expect(sameKindTargetFor(undefined)).toBeNull()
  })
})

describe('sameKindIndices', () => {
  it('selects every folder and no file, skipping the `..` row', () => {
    const entries = [parentRow, entry('src', true), entry('report.pdf'), entry('name.like.this', true)]
    expect(sameKindIndices({ kind: 'allFolders' }, entries)).toEqual([1, 3])
  })

  it('matches an extension case-insensitively and never crosses into folders', () => {
    const entries = [
      entry('report.pdf'),
      entry('SCAN.PDF'),
      entry('notes.pdfx'),
      entry('deck.pdf.bak'),
      entry('archive.pdf', true),
    ]
    expect(sameKindIndices({ kind: 'sameExtension', extension: 'pdf' }, entries)).toEqual([0, 1])
  })

  it('matches `gz` on the last segment only, so `.tar` stays out', () => {
    const entries = [entry('archive.tar.gz'), entry('backup.gz'), entry('plain.tar')]
    expect(sameKindIndices({ kind: 'sameExtension', extension: 'gz' }, entries)).toEqual([0, 1])
  })

  it('groups every extension-less file together, folders excluded', () => {
    const entries = [
      parentRow,
      entry('README'),
      entry('.gitignore'),
      entry('file.'),
      entry('bin', true),
      entry('a.txt'),
    ]
    expect(sameKindIndices({ kind: 'noExtension' }, entries)).toEqual([1, 2, 3])
  })

  it('returns an empty list when the listing holds nothing of that kind', () => {
    expect(sameKindIndices({ kind: 'allFolders' }, [entry('a.txt')])).toEqual([])
    expect(sameKindIndices({ kind: 'noExtension' }, [])).toEqual([])
  })

  it('includes the cursor row itself, so pressing the key on an unselected row selects it', () => {
    const entries = [entry('report.pdf'), entry('other.pdf')]
    expect(sameKindIndices({ kind: 'sameExtension', extension: 'pdf' }, entries)).toContain(0)
  })
})

/**
 * Pins `FilePane.selectSameKind`'s effect on a real selection state without
 * mounting the component (the approach `first-selected-index.test.ts` takes).
 * Replicates the method body after its two awaits: target → indices → ADD.
 */
function runSelectSameKind(
  cursorEntry: FileEntry | null,
  entries: FileEntry[],
  hasParent: boolean,
  selection: ReturnType<typeof createSelectionState>,
): void {
  const target = sameKindTargetFor(cursorEntry)
  if (!target) return
  const idxs = sameKindIndices(target, entries)
  if (idxs.length === 0) return
  selection.applyIndices(idxs, 'add', hasParent)
}

describe('selecting same-kind rows in a pane', () => {
  const entries = [parentRow, entry('src', true), entry('a.pdf'), entry('b.txt'), entry('c.pdf')]

  it('ADDS to the selection, Total Commander style, and never clears it', () => {
    const selection = createSelectionState()
    selection.setSelectedIndices([3]) // `b.txt`, a kind the command won't match
    runSelectSameKind(entries[2], entries, true, selection)
    expect(selection.getSelectedIndices()).toEqual([2, 3, 4])
  })

  it('never selects the `..` row, even when the cursor sits on a folder', () => {
    const selection = createSelectionState()
    runSelectSameKind(entries[1], entries, true, selection)
    expect(selection.getSelectedIndices()).toEqual([1])
  })

  it('leaves the selection alone on the `..` row', () => {
    const selection = createSelectionState()
    selection.setSelectedIndices([2])
    runSelectSameKind(parentRow, entries, true, selection)
    expect(selection.getSelectedIndices()).toEqual([2])
  })
})
