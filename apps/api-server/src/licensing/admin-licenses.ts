/**
 * `GET /admin/licenses`: every license this server has issued, in one list, for the dashboard's
 * Licenses page. Paddle's dashboard can't answer this — it knows about money, not about license
 * codes, activations, or anything we handed out by hand.
 *
 * It reads the `license_issuance` ledger and the `LICENSE_CODES` KV namespace and reconciles the
 * two, because each can hold a license the other doesn't know about.
 */

import { Hono } from 'hono'
import { isValidShortCode } from './license'
import { listLedger, type LedgerEntry } from './license-issuance'
import { type Bindings, verifyAdminAuth } from '../types'

const adminLicenses = new Hono<{ Bindings: Bindings }>()

/**
 * What a license is doing right now, as far as we can tell from our own records.
 *
 * ❌ Not the same vocabulary `/validate` answers the app in, and it can't be: for a Paddle row,
 * `active` means "we fulfilled this purchase", never "the subscription is still running" — only
 * Paddle knows that, and asking it per row would cost one API call per license on every page load.
 * For a manual row the two agree, except that `/validate` calls a revoked license `invalid`.
 *
 * - `active`: codes issued and (for a purchase) delivered, not expired, not revoked.
 * - `expired`: past `expiresAt`, which only a hand-issued license carries.
 * - `revoked`: killed by `/admin/revoke`. Its codes are gone from KV by design.
 * - `undelivered`: a purchase whose codes were minted but never emailed. Someone paid and is waiting.
 * - `unfinished`: claimed, never minted. A delivery that died, or one in flight this second.
 */
export type LicenseState = 'active' | 'expired' | 'revoked' | 'undelivered' | 'unfinished'

export interface LicenseListEntry extends LedgerEntry {
  state: LicenseState
}

export interface LicenseListing {
  licenses: LicenseListEntry[]
  /**
   * Activation codes sitting in KV that no ledger row explains: licenses handed out before the
   * ledger existed, or minted by a delivery that died before recording them. Someone may be holding
   * one, and nothing here says whether it ever worked.
   */
  orphanCodes: string[]
  /**
   * The mirror image: codes a ledger row says we issued that are no longer in KV, so they can't be
   * activated. Revoked licenses are left out, since revoking deletes their codes on purpose.
   */
  missingCodes: string[]
}

export function classifyLedgerEntry(entry: LedgerEntry, nowMs: number): LicenseState {
  if (entry.revokedAt) return 'revoked'
  if (entry.expiresAt) {
    const expiresMs = Date.parse(entry.expiresAt)
    // An unreadable expiry reads as expired, the same call `classifyManualLicense` makes for
    // `/validate`, so the two never disagree about a license in front of the same person.
    if (Number.isNaN(expiresMs) || expiresMs <= nowMs) return 'expired'
  }
  if (entry.shortCodes.length === 0) return 'unfinished'
  // A hand-issued license is often deliberately not emailed (the code goes into a reply by hand),
  // so only a purchase with no `emailedAt` is someone left waiting.
  if (entry.source === 'paddle' && !entry.emailedAt) return 'undelivered'
  return 'active'
}

adminLicenses.get('/admin/licenses', async (c) => {
  const unauthorized = verifyAdminAuth(c)
  if (unauthorized) return unauthorized

  const [ledger, codesInKv] = await Promise.all([
    listLedger(c.env.TELEMETRY_DB),
    listShortCodes(c.env.LICENSE_CODES),
  ])

  const now = Date.now()
  const licenses = ledger.map((entry) => ({ ...entry, state: classifyLedgerEntry(entry, now) }))

  const recorded = new Set(ledger.flatMap((entry) => entry.shortCodes))
  const stored = new Set(codesInKv)
  const orphanCodes = codesInKv.filter((code) => !recorded.has(code)).sort()
  const missingCodes = ledger
    .filter((entry) => !entry.revokedAt)
    .flatMap((entry) => entry.shortCodes)
    .filter((code) => !stored.has(code))
    .sort()

  return c.json<LicenseListing>({ licenses, orphanCodes, missingCodes })
})

/**
 * Every license short code in the KV namespace. It also holds device sets (`devices:…`) and the
 * activation counter, so the code format is what separates a license from bookkeeping.
 */
async function listShortCodes(kv: KVNamespace): Promise<string[]> {
  const codes: string[] = []
  let cursor: string | undefined
  for (;;) {
    const page = await kv.list({ cursor })
    for (const key of page.keys) {
      if (isValidShortCode(key.name)) codes.push(key.name)
    }
    if (page.list_complete) return codes
    cursor = page.cursor
  }
}

export { adminLicenses }
