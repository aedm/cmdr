/**
 * Regression test for conflict detection over the REAL command registry with
 * default bindings (no mocks). Guards two things at once:
 *
 * 1. Compound-scope commands (`Main window/File list`, `Main window/Brief mode`,
 *    …) participate in conflict detection. Before the scope hierarchy learned the
 *    compound chains, `getActiveScopes` returned `[]` for these and the 48
 *    compound-scope commands could never conflict with anything.
 * 2. The shipped default bindings don't introduce conflicts. If a future default
 *    edit binds the same combo to two overlapping-scope commands, this test
 *    fails and surfaces exactly which combo.
 */

import { describe, it, expect } from 'vitest'
import { getAllConflicts } from './conflict-detector'

/**
 * The one conflict the defaults DO carry, and only off macOS: `toPlatformShortcut` maps
 * both `⌘` and `⌃` onto `Ctrl`, so `⌘D` (`file.duplicate`) and `⌃D` (`favorites.open`)
 * arrive here as one combo. Cmdr ships on macOS, where they stay distinct, so this is
 * parked rather than solved — hence subtracting exactly this pair from a real run, ❌ never
 * skipping the check. ❌ Don't grow the set to quiet a new conflict: a second entry means
 * the mapping itself needs fixing.
 */
const PLATFORM_COLLAPSED_COMBOS = new Set(['Ctrl+D'])

describe('conflict-detector over the real registry (default bindings)', () => {
  it('reports no conflicts for the shipped defaults', () => {
    const conflicts = getAllConflicts().filter((c) => !PLATFORM_COLLAPSED_COMBOS.has(c.shortcut))
    // Surface the offending combos in the failure message rather than a bare count.
    const summary = conflicts.map((c) => `${c.shortcut}: ${c.commandIds.join(', ')}`)
    expect(summary).toEqual([])
  })
})
