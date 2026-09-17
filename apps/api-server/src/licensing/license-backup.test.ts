import { describe, expect, it, vi } from 'vitest'
import { backupLicenseLedger, licenseBackupKey, type LicenseBackup } from './license-backup'

interface LedgerRow {
  transaction_id: string
  source: string
  short_codes: string | null
  quantity: number | null
  license_type: string | null
  customer_email: string | null
  organization_name: string | null
  note: string | null
  claimed_at: string
  issued_at: string | null
  emailed_at: string | null
  expires_at: string | null
  revoked_at: string | null
}

function ledgerRow(overrides: Partial<LedgerRow> & { transaction_id: string }): LedgerRow {
  return {
    source: 'paddle',
    short_codes: '[]',
    quantity: 1,
    license_type: 'commercial_perpetual',
    customer_email: 'buyer@example.com',
    organization_name: null,
    note: null,
    claimed_at: '2026-09-01T10:00:00.000Z',
    issued_at: '2026-09-01T10:00:01.000Z',
    emailed_at: '2026-09-01T10:00:02.000Z',
    expires_at: null,
    revoked_at: null,
    ...overrides,
  }
}

function createD1(rows: LedgerRow[]): D1Database {
  return {
    prepare: () => ({ all: () => Promise.resolve({ results: rows }) }),
  } as unknown as D1Database
}

/** KV that pages, so a scan stopping at the first page is visible. `values` drives `get`. */
function createKv(pages: string[][], values: Record<string, unknown>): KVNamespace {
  return {
    list: ({ cursor }: { cursor?: string } = {}) => {
      const index = cursor ? Number(cursor) : 0
      const isLast = index >= pages.length - 1
      return Promise.resolve({
        keys: (pages[index] ?? []).map((name) => ({ name })),
        list_complete: isLast,
        cursor: isLast ? undefined : String(index + 1),
      })
    },
    get: (name: string) => Promise.resolve(values[name] ?? null),
  } as unknown as KVNamespace
}

function createBucket(): { bucket: R2Bucket; put: ReturnType<typeof vi.fn> } {
  const put = vi.fn(() => Promise.resolve())
  return { bucket: { put } as unknown as R2Bucket, put }
}

function writtenBackup(put: ReturnType<typeof vi.fn>): LicenseBackup {
  return JSON.parse(put.mock.calls[0][1] as string) as LicenseBackup
}

const when = new Date('2026-09-17T00:00:00.000Z')

describe('licenseBackupKey', () => {
  it('names the object for the day the snapshot was taken', () => {
    expect(licenseBackupKey(when)).toBe('backups/licenses/2026-09-17.json')
  })

  it('stays out of the error-report prefix, which the eviction sweep owns', () => {
    expect(licenseBackupKey(when).startsWith('error-reports/')).toBe(false)
  })
})

