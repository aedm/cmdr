import { afterEach, describe, expect, it, vi, type Mock } from 'vitest'
import { app } from '../index'

function createMockKv(): KVNamespace {
  return {
    get: vi.fn(() => null),
    put: vi.fn(),
  } as unknown as KVNamespace
}

/** A statement as the mock D1 records it: the SQL it was prepared with and the values bound to it. */
interface RecordedStatement {
  sql: string
  args: unknown[]
}

/**
 * Mock D1Database that records prepare/bind calls and the batch they're committed in. The route
 * commits a beat through ONE `batch`, so `batchMock` rejecting is what a D1 outage looks like.
 */
function createMockD1(batchImpl?: () => Promise<unknown>): {
  db: D1Database
  prepareMock: Mock
  bindMock: Mock
  batchMock: Mock
  /** Every statement of every batch, in order. */
  batched: () => RecordedStatement[]
} {
  const bindMock = vi.fn()
  const prepareMock = vi.fn((sql: string) => ({
    bind: (...args: unknown[]) => {
      bindMock(...args)
      return { sql, args }
    },
  }))
  const batchMock: Mock<(statements: RecordedStatement[]) => Promise<unknown>> = vi.fn(
    batchImpl ?? (() => Promise.resolve([])),
  )
  const batched = () => batchMock.mock.calls.flatMap((call) => call[0])
  return {
    db: { prepare: prepareMock, batch: batchMock } as unknown as D1Database,
    prepareMock,
    bindMock,
    batchMock,
    batched,
  }
}

function createMockAnalyticsEngine(): AnalyticsEngineDataset {
  return { writeDataPoint: vi.fn() }
}

/** Mock the Workers rate-limit binding. Defaults to allowing every request. */
function createMockRateLimiter(success = true): { limiter: RateLimit; limitMock: Mock } {
  const limitMock = vi.fn(() => Promise.resolve({ success }))
  return { limiter: { limit: limitMock }, limitMock }
}

function createBindings(overrides: Record<string, unknown> = {}) {
  return {
    LICENSE_CODES: createMockKv(),
    DEVICE_COUNTS: createMockAnalyticsEngine(),
    TELEMETRY_DB: createMockD1().db,
    HEARTBEAT_LIMITER: createMockRateLimiter().limiter,
    ED25519_PRIVATE_KEY: 'deadbeef'.repeat(8),
    RESEND_API_KEY: 'test-resend-key',
    PRODUCT_NAME: 'Cmdr',
    SUPPORT_EMAIL: 'test@example.com',
    ADMIN_API_TOKEN: 'test-admin-token-secret',
    ...overrides,
  }
}

const validBeat = {
  analId: 'anal_0123456789abcdef0123456789abcdef0123',
  appVersion: '1.2.3',
  osVersion: '15.3.1',
  arch: 'aarch64',
}

function postHeartbeat(body: unknown, bindings: Record<string, unknown>) {
  return app.request(
    '/heartbeat',
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    },
    bindings,
  )
}

