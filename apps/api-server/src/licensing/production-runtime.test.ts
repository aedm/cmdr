import { readFile } from 'node:fs/promises'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { createTestHarness } from 'wrangler'
import * as ed from '@noble/ed25519'
import { isValidShortCode, type LicenseData } from './license'
import type { ValidationResponse } from './paddle-api'

/**
 * Mints a license through the REAL Worker: wrangler builds `src/index.ts` and runs it in workerd
 * under `wrangler.toml`, so the compatibility date and flags are production's. Node globals the
 * deployed Worker doesn't have (`Buffer`, `process`) are missing here too, which is the point:
 * every other test in this app runs in Node, where they exist.
 *
 * ❌ Never give this harness its own compatibility settings, and never reach for `nodejs_compat`
 * to make it pass: a test runtime richer than the deployed one is the hole that let a
 * `Buffer.from()` call ship in the minting path and throw on every purchase.
 *
 * `/admin/generate` is the smallest route that runs the whole path (sign, encode, store), so it
 * stands in for the Paddle webhook without needing Paddle.
 */

const webhookSecret = 'test-webhook-secret'
const adminToken = 'test-admin-token'
const privateKey = ed.utils.randomSecretKey()
const privateKeyHex = Array.from(privateKey, (byte) => byte.toString(16).padStart(2, '0')).join('')

const server = createTestHarness({
  workers: [
    {
      configPath: new URL('../../wrangler.toml', import.meta.url),
      // Test-only, and deliberately not the production signer: this key only has to verify here.
      secrets: {
        ED25519_PRIVATE_KEY: privateKeyHex,
        PADDLE_WEBHOOK_SECRET_LIVE: webhookSecret,
        ADMIN_API_TOKEN: adminToken,
      },
    },
  ],
})

interface HarnessEnv {
  LICENSE_CODES: KVNamespace
  TELEMETRY_DB: D1Database
}

/** Starting the harness builds the Worker and boots workerd, so it's slower than a normal test. */
beforeAll(async () => {
  await server.listen()
  await applyLicenseIssuanceSchema()
}, 60_000)

afterAll(async () => {
  await server.close()
})

/**
 * Create `license_issuance` in the harness's D1 by running the real migration files, so a column
 * this suite relies on can't exist only in the test.
 */
async function applyLicenseIssuanceSchema(): Promise<void> {
  const env = await server.getWorker<HarnessEnv>().getEnv()
  for (const file of ['0012_license_issuance.sql', '0017_manual_licenses.sql']) {
    const sql = await readFile(new URL(`../../migrations/${file}`, import.meta.url), 'utf8')
    const statements = sql
      .split('\n')
      .filter((line) => !line.trimStart().startsWith('--'))
      .join('\n')
      .split(';')
      .map((statement) => statement.trim())
      .filter((statement) => statement.length > 0)
    for (const statement of statements) {
      await env.TELEMETRY_DB.prepare(statement).run()
    }
  }
}

type ValidateBody = Partial<ValidationResponse> & { error?: string }

async function validate(transactionId: string): Promise<{ status: number; body: ValidateBody }> {
  const response = await server.fetch('http://api.getcmdr.com/validate', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ transactionId }),
  })
  const text = await response.text()
  return { status: response.status, body: JSON.parse(text) as ValidateBody }
}

/** Insert a manual license row the way `/admin/generate` would, minus the minting. */
async function insertManualLicense(row: {
  transactionId: string
  expiresAt?: string | null
  revokedAt?: string | null
  licenseType?: string
}): Promise<void> {
  const env = await server.getWorker<HarnessEnv>().getEnv()
  await env.TELEMETRY_DB.prepare(
    `INSERT INTO license_issuance
       (transaction_id, source, short_codes, quantity, license_type, customer_email,
        organization_name, note, claimed_at, issued_at, expires_at, revoked_at)
     VALUES (?, 'manual', '["CMDR-2345-6789-ABCD"]', 1, ?, 'friend@example.com',
             'Acme Inc', 'a friend of the project', ?, ?, ?, ?)`,
  )
    .bind(
      row.transactionId,
      row.licenseType ?? 'commercial_perpetual',
      new Date().toISOString(),
      new Date().toISOString(),
      row.expiresAt ?? null,
      row.revokedAt ?? null,
    )
    .run()
}

