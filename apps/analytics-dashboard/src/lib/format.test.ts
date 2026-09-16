import { describe, it, expect } from 'vitest'
import { formatUtcDateTime } from './format.js'

describe('formatUtcDateTime', () => {
  it('prints an ISO timestamp as a UTC day and time', () => {
    expect(formatUtcDateTime('2026-09-16T10:04:02.000Z')).toBe('2026-09-16 10:04')
  })

  it('stays on UTC for a timestamp written in another offset', () => {
    expect(formatUtcDateTime('2026-09-16T23:30:00+02:00')).toBe('2026-09-16 21:30')
  })

  it('returns an unparseable timestamp verbatim', () => {
    expect(formatUtcDateTime('not a date')).toBe('not a date')
  })
})
