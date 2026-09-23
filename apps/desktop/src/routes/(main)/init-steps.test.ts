/**
 * The main window's startup runs as a list of independent steps. One step that throws must
 * cost only itself: a leftover legacy API key plus a refusing Keychain once threw out of the
 * key migration and skipped every step after it (the AI config push, shortcuts, the MCP
 * bridges, the update checker, AI state) on every launch.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest'
import layoutSource from './+layout.svelte?raw'

const { logError } = vi.hoisted(() => ({ logError: vi.fn() }))
vi.mock('$lib/logging/logger', () => ({
  getAppLogger: () => ({ warn: vi.fn(), info: vi.fn(), debug: vi.fn(), error: logError }),
}))

import { runInitSteps } from './init-steps'

beforeEach(() => {
  logError.mockClear()
})

describe('runInitSteps', () => {
  it('runs every step in order, each one finished before the next starts', async () => {
    const order: string[] = []
    let releaseFirst: () => void = () => undefined
    const first = new Promise<void>((resolve) => {
      releaseFirst = resolve
    })

    const done = runInitSteps([
      {
        name: 'first',
        run: async () => {
          order.push('first:start')
          await first
          order.push('first:end')
        },
      },
      { name: 'second', run: () => order.push('second') },
    ])

    await Promise.resolve()
    expect(order).toEqual(['first:start'])
    releaseFirst()
    await done
    expect(order).toEqual(['first:start', 'first:end', 'second'])
  })

  it('a step that rejects costs only itself: the steps after it still run', async () => {
    const ran: string[] = []

    await runInitSteps([
      { name: 'before', run: () => ran.push('before') },
      { name: 'rejects', run: () => Promise.reject(new Error('keychain refused')) },
      { name: 'after', run: () => ran.push('after') },
    ])

    expect(ran).toEqual(['before', 'after'])
  })

  it('a step that throws synchronously costs only itself too', async () => {
    const ran: string[] = []

    await runInitSteps([
      {
        name: 'throws',
        run: () => {
          throw new Error('bridge missing')
        },
      },
      { name: 'after', run: () => ran.push('after') },
    ])

    expect(ran).toEqual(['after'])
  })

  it('logs the failing step by name, with its error', async () => {
    const error = new Error('keychain refused')

    await runInitSteps([{ name: 'aiConfig', run: () => Promise.reject(error) }])

    expect(logError).toHaveBeenCalledOnce()
    expect(logError.mock.calls[0][1]).toEqual({ step: 'aiConfig', error })
  })
})

describe("the main window's startup steps", () => {
  it('retry a "no" to cloud AI the store refused, on every launch until it lands', () => {
    // The retry itself is pinned in `cloud-consent.svelte.test.ts`; this pins that launch runs
    // it. Without the step, a held "no" would only settle once a cloud gate refreshed.
    expect(layoutSource).toContain('run: settleHeldCloudConsentRevoke')
  })

  it('map the old Ask Cmdr opt-in onto the on/off switch, after settings load', () => {
    // The mapping is pinned in `ask-cmdr-enabled-mapping.test.ts`; this pins that launch runs
    // it, and after the settings step it reads from.
    expect(layoutSource).toContain('run: mapLegacyAskCmdrOptIn')
    expect(layoutSource.indexOf('run: mapLegacyAskCmdrOptIn')).toBeGreaterThan(layoutSource.indexOf("name: 'settings'"))
  })
})
