import { describe, it, expect } from 'vitest'
import { anchoredTopFor } from './modal-anchor'

describe('anchoredTopFor', () => {
  it('stays where it opened while the dialog fits', () => {
    expect(anchoredTopFor(200, 900, 400)).toBe(200)
  })

  it('pulls up only as far as keeps the bottom on screen', () => {
    expect(anchoredTopFor(200, 900, 800)).toBe(100)
  })

  it('never goes above the top, even for a dialog taller than the window', () => {
    expect(anchoredTopFor(200, 900, 1200)).toBe(0)
  })

  it('returns to where it opened once it shrinks back', () => {
    const grown = anchoredTopFor(200, 900, 1200)
    expect(grown).toBe(0)
    expect(anchoredTopFor(200, 900, 400)).toBe(200)
  })
})