function base64ToBytes(base64: string): Uint8Array {
  return Uint8Array.from(atob(base64), (char) => char.charCodeAt(0))
}

describe('minting a license in the Worker runtime', () => {
  it('signs, encodes, and stores a license key', async () => {
    const response = await server.fetch('http://api.getcmdr.com/admin/generate', {
      method: 'POST',
      headers: { Authorization: `Bearer ${webhookSecret}`, 'Content-Type': 'application/json' },
      body: JSON.stringify({ email: 'buyer@example.com', type: 'commercial_perpetual' }),
    })

    // Carry the body and the runtime logs into the failure message: a route that throws inside
    // workerd answers with a bare 500, and the reason only exists in the logs.
    const body = await response.text()
    const logs = server.getLogs().map((log) => JSON.stringify(log))
    expect(response.status, `Body: ${body}\nWorker logs:\n${logs.join('\n')}`).toBe(200)

    const { code } = JSON.parse(body) as { code: string }
    expect(isValidShortCode(code)).toBe(true)

    // The key the buyer would receive, read back from the KV namespace the route wrote it to.
    const env = await server.getWorker<{ LICENSE_CODES: KVNamespace }>().getEnv()
    const stored = await env.LICENSE_CODES.get<{ fullKey: string }>(code, 'json')
    const [payloadBase64, signatureBase64] = (stored?.fullKey ?? '').split('.')

    const payloadBytes = base64ToBytes(payloadBase64)
    const publicKey = await ed.getPublicKeyAsync(privateKey)
    expect(await ed.verifyAsync(base64ToBytes(signatureBase64), payloadBytes, publicKey)).toBe(true)

    const payload = JSON.parse(new TextDecoder().decode(payloadBytes)) as LicenseData
    expect(payload.email).toBe('buyer@example.com')
    expect(payload.type).toBe('commercial_perpetual')
    expect(payload.shortCode).toBe(code)
  })
})

/**
 * A manually issued license has no Paddle transaction behind it, so `/validate` has to answer for
 * it from the `license_issuance` ledger. The harness has no Paddle API key, which doubles as the
 * dispatch proof: anything that reaches the Paddle branch answers 502 `upstream_error`.
 */
describe('validating a manual license', () => {
  it('reports a live manual license as active, with its type and organization', async () => {
    await insertManualLicense({ transactionId: 'manual-ACTIVE01' })

    const { status, body } = await validate('manual-ACTIVE01')

    expect(status).toBe(200)
    expect(body.status).toBe('active')
    expect(body.type).toBe('commercial_perpetual')
    expect(body.organizationName).toBe('Acme Inc')
    expect(body.expiresAt).toBeNull()
  })

  it('reports a manual license past its expiry as expired', async () => {
    await insertManualLicense({
      transactionId: 'manual-EXPIRED1',
      licenseType: 'commercial_subscription',
      expiresAt: '2020-01-01T00:00:00.000Z',
    })

    const { body } = await validate('manual-EXPIRED1')

    expect(body.status).toBe('expired')
    expect(body.type).toBe('commercial_subscription')
    expect(body.expiresAt).toBe('2020-01-01T00:00:00.000Z')
  })

  it('reports a revoked manual license as invalid', async () => {
    await insertManualLicense({ transactionId: 'manual-REVOKED1', revokedAt: '2026-09-01T00:00:00.000Z' })

    const { body } = await validate('manual-REVOKED1')

    expect(body.status).toBe('invalid')
    expect(body.type).toBeNull()
  })

  it('reports an unknown manual id as invalid without asking Paddle', async () => {
    const { status, body } = await validate('manual-NOSUCHID')

    expect(status).toBe(200)
    expect(body.status).toBe('invalid')
  })

  it('still resolves a Paddle transaction id against Paddle', async () => {
    // No Paddle API key in the harness, so the Paddle branch can only answer 502. That failure IS
    // the assertion: a `txn_` id must never be answered from the ledger.
    const { status, body } = await validate('txn_01abcdef')

    expect(status).toBe(502)
    expect(body).toEqual({ error: 'upstream_error' })
  })
})
