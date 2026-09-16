/**
 * Pure matcher behind `selection.selectSameKind` ("select every entry of the same
 * kind as the row under the cursor", Total Commander's `Alt+Num +`).
 *
 * Two steps, so the palette label and the selection can share one answer: read
 * the cursor row into a `SameKindTarget`, then ask the pane's whole-listing
 * snapshot which rows match it. The command ADDS its matches to whatever is
 * already selected; it never clears (that's `selection-state::applyIndices` in
 * `'add'` mode, and it's why the cursor row itself is always part of the result).
 *
 * The extension rule is `getDisplayExtension`, the same one the **Ext column the
 * user is looking at** renders and Rust sorts by — ❌ never `getExtension` from
 * `$lib/utils/filename-validation`, which returns the last dot segment WITH its
 * dot and calls a trailing-dot name (`file.`) an extension of `"."`. What the
 * command groups has to be what the column shows.
 */

import type { FileEntry } from '../types'
import { getDisplayExtension } from '../views/full-list-utils'

/**
 * What the cursor row says the command will select. Three kinds, mirroring the
 * three things a listing row can be; the `..` row has no target at all
 * (`sameKindTargetFor` returns `null`, and the command no-ops).
 */
export type SameKindTarget =
  /** The cursor is on a folder: every folder, whatever its name (`name.like.this` is still just a folder). */
  | { kind: 'allFolders' }
  /** The cursor is on a file with an extension: every file with it, case-insensitively. `extension` is lowercased and carries no dot. */
  | { kind: 'sameExtension'; extension: string }
  /** The cursor is on an extension-less file (`README`, `.gitignore`, `file.`): every other one. */
  | { kind: 'noExtension' }

/** The synthetic parent row, which is a navigation affordance rather than a kind. */
function isParentRow(entry: FileEntry): boolean {
  return entry.name === '..'
}

/** One row's extension as the Ext column shows it, lowercased. Empty for a folder. */
function displayExtensionOf(entry: FileEntry): string {
  return getDisplayExtension(entry.name, entry.isDirectory).toLowerCase()
}

/**
 * The target `entry` implies, or `null` when there's nothing to act on: an empty
 * listing, a cursor entry that hasn't resolved yet, or the `..` row.
 */
export function sameKindTargetFor(entry: FileEntry | null | undefined): SameKindTarget | null {
  if (!entry || isParentRow(entry)) return null
  if (entry.isDirectory) return { kind: 'allFolders' }
  const extension = displayExtensionOf(entry)
  return extension ? { kind: 'sameExtension', extension } : { kind: 'noExtension' }
}

/** Whether one row is of the kind `target` describes. Folders and files never mix. */
function matches(target: SameKindTarget, entry: FileEntry): boolean {
  if (isParentRow(entry)) return false
  if (target.kind === 'allFolders') return entry.isDirectory
  if (entry.isDirectory) return false
  const extension = displayExtensionOf(entry)
  return target.kind === 'noExtension' ? extension === '' : extension === target.extension
}

/**
 * The rows to add to the selection, as indices into `entries`.
 *
 * `entries` is the pane's whole-listing snapshot (`FilePane.getEntriesSnapshot()`),
 * so the returned indices are FRONTEND indices already — the synthetic `..` sits
 * at index 0 when the pane has a parent, and nothing needs offsetting. ❌ Don't
 * feed this the rendered virtual window's cache: it knows nothing outside the
 * rendered range, so every off-screen match would silently vanish.
 */
export function sameKindIndices(target: SameKindTarget, entries: readonly FileEntry[]): number[] {
  const indices: number[] = []
  for (const [index, entry] of entries.entries()) {
    if (matches(target, entry)) indices.push(index)
  }
  return indices
}