describe('POST /heartbeat', () => {
  it('returns 204 for a valid beat', async () => {
    const bindings = createBindings()
    const res = await postHeartbeat(validBeat, bindings)
    expect(res.status).toBe(204)
  })

  it('inserts correct data into D1 (no IP column)', async () => {
    const { db, prepareMock, bindMock } = createMockD1()
    const bindings = createBindings({ TELEMETRY_DB: db })

    await postHeartbeat(validBeat, bindings)

    expect(prepareMock).toHaveBeenCalledOnce()
    const sql = prepareMock.mock.calls[0][0] as string
    expect(sql).toContain('INSERT INTO heartbeat')
    expect(sql).not.toContain('hashed_ip')
    expect(sql).not.toContain('ip')

    const bindArgs = bindMock.mock.calls[0]
    // bindArgs: [anal_id, app_version, os_version, arch, build_mode, config_json]
    expect(bindArgs[0]).toBe('anal_0123456789abcdef0123456789abcdef0123')
    expect(bindArgs[1]).toBe('1.2.3')
    expect(bindArgs[2]).toBe('15.3.1')
    expect(bindArgs[3]).toBe('aarch64')
    expect(bindArgs[4]).toBeNull() // buildMode not supplied
    expect(bindArgs[5]).toBeNull() // config not supplied
    expect(bindArgs[6]).toBeNull() // uptimeSeconds not supplied
  })

  it('round-trips the config blob verbatim as config_json', async () => {
    const { db, bindMock } = createMockD1()
    const bindings = createBindings({ TELEMETRY_DB: db })

    const config = { theme: 'dark', viewMode: 'full', fdaGranted: true, tabCount: 3 }
    await postHeartbeat({ ...validBeat, buildMode: 'release', config }, bindings)

    const bindArgs = bindMock.mock.calls[0]
    expect(bindArgs[4]).toBe('release')
    expect(JSON.parse(bindArgs[5] as string)).toEqual(config)
  })

  it('returns 204 when optional fields are omitted', async () => {
    const bindings = createBindings()
    const res = await postHeartbeat(validBeat, bindings)
    expect(res.status).toBe(204)
  })

  it('accepts an explicit null buildMode (upgrade-window compat)', async () => {
    const { db, bindMock } = createMockD1()
    const bindings = createBindings({ TELEMETRY_DB: db })

    const res = await postHeartbeat({ ...validBeat, buildMode: null, config: null }, bindings)
    expect(res.status).toBe(204)

    const bindArgs = bindMock.mock.calls[0]
    expect(bindArgs[4]).toBeNull()
    expect(bindArgs[5]).toBeNull()
  })

  it('returns 400 when analId is missing', async () => {
    const bindings = createBindings()
    const { analId, ...withoutId } = validBeat

    const res = await postHeartbeat(withoutId, bindings)
    expect(res.status).toBe(400)
    const body = await res.json<{ error: string }>()
    expect(body.error).toBe('Missing required field: analId')
  })

  it('returns 400 for a malformed analId', async () => {
    const bindings = createBindings()
    const res = await postHeartbeat({ ...validBeat, analId: 'anal_too-short' }, bindings)
    expect(res.status).toBe(400)
    const body = await res.json<{ error: string }>()
    expect(body.error).toBe('Invalid analId')
  })

  it('returns 400 for an analId without the anal_ prefix', async () => {
    const bindings = createBindings()
    const res = await postHeartbeat({ ...validBeat, analId: 'diag_0123456789abcdef0123456789abcdef0123' }, bindings)
    expect(res.status).toBe(400)
    const body = await res.json<{ error: string }>()
    expect(body.error).toBe('Invalid analId')
  })

  it('returns 400 for a malformed appVersion', async () => {
    const bindings = createBindings()
    const res = await postHeartbeat({ ...validBeat, appVersion: 'not-a-version' }, bindings)
    expect(res.status).toBe(400)
    const body = await res.json<{ error: string }>()
    expect(body.error).toBe('Invalid appVersion')
  })

  it('returns 400 when osVersion is missing', async () => {
    const bindings = createBindings()
    const { osVersion, ...rest } = validBeat

    const res = await postHeartbeat(rest, bindings)
    expect(res.status).toBe(400)
    const body = await res.json<{ error: string }>()
    expect(body.error).toBe('Missing required field: osVersion')
  })

  it('returns 400 for an invalid buildMode', async () => {
    const bindings = createBindings()
    const res = await postHeartbeat({ ...validBeat, buildMode: 'staging' }, bindings)
    expect(res.status).toBe(400)
    const body = await res.json<{ error: string }>()
    expect(body.error).toBe('Invalid buildMode')
  })

  it('returns 400 for an oversized body', async () => {
    const bindings = createBindings()
    // Each event stays valid on its own; together they blow the 256 KB request cap.
    const events = Array.from({ length: 300 }, () => ({
      event: 'search_used',
      timestamp: '2026-09-24T10:00:00Z',
      properties: { padding: 'x'.repeat(1_000) },
    }))
    const oversized = { ...validBeat, events }

    const res = await postHeartbeat(oversized, bindings)
    expect(res.status).toBe(400)
    const body = await res.json<{ error: string }>()
    expect(body.error).toBe('Heartbeat too large')
  })

  it('returns 400 for an oversized config blob', async () => {
    const bindings = createBindings()
    // Config blob exceeds its own 16 KB cap while the whole body stays under the 256 KB request cap.
    const config = { note: 'x'.repeat(20_000) }
    const res = await postHeartbeat({ ...validBeat, config }, bindings)
    expect(res.status).toBe(400)
    const body = await res.json<{ error: string }>()
    expect(body.error).toBe('Config too large')
  })

  it('returns 400 for malformed JSON', async () => {
    const bindings = createBindings()
    const res = await app.request(
      '/heartbeat',
      { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: 'not json {{{' },
      bindings,
    )
    expect(res.status).toBe(400)
    const body = await res.json<{ error: string }>()
    expect(body.error).toBe('Invalid JSON')
  })

  it('returns 502 when the D1 write fails, so the client keeps its events and retries', async () => {
    const { db } = createMockD1(() => Promise.reject(new Error('D1 unavailable')))
    const bindings = createBindings({ TELEMETRY_DB: db })

    const res = await postHeartbeat(validBeat, bindings)
    expect(res.status).toBe(502)
  })

  it('returns 429 when over the rate limit', async () => {
    const { limiter } = createMockRateLimiter(false)
    const bindings = createBindings({ HEARTBEAT_LIMITER: limiter })

    const res = await postHeartbeat(validBeat, bindings)
    expect(res.status).toBe(429)
  })

  it('keys the rate limiter by the caller IP', async () => {
    const { limiter, limitMock } = createMockRateLimiter(true)
    const bindings = createBindings({ HEARTBEAT_LIMITER: limiter })

    await app.request(
      '/heartbeat',
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'cf-connecting-ip': '203.0.113.7' },
        body: JSON.stringify(validBeat),
      },
      bindings,
    )

    expect(limitMock).toHaveBeenCalledWith({ key: '203.0.113.7' })
  })

  it('does not touch D1 when rate-limited', async () => {
    const { db, prepareMock } = createMockD1()
    const { limiter } = createMockRateLimiter(false)
    const bindings = createBindings({ TELEMETRY_DB: db, HEARTBEAT_LIMITER: limiter })

    await postHeartbeat(validBeat, bindings)
    expect(prepareMock).not.toHaveBeenCalled()
  })
})