describe('backupLicenseLedger', () => {
  it('writes every ledger row and every stored key into one object', async () => {
    const { bucket, put } = createBucket()
    const rows = [
      ledgerRow({ transaction_id: 'txn_paid', short_codes: '["CMDR-2345-6789-ABCD"]', note: 'First sale ever!!' }),
      ledgerRow({ transaction_id: 'manual-GIFT', source: 'manual', short_codes: '["CMDR-3456-789A-BCDE"]' }),
    ]

    await backupLicenseLedger(
      {
        TELEMETRY_DB: createD1(rows),
        LICENSE_CODES: createKv([['CMDR-2345-6789-ABCD', 'CMDR-3456-789A-BCDE']], {
          'CMDR-2345-6789-ABCD': { fullKey: 'paid-key' },
          'CMDR-3456-789A-BCDE': { fullKey: 'gift-key' },
        }),
        ERROR_REPORTS_BUCKET: bucket,
      },
      when,
    )

    expect(put.mock.calls[0][0]).toBe('backups/licenses/2026-09-17.json')
    const backup = writtenBackup(put)
    expect(backup.version).toBe(1)
    expect(backup.generatedAt).toBe('2026-09-17T00:00:00.000Z')
    expect(backup.licenses.map((license) => license.transactionId)).toEqual(['txn_paid', 'manual-GIFT'])
    expect(backup.licenses[0].note).toBe('First sale ever!!')
    expect(backup.keys).toEqual({
      'CMDR-2345-6789-ABCD': { fullKey: 'paid-key' },
      'CMDR-3456-789A-BCDE': { fullKey: 'gift-key' },
    })
  })

  it('carries a key no ledger row explains, which is the one nothing else could restore', async () => {
    const { bucket, put } = createBucket()

    await backupLicenseLedger(
      {
        TELEMETRY_DB: createD1([]),
        LICENSE_CODES: createKv([['CMDR-4567-89AB-CDEF']], { 'CMDR-4567-89AB-CDEF': { fullKey: 'orphan-key' } }),
        ERROR_REPORTS_BUCKET: bucket,
      },
      when,
    )

    expect(writtenBackup(put).keys).toEqual({ 'CMDR-4567-89AB-CDEF': { fullKey: 'orphan-key' } })
  })

  it('walks every KV page, so a second page of licenses is not silently dropped', async () => {
    const { bucket, put } = createBucket()

    await backupLicenseLedger(
      {
        TELEMETRY_DB: createD1([]),
        LICENSE_CODES: createKv([['CMDR-2345-6789-ABCD'], ['CMDR-3456-789A-BCDE']], {
          'CMDR-2345-6789-ABCD': { fullKey: 'first-page' },
          'CMDR-3456-789A-BCDE': { fullKey: 'second-page' },
        }),
        ERROR_REPORTS_BUCKET: bucket,
      },
      when,
    )

    expect(Object.keys(writtenBackup(put).keys)).toEqual(['CMDR-2345-6789-ABCD', 'CMDR-3456-789A-BCDE'])
  })

  it('leaves out the namespace bookkeeping, which is not a license', async () => {
    const { bucket, put } = createBucket()

    await backupLicenseLedger(
      {
        TELEMETRY_DB: createD1([]),
        LICENSE_CODES: createKv([['devices:txn_paid', '_meta:activation_count', 'CMDR-2345-6789-ABCD']], {
          'devices:txn_paid': { devices: [] },
          '_meta:activation_count': 7,
          'CMDR-2345-6789-ABCD': { fullKey: 'real-key' },
        }),
        ERROR_REPORTS_BUCKET: bucket,
      },
      when,
    )

    expect(Object.keys(writtenBackup(put).keys)).toEqual(['CMDR-2345-6789-ABCD'])
  })

  it('leaves out a code that vanished between the list and the read, rather than storing a null key', async () => {
    const { bucket, put } = createBucket()

    await backupLicenseLedger(
      {
        TELEMETRY_DB: createD1([]),
        LICENSE_CODES: createKv([['CMDR-2345-6789-ABCD', 'CMDR-3456-789A-BCDE']], {
          'CMDR-2345-6789-ABCD': { fullKey: 'still-there' },
        }),
        ERROR_REPORTS_BUCKET: bucket,
      },
      when,
    )

    expect(Object.keys(writtenBackup(put).keys)).toEqual(['CMDR-2345-6789-ABCD'])
  })

  it('records the counts as object metadata, so a listing shows what each day held', async () => {
    const { bucket, put } = createBucket()

    await backupLicenseLedger(
      {
        TELEMETRY_DB: createD1([ledgerRow({ transaction_id: 'txn_paid' })]),
        LICENSE_CODES: createKv([['CMDR-2345-6789-ABCD']], { 'CMDR-2345-6789-ABCD': { fullKey: 'paid-key' } }),
        ERROR_REPORTS_BUCKET: bucket,
      },
      when,
    )

    const options = put.mock.calls[0][2] as R2PutOptions
    expect(options.customMetadata).toEqual({ licenses: '1', keys: '1' })
    expect(options.httpMetadata).toEqual({ contentType: 'application/json' })
  })
})
