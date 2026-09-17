/**
 * A daily snapshot of every license this server has issued, written to R2.
 *
 * D1 carries 30 days of Time Travel, but KV carries nothing: lose the `LICENSE_CODES` namespace and
 * the signed keys are gone, while the ledger row that describes each one survives. That asymmetry
 * is the whole reason this exists, so the snapshot holds BOTH halves (the row and the stored key)
 * and is restorable on its own, without the signing key and without re-minting anything.
 *
 * It lives under its own R2 prefix, away from `error-reports/`: the eviction sweep and its size
 * watermarks only ever list that one, so nothing here is swept or counted against it.
 */

import { readWholeLedger, type LedgerEntry } from './license-issuance'
import { isValidShortCode } from './license'

/** R2 key prefix for license snapshots. One object per day, and nothing deletes them. */
export const licenseBackupPrefix = 'backups/licenses/'

/** The snapshot's shape. Versioned so a future reader knows what it's holding. */
export interface LicenseBackup {
  version: 1
  generatedAt: string
  licenses: LedgerEntry[]
  /**
   * Every license short code in KV with the record stored under it, including codes no ledger row
   * explains. The ledger says a license exists; this is what makes it work.
   */
  keys: Record<string, unknown>
}

export function licenseBackupKey(when: Date): string {
  return `${licenseBackupPrefix}${when.toISOString().slice(0, 10)}.json`
}

interface BackupEnv {
  TELEMETRY_DB: D1Database
  LICENSE_CODES: KVNamespace
  ERROR_REPORTS_BUCKET: R2Bucket
}

/**
 * Snapshot the ledger and the key store into one R2 object, named for the day it was taken. A
 * second run the same day overwrites it, so a retried tick can't leave two versions of one day.
 */
export async function backupLicenseLedger(env: BackupEnv, when: Date): Promise<LicenseBackup> {
  const [licenses, keys] = await Promise.all([readWholeLedger(env.TELEMETRY_DB), readStoredKeys(env.LICENSE_CODES)])

  const backup: LicenseBackup = {
    version: 1,
    generatedAt: when.toISOString(),
    licenses,
    keys,
  }

  await env.ERROR_REPORTS_BUCKET.put(licenseBackupKey(when), JSON.stringify(backup, null, 2), {
    httpMetadata: { contentType: 'application/json' },
    customMetadata: {
      licenses: String(licenses.length),
      keys: String(Object.keys(keys).length),
    },
  })

  return backup
}

/**
 * Every license short code in KV, with its stored value. The namespace also holds device sets
 * (`devices:…`) and the activation counter, so the code format is what separates a license from
 * bookkeeping, the same test `/admin/licenses` applies.
 *
 * A key that vanishes between the list and the read is left out rather than stored as null: the
 * snapshot says what existed, and a null would read as a license with no key.
 */
async function readStoredKeys(kv: KVNamespace): Promise<Record<string, unknown>> {
  const keys: Record<string, unknown> = {}
  let cursor: string | undefined
  for (;;) {
    const page = await kv.list({ cursor })
    for (const key of page.keys) {
      if (!isValidShortCode(key.name)) continue
      const stored = await kv.get(key.name, 'json')
      if (stored !== null) keys[key.name] = stored
    }
    if (page.list_complete) return keys
    cursor = page.cursor
  }
}
