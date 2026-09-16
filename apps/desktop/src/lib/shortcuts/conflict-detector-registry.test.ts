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
 * The one conflict the defaults DO carry, and only off macOS.
 *
 * ❗ It isn't a binding mistake: `key-capture.ts::toPlatformShortcut` maps BOTH `⌘` and
 * `⌃` onto `Ctrl` for non-mac platforms, so the two distinct macOS combos `⌘D`
 * (`file.duplicate`, and the error screen's deliberate shadow of it) and `⌃D`
 * (`favorites.open`, which is what Total Commander and Double Commander bind for the same
 * list) arrive here as one string. On macOS — the platform Cmdr ships on — they stay
 * distinct and this list is never consulted, which is why the test runs the check for real
 * and then subtracts exactly this pair rather than skipping.
 *
 * What it costs on Linux: `shortcut-dispatch` keeps one winner per combo, most specific
 * scope first, so `file.duplicate` (`Main window/File list`) beats `favorites.open`
 * (`Main window`) and Ctrl+D duplicates instead of opening the menu. The native menu bar is
 * unaffected — muda maps `Cmd+` to Super there, so its Duplicate is Super+D.
 *
 * ❌ Don't grow this list to quiet a new conflict. A second entry means the ⌘/⌃ collapse
 * has become a pattern, and the fix then is to make the Linux mapping injective (a lone
 * `⌃` → Super, the way the function already remaps `⌃` → Shift when both appear together),
 * not another line here.
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
