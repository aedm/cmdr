import { describe, it, expect } from 'vitest'
import { cursorAcrossRebuild, cursorAfterArrow } from './servers-hub-keys'

describe('cursorAfterArrow', () => {
  it('steps one row, clamped to the list', () => {
    expect(cursorAfterArrow('ArrowDown', 2, 5)).toBe(3)
    expect(cursorAfterArrow('ArrowDown', 4, 5)).toBe(4)
    expect(cursorAfterArrow('ArrowUp', 0, 5)).toBe(0)
  })

  it('jumps to the first and the last row on left and right', () => {
    expect(cursorAfterArrow('ArrowLeft', 3, 5)).toBe(0)
    expect(cursorAfterArrow('ArrowRight', 1, 5)).toBe(4)
  })

  it('leaves every other key alone', () => {
    expect(cursorAfterArrow('Enter', 1, 5)).toBeNull()
  })
})

describe('cursorAcrossRebuild', () => {
  const rows = (...ids: string[]) => ids.map((id) => ({ id }))

  it('follows the row it was on when the list re-sorts', () => {
    expect(cursorAcrossRebuild(rows('a', 'b', 'c'), rows('x', 'a', 'b', 'c'), 1)).toBe(2)
  })

  it('keeps the add row the add row', () => {
    expect(cursorAcrossRebuild(rows('a', 'b'), rows('a', 'b', 'c'), 2)).toBe(3)
  })

  it('clamps when its row left', () => {
    expect(cursorAcrossRebuild(rows('a', 'b', 'c'), rows('a'), 2)).toBe(1)
  })
})
