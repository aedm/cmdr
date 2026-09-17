import { describe, it, expect, vi, beforeEach } from 'vitest'
import { fetchLicenses, updateLicenseNote } from './licenses.js'

const env = { LICENSE_SERVER_ADMIN_TOKEN: 'test-admin-token' }

const listing = {
  licenses: [
    {
      transactionId: 'txn_01',
      source: 'paddle',
      shortCodes: ['CMDR-AAAA-BBBB-CCCC'],
      licenseType: 'commercial_subscription',
      customerEmail: 'buyer@example.com',
      organizationName: null,
      note: null,
      quantity: 1,
      claimedAt: '2026-09-16T10:00:00.000Z',
      issuedAt: '2026-09-16T10:00:01.000Z',
      emailedAt: '2026-09-16T10:00:02.000Z',
      expiresAt: null,
      revokedAt: null,
      state: 'active',
    },
  ],
  orphanCodes: [],
  missingCodes: [],
}

describe('fetchLicenses', () => {
  beforeEach(() => {
    vi.restoreAllMocks()
  })

  it('returns the listing and sends the bearer token to the admin endpoint', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: () => Promise.resolve(listing) })
    vi.stubGlobal('fetch', fetchMock)

    const result = await fetchLicenses(env)
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(result.data).toEqual(listing)
    expect(fetchMock.mock.calls[0][0]).toBe('https://api.getcmdr.com/admin/licenses')
    expect(fetchMock.mock.calls[0][1]?.headers).toEqual({ Authorization: 'Bearer test-admin-token' })
  })

  it('returns an error on 401 rather than an empty list', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({ ok: false, status: 401, text: () => Promise.resolve('Unauthorized') }),
    )
    const result = await fetchLicenses(env)
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toContain('401')
  })

  it('reports a network failure as an error', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('connection reset')))
    const result = await fetchLicenses(env)
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toContain('connection reset')
  })

  it('honors WORKER_BASE_URL override', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: () => Promise.resolve(listing) })
    vi.stubGlobal('fetch', fetchMock)
    await fetchLicenses({ ...env, WORKER_BASE_URL: 'http://127.0.0.1:18900' })
    expect(fetchMock.mock.calls[0][0]).toBe('http://127.0.0.1:18900/admin/licenses')
  })
})

describe('updateLicenseNote', () => {
  beforeEach(() => {
    vi.restoreAllMocks()
  })

  it('PUTs the note to the row it names and returns what landed', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      text: () => Promise.resolve(JSON.stringify({ transactionId: 'txn_01', note: 'First purchase ever!!' })),
    })
    vi.stubGlobal('fetch', fetchMock)

    const result = await updateLicenseNote(env, { transactionId: 'txn_01', note: 'First purchase ever!!' })

    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(result.data).toBe('First purchase ever!!')
    expect(fetchMock.mock.calls[0][0]).toBe('https://api.getcmdr.com/admin/licenses/txn_01/note')
    expect(fetchMock.mock.calls[0][1]?.method).toBe('PUT')
    expect(fetchMock.mock.calls[0][1]?.body).toBe(JSON.stringify({ note: 'First purchase ever!!' }))
  })

  it("shows the worker's own objection rather than a status code", async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: false,
        status: 400,
        text: () =>
          Promise.resolve(JSON.stringify({ error: "A hand-issued license needs a note saying who it's for and why" })),
      }),
    )

    const result = await updateLicenseNote(env, { transactionId: 'manual-GIFT', note: '' })

    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toBe("A hand-issued license needs a note saying who it's for and why")
  })

  it('falls back to the status when the body is not the shape we expect', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({ ok: false, status: 502, text: () => Promise.resolve('<html>bad gateway</html>') }),
    )

    const result = await updateLicenseNote(env, { transactionId: 'txn_01', note: 'anything' })

    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toContain('502')
  })

  it('escapes a transaction id into the path', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue({ ok: true, text: () => Promise.resolve(JSON.stringify({ note: null })) })
    vi.stubGlobal('fetch', fetchMock)

    await updateLicenseNote(env, { transactionId: 'manual-A/B', note: 'x' })

    expect(fetchMock.mock.calls[0][0]).toBe('https://api.getcmdr.com/admin/licenses/manual-A%2FB/note')
  })
})
