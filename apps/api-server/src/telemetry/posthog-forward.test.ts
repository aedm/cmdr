import { afterEach, describe, expect, it, vi } from 'vitest'
import { buildPostHogBatch, forwardEventsToPostHog, type ForwardIdentity } from './posthog-forward'

const identity: ForwardIdentity = {
  analId: 'anal_0123456789abcdef0123456789abcdef0123',
  osVersion: 'macOS 26.0',
  arch: 'aarch64',
}

const events = [
  {
    event: 'search_used',
    timestamp: '2026-09-24T10:00:00.000Z',
    properties: { mode: 'ai' },
    appVersion: '1.2.3',
    id: '3b241101-e2bb-4255-8caf-4136c566a962',
  },
  { event: 'app_launched', timestamp: '2026-09-24T09:00:00.000Z', properties: {}, appVersion: '1.2.2', id: null },
]

describe('buildPostHogBatch', () => {
  it('builds one batch entry per event, keyed on the install id, with its own client timestamp', () => {
    const body = buildPostHogBatch('phc_test', identity, events, null)
    expect(body.api_key).toBe('phc_test')
    expect(body.batch).toHaveLength(2)
    expect(body.batch[0]).toMatchObject({
      event: 'search_used',
      distinct_id: identity.analId,
      timestamp: '2026-09-24T10:00:00.000Z',
    })
    expect(body.batch[1].timestamp).toBe('2026-09-24T09:00:00.000Z')
  })

  it('carries the identity properties the app used to send, plus the event own ones', () => {
    const [entry] = buildPostHogBatch('phc_test', identity, events, null).batch
    expect(entry.properties).toMatchObject({
      source: 'desktop',
      app_version: '1.2.3',
      os_version: 'macOS 26.0',
      arch: 'aarch64',
      mode: 'ai',
    })
  })

  it('tags each event with the version that produced it, not the beat that carried it', () => {
    const { batch } = buildPostHogBatch('phc_test', identity, events, null)
    expect(batch.map((e) => e.properties.app_version)).toEqual(['1.2.3', '1.2.2'])
  })

  it('sends the client event id as `uuid`, which PostHog dedupes on, and none when there is no id', () => {
    const { batch } = buildPostHogBatch('phc_test', identity, events, null)
    expect(batch[0].uuid).toBe('3b241101-e2bb-4255-8caf-4136c566a962')
    expect(batch[1]).not.toHaveProperty('uuid')
  })

  it('turns geo-IP off, since the caller PostHog sees is our Worker', () => {
    const [entry] = buildPostHogBatch('phc_test', identity, events, null).batch
    expect(entry.properties.$geoip_disable).toBe(true)
  })

  it('lets no event property shadow the identity or the geo-IP switch', () => {
    const sneaky = [
      {
        event: 'e',
        timestamp: '2026-09-24T10:00:00.000Z',
        properties: { source: 'sneaky', app_version: '9.9.9', arch: 'sparc', $geoip_disable: false },
        appVersion: '1.2.3',
        id: null,
      },
    ]
    const [entry] = buildPostHogBatch('phc_test', identity, sneaky, null).batch
    expect(entry.properties.source).toBe('desktop')
    expect(entry.properties.app_version).toBe('1.2.3')
    expect(entry.properties.arch).toBe('aarch64')
    expect(entry.properties.$geoip_disable).toBe(true)
  })

  it('mirrors the config snapshot as `$set` person properties', () => {
    const config = { 'theme.mode': 'dark', fdaGranted: true }
    const [entry] = buildPostHogBatch('phc_test', identity, events, config).batch
    expect(entry.properties.$set).toEqual(config)
  })

  it('sends no `$set` when the beat carried no config', () => {
    const [entry] = buildPostHogBatch('phc_test', identity, events, null).batch
    expect(entry.properties).not.toHaveProperty('$set')
  })
})

describe('forwardEventsToPostHog', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
    vi.restoreAllMocks()
  })

  it('POSTs the batch as JSON to the EU batch endpoint', async () => {
    const fetchMock = vi.fn(() => Promise.resolve(new Response('{"status":"Ok"}', { status: 200 })))
    vi.stubGlobal('fetch', fetchMock)

    await forwardEventsToPostHog('phc_test', identity, events, null)

    expect(fetchMock).toHaveBeenCalledOnce()
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(url).toBe('https://eu.i.posthog.com/batch/')
    expect(init.method).toBe('POST')
    expect(new Headers(init.headers).get('Content-Type')).toBe('application/json')
    expect(JSON.parse(init.body as string)).toEqual(buildPostHogBatch('phc_test', identity, events, null))
  })

  it('does nothing without a key', async () => {
    const fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)
    await forwardEventsToPostHog(undefined, identity, events, null)
    await forwardEventsToPostHog('', identity, events, null)
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it('does nothing without events', async () => {
    const fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)
    await forwardEventsToPostHog('phc_test', identity, [], null)
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it('resolves, logging, when PostHog answers an error status', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.resolve(new Response('nope', { status: 500 }))),
    )
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    await expect(forwardEventsToPostHog('phc_test', identity, events, null)).resolves.toBeUndefined()
    expect(warn).toHaveBeenCalledOnce()
  })

  it('resolves, logging, when the request throws', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.reject(new Error('network down'))),
    )
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    await expect(forwardEventsToPostHog('phc_test', identity, events, null)).resolves.toBeUndefined()
    expect(warn).toHaveBeenCalledOnce()
  })
})
