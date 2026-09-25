import { describe, it, expect } from 'vitest'
import { anchoredTopFor } from './modal-anchor'

describe('anchoredTopFor', () => {
  it('stays where it opened while the dialog fits', () => {
    expect(anchoredTopFor(200, 900, 400, 24)).toBe(200)
  })

  /** ❗ The Add sheet with Advanced open at 1080×720 sat flush with the window's bottom (QA round 3). */
  it('pulls up only as far as keeps the bottom a margin inside the window', () => {
    expect(anchoredTopFor(200, 900, 800, 24)).toBe(76)
  })

  it('never goes above the top margin, even for a dialog taller than the window', () => {
    expect(anchoredTopFor(200, 900, 1200, 24)).toBe(24)
  })

  it('returns to where it opened once it shrinks back', () => {
    const grown = anchoredTopFor(200, 900, 1200, 24)
    expect(grown).toBe(24)
    expect(anchoredTopFor(200, 900, 400, 24)).toBe(200)
  })
})
