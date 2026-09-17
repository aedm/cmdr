/**
 * Client-safe helpers for the Licenses page (`/licenses`): the shape `GET /admin/licenses` returns,
 * the labels the table shows, and the pure summaries the page renders. Pure functions only, so the
 * page (browser bundle), the server source, and the tests share one copy. The fetch itself lives in
 * `$lib/server/sources/licenses.ts`.
 *
 * The types mirror `apps/api-server/src/licensing/admin-licenses.ts` by hand (the two apps ship
 * separately). The api-server owns the contract: what the fields mean and what each state decides.
 */

import { formatNumber } from './format.js'

/** Where the license came from: a purchase through Paddle, or one we handed out. */
export type LicenseSource = 'paddle' | 'manual'

/**
 * The note's ceiling, mirroring `maxLicenseNoteLength` in the api-server (the two apps ship
 * separately, so the constant is duplicated by hand; the api-server owns it). Checked here only to
 * answer before a round-trip.
 */
export const maxNoteLength = 2000

/** The api-server's computed state per row. Meanings: `admin-licenses.ts` and its `DETAILS.md`. */
export type LicenseState = 'active' | 'expired' | 'revoked' | 'undelivered' | 'unfinished'

/** One ledger row as the endpoint returns it, plus the state the api-server computed for it. */
export interface LicenseRow {
  transactionId: string
  source: LicenseSource
  /** One code per seat. Empty on a row that was claimed but never minted. */
  shortCodes: string[]
  licenseType: string | null
  customerEmail: string | null
  organizationName: string | null
  note: string | null
  quantity: number | null
  claimedAt: string
  issuedAt: string | null
  emailedAt: string | null
  /** Only a hand-issued license carries one; a purchase's end date lives in Paddle. */
  expiresAt: string | null
  revokedAt: string | null
  state: LicenseState
}

/** The whole response: every ledger row, plus the two ways the ledger and the key store can disagree. */
export interface LicenseListing {
  licenses: LicenseRow[]
  /** Codes in the key store that no ledger row explains. */
  orphanCodes: string[]
  /** Codes a row claims we issued that are gone from the key store, so they can't be activated. */
  missingCodes: string[]
}

/**
 * What the edit dialog is allowed to send. Only the two things this side can know: a row was named,
 * and the note fits. Whether a blank note is allowed depends on the source, and the api-server
 * decides that, so a rejection there is shown verbatim rather than guessed at here.
 */
export function validateNote(raw: {
  transactionId: string
  note: string
}): { ok: true; transactionId: string; note: string } | { ok: false; error: string } {
  const transactionId = raw.transactionId.trim()
  if (transactionId.length === 0) return { ok: false, error: 'No license was named.' }
  if (raw.note.length > maxNoteLength) {
    return {
      ok: false,
      error: `That note is ${formatNumber(raw.note.length)} characters, and the limit is ${formatNumber(maxNoteLength)}.`,
    }
  }
  return { ok: true, transactionId, note: raw.note }
}

/** How loud a state should read in the table. */
export type StateTone = 'ok' | 'warning' | 'alarm' | 'muted'

/**
 * The badge label per state.
 *
 * ❌ None of these says a Paddle subscription is still running, and none may start to: `active` only
 * means our own records are complete for that row (codes minted, and for a purchase, emailed). Only
 * `/validate` asks Paddle whether a subscription is live.
 */
export function stateLabel(state: LicenseState): string {
  switch (state) {
    case 'active':
      return 'Issued'
    case 'expired':
      return 'Expired'
    case 'revoked':
      return 'Revoked'
    case 'undelivered':
      return 'Not emailed'
    case 'unfinished':
      return 'No codes'
  }
}

/** A one-line explanation of a state, shown as the badge's tooltip. */
export function stateExplanation(state: LicenseState): string {
  switch (state) {
    case 'active':
      return 'Codes are minted, and for a purchase the email went out. Says nothing about whether a Paddle subscription still runs.'
    case 'expired':
      return 'Past its expiry date. Only a hand-issued license carries one.'
    case 'revoked':
      return 'Killed by /admin/revoke. Its codes are gone from the key store on purpose.'
    case 'undelivered':
      return 'Codes were minted but never emailed. Someone paid and is waiting.'
    case 'unfinished':
      return 'Claimed but never minted, so there are no codes at all. A delivery that died, or one in flight this second.'
  }
}

export function stateTone(state: LicenseState): StateTone {
  switch (state) {
    case 'active':
      return 'ok'
    case 'expired':
      return 'warning'
    case 'revoked':
      return 'muted'
    case 'undelivered':
    case 'unfinished':
      return 'alarm'
  }
}

export function sourceLabel(source: LicenseSource): string {
  return source === 'manual' ? 'Hand-issued' : 'Purchased'
}

const licenseTypeLabels: Record<string, string> = {
  commercial_subscription: 'Commercial subscription',
  commercial_perpetual: 'Commercial perpetual',
}

/** A readable license type. An unknown one shows verbatim rather than vanishing behind a dash. */
export function licenseTypeLabel(licenseType: string | null): string {
  if (!licenseType) return '–'
  return licenseTypeLabels[licenseType] ?? licenseType
}

/**
 * What an empty expiry column means, which depends on the source: a hand-issued license with no
 * expiry really is perpetual, while a purchase's end date lives in Paddle and we simply don't have it.
 * ❌ Never print "Never" for a purchase: that would read as a perpetual license on a subscription.
 */
export function expiryPlaceholder(source: LicenseSource): string {
  return source === 'manual' ? 'Never' : '–'
}

/** The source filter above the table. */
export type SourceFilter = 'all' | LicenseSource

export function filterBySource(rows: LicenseRow[], filter: SourceFilter): LicenseRow[] {
  return filter === 'all' ? rows : rows.filter((row) => row.source === filter)
}

/** The counts above the table, plus the rows and codes that mean something is wrong. */
export interface LicenseSummary {
  total: number
  purchased: number
  handIssued: number
  /** Purchases whose codes were minted but never emailed. */
  undelivered: number
  /** Rows claimed but never minted. Today's outage left exactly this shape. */
  unfinished: number
  /** True when any row, orphan code, or missing code needs a human. */
  needsAttention: boolean
}

export function summarizeLicenses(listing: LicenseListing): LicenseSummary {
  const undelivered = listing.licenses.filter((row) => row.state === 'undelivered').length
  const unfinished = listing.licenses.filter((row) => row.state === 'unfinished').length
  return {
    total: listing.licenses.length,
    purchased: listing.licenses.filter((row) => row.source === 'paddle').length,
    handIssued: listing.licenses.filter((row) => row.source === 'manual').length,
    undelivered,
    unfinished,
    needsAttention:
      undelivered > 0 || unfinished > 0 || listing.orphanCodes.length > 0 || listing.missingCodes.length > 0,
  }
}
