/**
 * What each mode's logger configuration delivers to the two sinks.
 *
 * The bridge sink is what reaches the log file and every error-report bundle, so
 * the counts here are the contract: a line a bundle needs must arrive, and it must
 * arrive ONCE. A doubled line reads as two failures at triage time.
 *
 * LogTape is configured with the real `buildLoggerConfig`, spies standing in for
 * the console and bridge sinks, and records go through the real `getAppLogger`.
 */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { configure, reset, type LogRecord } from '@logtape/logtape'
import { buildLoggerConfig, debugCategories, getAppLogger } from './logger'

/** A category in `debugCategories`, and one that isn't. */
const DEBUG_CATEGORY = debugCategories[0]
const ORDINARY_CATEGORY = 'viewer'

async function configureFor(mode: { isDev: boolean; verbose: boolean }) {
  const bridgeSink = vi.fn<(record: LogRecord) => void>()
  const consoleSink = vi.fn<(record: LogRecord) => void>()
  await configure({ ...buildLoggerConfig({ ...mode, consoleSink, bridgeSink }), reset: true })
  /** How many records carrying `message` a sink received. */
  const received = (sink: typeof bridgeSink, message: string): number =>
    sink.mock.calls.filter(([record]) => record.rawMessage === message).length
  return {
    bridged: (message: string) => received(bridgeSink, message),
    consoled: (message: string) => received(consoleSink, message),
  }
}

afterEach(async () => {
  await reset()
})

describe('the production config', () => {
  const PROD = { isDev: false, verbose: false }

  it('has a debug category and an ordinary one to compare', () => {
    expect(debugCategories).toContain(DEBUG_CATEGORY)
    expect(debugCategories).not.toContain(ORDINARY_CATEGORY)
  })

  it('sends a warn from a debug category to the bridge exactly once', async () => {
    const { bridged } = await configureFor(PROD)
    getAppLogger(DEBUG_CATEGORY).warn('debug-category warn')
    expect(bridged('debug-category warn')).toBe(1)
  })

  it('sends a warn from an ordinary category to the bridge exactly once', async () => {
    const { bridged } = await configureFor(PROD)
    getAppLogger(ORDINARY_CATEGORY).warn('ordinary warn')
    expect(bridged('ordinary warn')).toBe(1)
  })

  it('sends an error from either kind of category to the bridge and the console exactly once', async () => {
    const { bridged, consoled } = await configureFor(PROD)
    getAppLogger(DEBUG_CATEGORY).error('debug-category error')
    getAppLogger(ORDINARY_CATEGORY).error('ordinary error')
    expect(bridged('debug-category error')).toBe(1)
    expect(bridged('ordinary error')).toBe(1)
    expect(consoled('debug-category error')).toBe(1)
    expect(consoled('ordinary error')).toBe(1)
  })

  it('keeps info and debug from an ordinary category off the bridge', async () => {
    const { bridged } = await configureFor(PROD)
    getAppLogger(ORDINARY_CATEGORY).info('ordinary info')
    getAppLogger(ORDINARY_CATEGORY).debug('ordinary debug')
    expect(bridged('ordinary info')).toBe(0)
    expect(bridged('ordinary debug')).toBe(0)
  })

  it('still sends debug from a debug category to the bridge, once', async () => {
    const { bridged } = await configureFor(PROD)
    getAppLogger(DEBUG_CATEGORY).debug('debug-category debug')
    expect(bridged('debug-category debug')).toBe(1)
  })

  it('keeps warns out of the console, which nobody reads in a shipped app', async () => {
    const { consoled } = await configureFor(PROD)
    getAppLogger(ORDINARY_CATEGORY).warn('ordinary warn')
    expect(consoled('ordinary warn')).toBe(0)
  })
})

describe('the dev config', () => {
  it('sends every line from a debug category to the bridge once', async () => {
    const { bridged } = await configureFor({ isDev: true, verbose: false })
    getAppLogger(DEBUG_CATEGORY).debug('dev debug')
    getAppLogger(DEBUG_CATEGORY).warn('dev warn')
    expect(bridged('dev debug')).toBe(1)
    expect(bridged('dev warn')).toBe(1)
  })

  it('sends debug from an ordinary category to the bridge once, for RUST_LOG to filter', async () => {
    const { bridged } = await configureFor({ isDev: true, verbose: false })
    getAppLogger(ORDINARY_CATEGORY).debug('dev ordinary debug')
    expect(bridged('dev ordinary debug')).toBe(1)
  })
})

describe('the verbose config', () => {
  it('sends debug from every category to the bridge once, in a production build too', async () => {
    const { bridged } = await configureFor({ isDev: false, verbose: true })
    getAppLogger(DEBUG_CATEGORY).debug('verbose debug-category debug')
    getAppLogger(ORDINARY_CATEGORY).debug('verbose ordinary debug')
    expect(bridged('verbose debug-category debug')).toBe(1)
    expect(bridged('verbose ordinary debug')).toBe(1)
  })
})
