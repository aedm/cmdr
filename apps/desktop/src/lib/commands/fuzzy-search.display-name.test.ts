/**
 * The palette searches and highlights `displayName`, not `name`.
 *
 * A command whose label says what it will do RIGHT NOW ("Select all with
 * extension .pdf") has to be findable by that text, and its highlight has to
 * land on the characters the row actually renders. Every other surface —
 * Settings > Shortcuts, the help window, the conflict toast, the MCP bridge —
 * keeps reading the static `name`, which is why this lives in its own file with
 * a mocked registry rather than in `fuzzy-search.test.ts` (which runs the real
 * one, where no command overrides `displayName` yet).
 */
import { describe, it, expect, vi } from 'vitest'

// Inlined in the factory: `vi.mock` is hoisted above every top-level binding.
// `displayName` is deliberately LONGER than `name`, so a highlight clamped to
// `name.length` would drop every index the query matched.
vi.mock('./command-registry', () => {
  const mockCommands = [
    {
      id: 'app.about',
      name: 'About Cmdr',
      displayName: 'About Cmdr',
      scope: 'App',
      showInPalette: true,
      shortcuts: [],
    },
    {
      id: 'selection.invert',
      name: 'Select same kind',
      displayName: 'Select all with extension .pdf',
      scope: 'Main window/File list',
      showInPalette: true,
      shortcuts: [],
    },
  ]
  return { commands: mockCommands, getPaletteCommands: () => mockCommands.filter((c) => c.showInPalette) }
})

import { searchCommands } from './fuzzy-search'

describe('palette haystack', () => {
  it('finds a command by text only its displayName carries', () => {
    const results = searchCommands('pdf')
    expect(results.map((r) => r.command.id)).toEqual(['selection.invert'])
  })

  it('highlights indices inside the displayName the row renders, not a name-length window', () => {
    const results = searchCommands('extension')
    const match = results.find((r) => r.command.id === 'selection.invert')
    expect(match).toBeDefined()
    if (!match) return
    expect(match.matchedIndices.length).toBeGreaterThan(0)
    expect(Math.max(...match.matchedIndices)).toBeLessThan(match.command.displayName.length)
  })
})