/** The heartbeat row's bind args, from the first statement of the first batch. */
function heartbeatArgs(batched: () => RecordedStatement[]): unknown[] {
  const [first] = batched()
  expect(first.sql).toContain('INSERT INTO heartbeat')
  return first.args
}

/** The events the route handed D1, decoded from the one JSON parameter they travel in. */
function storedEvents(
  batched: () => RecordedStatement[],
): { event: string; occurredAt: string; properties: unknown }[] {
  const statement = batched().find((s) => s.sql.includes('INSERT INTO analytics_event'))
  if (!statement) return []
  const rows = JSON.parse(statement.args[2] as string) as [string, string, string][]
  return rows.map(([event, occurredAt, properties]) => ({
    event,
    occurredAt,
    properties: JSON.parse(properties) as unknown,
  }))
}

function makeEvent(overrides: Record<string, unknown> = {}) {
  return { event: 'search_used', timestamp: '2026-09-24T10:00:00Z', properties: { mode: 'ai' }, ...overrides }
}

describe('POST /heartbeat: uptimeSeconds', () => {
  it('stores the uptime the beat accounts for', async () => {
    const { db, batched } = createMockD1()
    const res = await postHeartbeat({ ...validBeat, uptimeSeconds: 5_400 }, createBindings({ TELEMETRY_DB: db }))
    expect(res.status).toBe(204)
    expect(heartbeatArgs(batched)[6]).toBe(5_400)
  })

  it('accepts zero', async () => {
    const { db, batched } = createMockD1()
    const res = await postHeartbeat({ ...validBeat, uptimeSeconds: 0 }, createBindings({ TELEMETRY_DB: db }))
    expect(res.status).toBe(204)
    expect(heartbeatArgs(batched)[6]).toBe(0)
  })

  it('stores NULL for an explicit null (upgrade-window compat)', async () => {
    const { db, batched } = createMockD1()
    const res = await postHeartbeat({ ...validBeat, uptimeSeconds: null }, createBindings({ TELEMETRY_DB: db }))
    expect(res.status).toBe(204)
    expect(heartbeatArgs(batched)[6]).toBeNull()
  })

  it('clamps an implausibly long uptime to seven days', async () => {
    const { db, batched } = createMockD1()
    const res = await postHeartbeat({ ...validBeat, uptimeSeconds: 90 * 86_400 }, createBindings({ TELEMETRY_DB: db }))
    expect(res.status).toBe(204)
    expect(heartbeatArgs(batched)[6]).toBe(7 * 86_400)
  })

  it.each([
    ['a negative number', -1],
    ['a fraction', 1.5],
    ['a string', '3600'],
    ['NaN-ish', 'NaN'],
  ])('returns 400 for %s', async (_label, uptimeSeconds) => {
    const res = await postHeartbeat({ ...validBeat, uptimeSeconds }, createBindings())
    expect(res.status).toBe(400)
    const body = await res.json<{ error: string }>()
    expect(body.error).toBe('Invalid uptimeSeconds')
  })
})

