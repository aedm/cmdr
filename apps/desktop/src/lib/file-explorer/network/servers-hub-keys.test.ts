import { describe, it, expect } from 'vitest'
import { cursorAcrossRebuild } from './servers-hub-keys'

describe('cursorAcrossRebuild', () => {
  const rows = (...ids: string[]) => ids.map((id) => ({ id }))

  it('follows the row it was on when the list re-sorts', () => {
    expect(cursorAcrossRebuild(rows('a', 'b', 'c'), rows('x', 'a', 'b', 'c'), 1)).toBe(2)
  })

  it('keeps the add row the add row', () => {
    expect(cursorAcrossRebuild(rows('a', 'b'), rows('a', 'b', 'c'), 2)).toBe(3)
  })

  it('lands on the first row when the list fills, not on the add row an empty list had', () => {
    expect(cursorAcrossRebuild(rows(), rows('a', 'b'), 0)).toBe(0)
  })

  it('clamps when its row left', () => {
    expect(cursorAcrossRebuild(rows('a', 'b', 'c'), rows('a'), 2)).toBe(1)
  })
})
