import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { createTestHarness } from 'wrangler'
import * as ed from '@noble/ed25519'
import { isValidShortCode, type LicenseData } from './license'

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
const privateKey = ed.utils.randomSecretKey()
const privateKeyHex = Array.from(privateKey, (byte) => byte.toString(16).padStart(2, '0')).join('')

const server = createTestHarness({
  workers: [
    {
      configPath: new URL('../../wrangler.toml', import.meta.url),
      // Test-only, and deliberately not the production signer: this key only has to verify here.
      secrets: { ED25519_PRIVATE_KEY: privateKeyHex, PADDLE_WEBHOOK_SECRET_LIVE: webhookSecret },
    },
  ],
})

/** Starting the harness builds the Worker and boots workerd, so it's slower than a normal test. */
beforeAll(async () => {
  await server.listen()
}, 60_000)

afterAll(async () => {
  await server.close()
})

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