describe('POST /heartbeat: events', () => {
  it('stores the heartbeat and its events in ONE batch, so a 2xx means both are in D1', async () => {
    const { db, batchMock, batched } = createMockD1()
    const res = await postHeartbeat(
      { ...validBeat, events: [makeEvent(), makeEvent({ event: 'app_launched', properties: {} })] },
      createBindings({ TELEMETRY_DB: db }),
    )
    expect(res.status).toBe(204)
    expect(batchMock).toHaveBeenCalledOnce()
    expect(batched().map((s) => s.sql.includes('INSERT INTO heartbeat'))).toEqual([true, false])

    const eventStatement = batched()[1]
    expect(eventStatement.sql).toContain('INSERT INTO analytics_event')
    expect(eventStatement.sql).not.toContain('ip')
    expect(eventStatement.args[0]).toBe(validBeat.analId)
    expect(eventStatement.args[1]).toBe(validBeat.appVersion)
    expect(storedEvents(batched)).toEqual([
      { event: 'search_used', occurredAt: '2026-09-24T10:00:00.000Z', properties: { mode: 'ai' } },
      { event: 'app_launched', occurredAt: '2026-09-24T10:00:00.000Z', properties: {} },
    ])
  })

  it('writes only the heartbeat for an old client that sends no events', async () => {
    const { db, batched } = createMockD1()
    const res = await postHeartbeat(validBeat, createBindings({ TELEMETRY_DB: db }))
    expect(res.status).toBe(204)
    expect(batched()).toHaveLength(1)
  })

  it('writes only the heartbeat for an empty events array', async () => {
    const { db, batched } = createMockD1()
    const res = await postHeartbeat({ ...validBeat, events: [] }, createBindings({ TELEMETRY_DB: db }))
    expect(res.status).toBe(204)
    expect(batched()).toHaveLength(1)
  })

  it('accepts null events (upgrade-window compat)', async () => {
    const { db, batched } = createMockD1()
    const res = await postHeartbeat({ ...validBeat, events: null }, createBindings({ TELEMETRY_DB: db }))
    expect(res.status).toBe(204)
    expect(batched()).toHaveLength(1)
  })

  it('returns 400 when events is not an array', async () => {
    const res = await postHeartbeat({ ...validBeat, events: { event: 'x' } }, createBindings())
    expect(res.status).toBe(400)
    const body = await res.json<{ error: string }>()
    expect(body.error).toBe('Invalid events')
  })

  it('keeps the first 500 events and drops the rest', async () => {
    const { db, batched } = createMockD1()
    const events = Array.from({ length: 520 }, (_, i) => makeEvent({ properties: { i } }))
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const res = await postHeartbeat({ ...validBeat, events }, createBindings({ TELEMETRY_DB: db }))
    expect(res.status).toBe(204)

    const stored = storedEvents(batched)
    expect(stored).toHaveLength(500)
    expect(stored[0].properties).toEqual({ i: 0 })
    expect(stored[499].properties).toEqual({ i: 499 })
    expect(warn.mock.calls.some((call) => String(call[0]).includes('20'))).toBe(true)
    warn.mockRestore()
  })

  it.each([
    ['an uppercase name', { event: 'SearchUsed' }],
    ['an empty name', { event: '' }],
    ['a name over 100 chars', { event: 'a'.repeat(101) }],
    ['a non-string name', { event: 42 }],
    ['a missing timestamp', { timestamp: undefined }],
    ['an unparseable timestamp', { timestamp: 'yesterday' }],
    ['a date-only timestamp', { timestamp: '2026-09-24' }],
    ['a timestamp far in the future', { timestamp: '2099-01-01T00:00:00Z' }],
    ['array properties', { properties: [1, 2] }],
    ['string properties', { properties: 'mode=ai' }],
  ])('drops one event with %s and still stores the beat and the rest', async (_label, overrides) => {
    const { db, batched } = createMockD1()
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const res = await postHeartbeat(
      { ...validBeat, events: [makeEvent(overrides), makeEvent({ event: 'app_launched' })] },
      createBindings({ TELEMETRY_DB: db }),
    )
    expect(res.status).toBe(204)
    expect(storedEvents(batched).map((e) => e.event)).toEqual(['app_launched'])
    warn.mockRestore()
  })

  it('accepts the names the app sends, `$` included', async () => {
    const { db, batched } = createMockD1()
    const res = await postHeartbeat(
      { ...validBeat, events: [makeEvent({ event: 'suggestion_group_approved' }), makeEvent({ event: '$pageview' })] },
      createBindings({ TELEMETRY_DB: db }),
    )
    expect(res.status).toBe(204)
    expect(storedEvents(batched).map((e) => e.event)).toEqual(['suggestion_group_approved', '$pageview'])
  })

  it('normalizes an offset timestamp to UTC', async () => {
    const { db, batched } = createMockD1()
    await postHeartbeat(
      { ...validBeat, events: [makeEvent({ timestamp: '2026-09-24T12:00:00.5+02:00' })] },
      createBindings({ TELEMETRY_DB: db }),
    )
    expect(storedEvents(batched)[0].occurredAt).toBe('2026-09-24T10:00:00.500Z')
  })

  it('treats absent properties as an empty object', async () => {
    const { db, batched } = createMockD1()
    await postHeartbeat(
      { ...validBeat, events: [{ event: 'app_launched', timestamp: '2026-09-24T10:00:00Z' }] },
      createBindings({ TELEMETRY_DB: db }),
    )
    expect(storedEvents(batched)[0].properties).toEqual({})
  })
})

