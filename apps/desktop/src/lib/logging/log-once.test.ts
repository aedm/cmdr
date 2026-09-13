import { describe, expect, it } from 'vitest'
import { LogOnceGate, MAX_TRACKED_CONDITIONS } from './log-once'

describe('LogOnceGate', () => {
  it('logs the first occurrence of a condition and stays quiet while it repeats', () => {
    const gate = new LogOnceGate()
    expect(gate.shouldLog('store unreadable')).toBe(true)
    expect(gate.shouldLog('store unreadable')).toBe(false)
    expect(gate.shouldLog('store unreadable')).toBe(false)
  })

  it('treats the argument-less call as one condition of its own', () => {
    const gate = new LogOnceGate()
    expect(gate.shouldLog()).toBe(true)
    expect(gate.shouldLog()).toBe(false)
  })

  it('logs a different condition even while another one is being held back', () => {
    const gate = new LogOnceGate()
    expect(gate.shouldLog('timed out')).toBe(true)
    expect(gate.shouldLog('permission denied')).toBe(true)
    expect(gate.shouldLog('timed out')).toBe(false)
  })

  it('logs every condition again once they clear', () => {
    const gate = new LogOnceGate()
    gate.shouldLog('a')
    gate.shouldLog('b')
    gate.clear()
    expect(gate.shouldLog('a')).toBe(true)
    expect(gate.shouldLog('b')).toBe(true)
  })

  it('re-arms only the named condition when one clears', () => {
    const gate = new LogOnceGate()
    gate.shouldLog('vol-a')
    gate.shouldLog('vol-b')
    gate.clear('vol-a')
    expect(gate.shouldLog('vol-a')).toBe(true)
    expect(gate.shouldLog('vol-b')).toBe(false)
  })

  it('forgets what it held back rather than growing past its bound', () => {
    const gate = new LogOnceGate()
    for (let i = 0; i < MAX_TRACKED_CONDITIONS; i++) gate.shouldLog(`path ${String(i)}`)
    expect(gate.shouldLog('path 0')).toBe(false)
    // One more distinct condition tips it over: the set starts again, so an old
    // condition logs one more time instead of the gate holding every key forever.
    expect(gate.shouldLog('one too many')).toBe(true)
    expect(gate.shouldLog('path 0')).toBe(true)
  })
})
