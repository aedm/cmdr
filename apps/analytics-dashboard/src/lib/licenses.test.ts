import { describe, it, expect } from 'vitest'
import {
  expiryPlaceholder,
  filterBySource,
  licenseTypeLabel,
  sourceLabel,
  stateLabel,
  stateTone,
  summarizeLicenses,
  type LicenseRow,
  type LicenseState,
} from './licenses.js'

function row(overrides: Partial<LicenseRow> = {}): LicenseRow {
  return {
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
    ...overrides,
  }
}

const allStates: LicenseState[] = ['active', 'expired', 'revoked', 'undelivered', 'unfinished']

describe('stateLabel', () => {
  it('labels every state', () => {
    expect(allStates.map(stateLabel)).toEqual(['Issued', 'Expired', 'Revoked', 'Not emailed', 'No codes'])
  })

  it("never claims a subscription is active, since our records can't know that", () => {
    for (const state of allStates) {
      expect(stateLabel(state).toLowerCase()).not.toContain('active')
      expect(stateLabel(state).toLowerCase()).not.toContain('subscri')
    }
  })
})

describe('stateTone', () => {
  it('shouts about the two states that mean someone is waiting', () => {
    expect(stateTone('undelivered')).toBe('alarm')
    expect(stateTone('unfinished')).toBe('alarm')
  })

  it('keeps the settled states quiet', () => {
    expect(stateTone('active')).toBe('ok')
    expect(stateTone('expired')).toBe('warning')
    expect(stateTone('revoked')).toBe('muted')
  })
})

describe('sourceLabel', () => {
  it('separates a purchase from one we handed out', () => {
    expect(sourceLabel('paddle')).toBe('Purchased')
    expect(sourceLabel('manual')).toBe('Hand-issued')
  })
})

describe('licenseTypeLabel', () => {
  it('reads the known types back in plain words', () => {
    expect(licenseTypeLabel('commercial_subscription')).toBe('Commercial subscription')
    expect(licenseTypeLabel('commercial_perpetual')).toBe('Commercial perpetual')
  })

  it('shows an unknown type verbatim rather than hiding it', () => {
    expect(licenseTypeLabel('team_seat')).toBe('team_seat')
  })

  it('dashes an absent type', () => {
    expect(licenseTypeLabel(null)).toBe('–')
  })
})

describe('expiryPlaceholder', () => {
  it('calls a hand-issued license with no expiry perpetual', () => {
    expect(expiryPlaceholder('manual')).toBe('Never')
  })

  it("dashes a purchase, whose end date we don't hold", () => {
    expect(expiryPlaceholder('paddle')).toBe('–')
  })
})

describe('filterBySource', () => {
  const rows = [row(), row({ transactionId: 'man_01', source: 'manual' })]

  it('passes everything through on "all"', () => {
    expect(filterBySource(rows, 'all')).toHaveLength(2)
  })

  it('keeps only the asked-for source', () => {
    expect(filterBySource(rows, 'manual').map((r) => r.transactionId)).toEqual(['man_01'])
    expect(filterBySource(rows, 'paddle').map((r) => r.transactionId)).toEqual(['txn_01'])
  })
})

describe('summarizeLicenses', () => {
  it('counts the two sources', () => {
    const summary = summarizeLicenses({
      licenses: [row(), row({ source: 'manual' }), row({ source: 'manual' })],
      orphanCodes: [],
      missingCodes: [],
    })
    expect(summary).toMatchObject({ total: 3, purchased: 1, handIssued: 2, needsAttention: false })
  })

  it('flags a purchase that was minted but never emailed', () => {
    const summary = summarizeLicenses({
      licenses: [row({ state: 'undelivered', emailedAt: null })],
      orphanCodes: [],
      missingCodes: [],
    })
    expect(summary.undelivered).toBe(1)
    expect(summary.needsAttention).toBe(true)
  })

  it('flags a purchase that was claimed but never minted', () => {
    const summary = summarizeLicenses({
      licenses: [row({ state: 'unfinished', shortCodes: [], issuedAt: null, emailedAt: null })],
      orphanCodes: [],
      missingCodes: [],
    })
    expect(summary.unfinished).toBe(1)
    expect(summary.needsAttention).toBe(true)
  })

  it('flags a code the ledger and the key store disagree about, even with every row healthy', () => {
    expect(
      summarizeLicenses({ licenses: [row()], orphanCodes: ['CMDR-ZZZZ-ZZZZ-ZZZZ'], missingCodes: [] }).needsAttention,
    ).toBe(true)
    expect(
      summarizeLicenses({ licenses: [row()], orphanCodes: [], missingCodes: ['CMDR-YYYY-YYYY-YYYY'] }).needsAttention,
    ).toBe(true)
  })

  it('reports an empty ledger as nothing wrong', () => {
    expect(summarizeLicenses({ licenses: [], orphanCodes: [], missingCodes: [] })).toEqual({
      total: 0,
      purchased: 0,
      handIssued: 0,
      undelivered: 0,
      unfinished: 0,
      needsAttention: false,
    })
  })
})