describe('POST /heartbeat: the PostHog forward', () => {
  const posthogKey = 'phc_test'

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('forwards the stored events to PostHog', async () => {
    const fetchMock = vi.fn(() => Promise.resolve(new Response('{"status":"Ok"}', { status: 200 })))
    vi.stubGlobal('fetch', fetchMock)

    const res = await postHeartbeat(
      { ...validBeat, events: [makeEvent()] },
      createBindings({ POSTHOG_PROJECT_KEY: posthogKey }),
    )
    expect(res.status).toBe(204)
    expect(fetchMock).toHaveBeenCalledOnce()
    const [url] = fetchMock.mock.calls[0] as unknown as [string]
    expect(url).toBe('https://eu.i.posthog.com/batch/')
  })

  it('makes no call without the secret', async () => {
    const fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)

    const res = await postHeartbeat({ ...validBeat, events: [makeEvent()] }, createBindings())
    expect(res.status).toBe(204)
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it('makes no call for a beat without events', async () => {
    const fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)

    const res = await postHeartbeat(validBeat, createBindings({ POSTHOG_PROJECT_KEY: posthogKey }))
    expect(res.status).toBe(204)
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it('still answers 204 when PostHog answers 500', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.resolve(new Response('boom', { status: 500 }))),
    )
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})

    const res = await postHeartbeat(
      { ...validBeat, events: [makeEvent()] },
      createBindings({ POSTHOG_PROJECT_KEY: posthogKey }),
    )
    expect(res.status).toBe(204)
    warn.mockRestore()
  })

  it('still answers 204 when PostHog is unreachable', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.reject(new Error('network down'))),
    )
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})

    const res = await postHeartbeat(
      { ...validBeat, events: [makeEvent()] },
      createBindings({ POSTHOG_PROJECT_KEY: posthogKey }),
    )
    expect(res.status).toBe(204)
    warn.mockRestore()
  })

  it('never forwards events that D1 did not store', async () => {
    const fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)
    const { db } = createMockD1(() => Promise.reject(new Error('D1 unavailable')))
    const error = vi.spyOn(console, 'error').mockImplementation(() => {})

    const res = await postHeartbeat(
      { ...validBeat, events: [makeEvent()] },
      createBindings({ TELEMETRY_DB: db, POSTHOG_PROJECT_KEY: posthogKey }),
    )
    expect(res.status).toBe(502)
    // The client retries this beat, so forwarding now would count every event twice.
    expect(fetchMock).not.toHaveBeenCalled()
    error.mockRestore()
  })
})
