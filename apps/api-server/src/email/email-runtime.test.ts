import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from 'vitest'
import { createTestHarness } from 'wrangler'
import { DAILY_ERROR_REPORT_EMAIL_CAP, errorReportEmailCountKey } from '../telemetry/error-report-intake'
import { EMAIL_PROBE_RECIPIENT } from './send'

/**
 * Every sender in `src/email/` leaving the REAL Worker: wrangler builds `src/index.ts` and runs it
 * in workerd under the production `wrangler.toml`, so a Node API anywhere between a route (or a
 * cron job) and the Resend call throws here exactly as it throws in production.
 *
 * `webhook-runtime.test.ts` already covers `sendLicenseEmail` over the purchase path; this file is
 * the other seven. The reason they need a runtime lane at all, when `eslint.config.js` already
 * bans Node globals in our own source, is the layer lint structurally can't see: a DEPENDENCY
 * reaching for `Buffer` or `process` on a path we exercise. `resend` and the layout helpers are
 * only proven safe by being run.
 *
 * ❌ Never give this harness its own compatibility settings, and never reach for `nodejs_compat`.
 *
 * **Resend and Paddle are stubbed at the socket, not in the source.** The harness points the
 * Worker's global `fetch` at this process, so replacing `globalThis.fetch` here intercepts every
 * outbound request with the route, the SDK, and the runtime untouched. See
 * `../../DETAILS.md` § Test runtimes for the two gotchas in reading an intercepted body.
 *
 * `DISCORD_WEBHOOK_URL` is deliberately left unset: the Discord posts share these paths, and
 * leaving them off keeps the assertion "what reached the wire is the email" exact.
 */

const crashRecipient = 'ops@example.com'
const paddleApiKey = 'test-paddle-key'

const server = createTestHarness({
  workers: [
    {
      configPath: new URL('../../wrangler.toml', import.meta.url),
      secrets: {
        RESEND_API_KEY: 'test-resend-key',
        CRASH_NOTIFICATION_EMAIL: crashRecipient,
        PADDLE_API_KEY_LIVE: paddleApiKey,
      },
    },
  ],
})

interface HarnessEnv {
  TELEMETRY_DB: D1Database
  ERROR_REPORT_META: KVNamespace
  LICENSE_CODES: KVNamespace
}

/** One message as it left the Worker, read off the wire rather than off a mocked SDK. */
interface SentEmail {
  to: string
  from: string
  subject: string
  html?: string
  text?: string
  reply_to?: string | string[]
}

const resend = { sent: [] as SentEmail[] }

/** The Paddle transaction `/validate` resolves, and the customer the device alert looks up. */
const paddleTransaction = { id: 'txn_devicealert', customer_id: 'ctm_01hv8x', custom_data: null }
const paddleCustomer = { email: 'fleet@example.com', name: 'Robin' }

/** Any host we didn't stub. Asserted empty after every test: the Worker talks to two services. */
const unexpectedOutbound: string[] = []

const realFetch = globalThis.fetch

beforeAll(async () => {
  globalThis.fetch = stubbedFetch
  await server.listen()
  await server.getWorker<HarnessEnv>().applyD1Migrations('TELEMETRY_DB')
}, 60_000)

afterAll(async () => {
  globalThis.fetch = realFetch
  await server.close()
})

beforeEach(() => {
  resend.sent.length = 0
})

afterEach(() => {
  expect(unexpectedOutbound).toEqual([])
})

async function stubbedFetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url
  if (url === 'https://api.resend.com/emails') {
    resend.sent.push(JSON.parse(await requestBody(input, init)) as SentEmail)
    return Response.json({ id: `email_${String(resend.sent.length)}` })
  }
  if (url.startsWith('https://api.paddle.com/transactions/')) {
    return Response.json({ data: paddleTransaction })
  }
  if (url.startsWith('https://api.paddle.com/customers/')) {
    return Response.json({ data: paddleCustomer })
  }
  unexpectedOutbound.push(url)
  return Response.json({ error: 'This host is not stubbed' }, { status: 502 })
}

/**
 * The body of an outbound request. The harness hands it over as `fetch(url, request)`, so the body
 * rides on the init rather than the input.
 *
 * ❌ No `instanceof Request` here: the init is undici's internal `Request` class, which is NOT the
 * `Request` this module's global resolves to, so the check silently reads false and the body comes
 * back empty. Duck-typing on `text()` is what survives that.
 */
async function requestBody(input: RequestInfo | URL, init?: RequestInit): Promise<string> {
  for (const candidate of [init, input]) {
    const body = candidate as { text?: () => Promise<string> } | undefined
    if (typeof body?.text === 'function') return await body.text()
  }
  return typeof init?.body === 'string' ? init.body : ''
}

/** The runtime's own logs, for a failure message: a throw inside workerd leaves nothing in the body. */
function workerLogs(): string {
  return server
    .getLogs()
    .map((log) => JSON.stringify(log))
    .join('\n')
}

/**
 * Wait for the mail that rides behind a 200 (`waitUntil`) or behind a cron job.
 *
 * A send that never reaches the wire IS the failure this file exists to catch, and both the route
 * and `runCronJob` swallow it, so the only trace is in the runtime's own logs. They're carried into
 * the assertion message rather than left for someone to go find.
 */
async function awaitEmails(count = 1): Promise<SentEmail[]> {
  try {
    await expect.poll(() => resend.sent.length).toBeGreaterThanOrEqual(count)
  } catch {
    expect.fail(`Expected ${String(count)} email(s), got ${String(resend.sent.length)}.\nWorker logs:\n${workerLogs()}`)
  }
  return resend.sent
}

/** The one message the path under test sent. */
async function awaitEmail(): Promise<SentEmail> {
  return (await awaitEmails())[0]
}

/** What `server.fetch` answers with: workerd's `Response`, not this realm's. */
type HarnessResponse = Awaited<ReturnType<typeof server.fetch>>

/** Fire one cron tick at the given UTC hour. `0` is the one that runs the daily jobs too. */
async function tick(utcHour: number): Promise<void> {
  const scheduledTime = new Date(Date.UTC(2026, 8, 16, utcHour, 0, 0))
  const result = await server.getWorker<HarnessEnv>().scheduled({ scheduledTime, cron: '0 */3 * * *' })
  expect(result.outcome, `Worker logs:\n${workerLogs()}`).toBe('ok')
}

const today = new Date().toISOString().slice(0, 10)

describe('mailing a hand-written error report from the Worker runtime', () => {
  /** Upload one report the way the desktop client does, and keep the amend credential it hands back. */
  async function upload(meta: Record<string, unknown>): Promise<{ id: string; amendKey: string }> {
    const form = new FormData()
    form.set('bundle', new File([new Uint8Array([0x50, 0x4b, 0x05, 0x06])], 'bundle.zip'), 'bundle.zip')
    form.set('meta', JSON.stringify(meta))

    // Serialize the multipart body here rather than handing `FormData` to the harness: the
    // boundary lives in the `content-type` header, and a body re-encoded without it parses as
    // garbage inside the Worker.
    const packed = new Response(form)
    const response = await server.fetch('http://api.getcmdr.com/error-report', {
      method: 'POST',
      headers: { 'Content-Type': packed.headers.get('content-type') ?? '' },
      body: await packed.arrayBuffer(),
    })
    const text = await response.text()
    expect(response.status, `Body: ${text}\nWorker logs:\n${workerLogs()}`).toBe(200)
    return JSON.parse(text) as { id: string; amendKey: string }
  }

  /** The harness answers with workerd's own `Response`, which is not this realm's `Response`. */
  async function amend(id: string, body: Record<string, unknown>): Promise<HarnessResponse> {
    return await server.fetch(`http://api.getcmdr.com/error-report/${id}/amend`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    })
  }

  let uploaded: { id: string; amendKey: string }

  it('mails the report, with the note and the reply-to the reporter attached', async () => {
    uploaded = await upload({
      id: 'ERR-2345A',
      kind: 'user',
      appVersion: '0.9.1',
      osVersion: 'macOS 26.0',
      arch: 'aarch64',
      userNote: 'Copying to the NAS stalls at 99%',
      email: 'reporter@example.com',
      generatedAt: new Date().toISOString(),
    })

    const email = await awaitEmail()

    expect(email.to).toBe(crashRecipient)
    expect(email.subject).toBe('Cmdr: someone sent error report ERR-2345A')
    expect(email.reply_to).toBe('reporter@example.com')
    expect(email.html).toContain('Copying to the NAS stalls at 99%')
    expect(email.html).toContain('ERR-2345A')
  })

  it('mails an amendment onto the report it lands on', async () => {
    const response = await amend(uploaded.id, {
      amendKey: uploaded.amendKey,
      note: 'It also stalls on a local disk',
      email: 'reporter@example.com',
    })
    expect(response.status, `Worker logs:\n${workerLogs()}`).toBe(200)

    const email = await awaitEmail()

    expect(email.to).toBe(crashRecipient)
    expect(email.subject).toBe(`Cmdr: someone added to error report ${uploaded.id}`)
    expect(email.reply_to).toBe('reporter@example.com')
    expect(email.html).toContain('It also stalls on a local disk')
  })

  it("mails the suppression notice once the day's allowance is spent", async () => {
    // Spend the day's allowance outright rather than by sending ten reports: the cap is what's
    // under test, and the counter is the only thing the route reads.
    const env = await server.getWorker<HarnessEnv>().getEnv()
    await env.ERROR_REPORT_META.put(errorReportEmailCountKey(today), String(DAILY_ERROR_REPORT_EMAIL_CAP))

    const response = await amend(uploaded.id, { amendKey: uploaded.amendKey, note: 'one too many' })
    expect(response.status).toBe(200)

    const email = await awaitEmail()

    expect(email.subject).toBe(`Cmdr: error report emails are suppressed for the rest of ${today}`)
    expect(email.html).toContain(String(DAILY_ERROR_REPORT_EMAIL_CAP))
    expect(email.html).not.toContain('one too many')
  })
})

describe('mailing the cron digests from the Worker runtime', () => {
  it('mails the crash digest, then stamps the rows it reported', async () => {
    const env = await server.getWorker<HarnessEnv>().getEnv()
    await env.TELEMETRY_DB.prepare(
      `INSERT INTO crash_reports
         (hashed_ip, backtrace, app_version, os_version, arch, signal, top_function, build_mode,
          short_id, email, panic_message, app_fate)
       VALUES ('', 'cmdr_fs::copy::stream', '0.9.1', 'macOS 26.0', 'aarch64', 'SIGSEGV',
               'cmdr_fs::copy::stream', 'release', 'CRASH-7KQ2M', 'tester@example.com',
               'assertion failed: chunk.len() > 0', 'ended')`,
    ).run()

    await tick(3)
    const email = await awaitEmail()

    expect(email.to).toBe(crashRecipient)
    expect(email.subject).toBe('Cmdr: 1 new crash report')
    expect(email.html).toContain('CRASH-7KQ2M')
    expect(email.html).toContain('cmdr_fs::copy::stream')
    expect(email.html).toContain('assertion failed: chunk.len() &gt; 0')

    const stamped = await env.TELEMETRY_DB.prepare(
      `SELECT notified_at FROM crash_reports WHERE short_id = 'CRASH-7KQ2M'`,
    ).first<{ notified_at: string | null }>()
    expect(stamped?.notified_at).toBeTruthy()
  })

  it('mails the feedback digest, replying to the one person who left an address', async () => {
    const env = await server.getWorker<HarnessEnv>().getEnv()
    await env.TELEMETRY_DB.prepare(
      `INSERT INTO feedback (feedback, email, app_version, os_version, build_mode)
       VALUES ('The dual pane finally makes sense to me', 'happy@example.com', '0.9.1', 'macOS 26.0', 'release')`,
    ).run()

    await tick(3)
    const email = await awaitEmail()

    expect(email.to).toBe(crashRecipient)
    expect(email.subject).toBe('Cmdr: 1 new feedback message')
    expect(email.reply_to).toBe('happy@example.com')
    expect(email.html).toContain('The dual pane finally makes sense to me')

    const stamped = await env.TELEMETRY_DB.prepare(
      `SELECT notified_at FROM feedback WHERE email = 'happy@example.com'`,
    ).first<{ notified_at: string | null }>()
    expect(stamped?.notified_at).toBeTruthy()
  })

  it('sends the daily path probe, which is the only send that proves the key still works', async () => {
    await tick(0)
    const email = await awaitEmail()

    expect(email.to).toBe(EMAIL_PROBE_RECIPIENT)
    expect(email.subject).toBe('Cmdr email path probe')
    expect(email.text).toContain('Nobody receives this')
  })
})

describe('mailing the device count alert from the Worker runtime', () => {
  it('alerts on the seat that crossed the threshold, naming the customer Paddle knows', async () => {
    // One device short of the alert threshold, so this validation is the one that crosses it.
    const env = await server.getWorker<HarnessEnv>().getEnv()
    const seen = new Date().toISOString()
    const devices = Object.fromEntries(Array.from({ length: 5 }, (_, i) => [`device-${String(i)}`, seen]))
    await env.LICENSE_CODES.put(`devices:${paddleTransaction.id}`, JSON.stringify({ devices }))

    const response = await server.fetch('http://api.getcmdr.com/validate', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ transactionId: paddleTransaction.id, deviceId: 'device-the-sixth' }),
    })
    expect(response.status, `Worker logs:\n${workerLogs()}`).toBe(200)

    const email = await awaitEmail()

    expect(email.to).toBe('legal@getcmdr.com')
    expect(email.subject).toBe(`Device count alert: ${paddleTransaction.id} (6 devices)`)
    expect(email.html).toContain(paddleCustomer.email)
    expect(email.html).toContain('vendors.paddle.com')
  })
})

/**
 * Last, and deliberately so: the only way to the DB size alert is a telemetry DB actually past
 * 100 MB, and the harness's D1 keeps that weight for the rest of the run. Writing it takes about
 * three seconds and lives in the harness's temp directory, which goes away with `server.close()`.
 * There is no seam to reach this job with a smaller database, and production code doesn't get one
 * just to be testable.
 */
describe('mailing the DB size alert from the Worker runtime', () => {
  it('alerts with the per-table row counts that say where the size went', async () => {
    const env = await server.getWorker<HarnessEnv>().getEnv()
    await env.TELEMETRY_DB.prepare('CREATE TABLE ballast (blob TEXT)').run()
    const chunk = 'x'.repeat(500_000)
    for (let i = 0; i < 240; i++) {
      const written = await env.TELEMETRY_DB.prepare('INSERT INTO ballast (blob) VALUES (?)').bind(chunk).run()
      if (written.meta.size_after > 100 * 1024 * 1024) break
    }

    await tick(0)

    // The path probe rides the same tick, so the alert is found by subject rather than by position.
    const sent = await awaitEmails(2)
    const email = sent.find((mail) => mail.subject.startsWith('Cmdr: telemetry DB is'))

    expect(email, `Sent: ${sent.map((mail) => mail.subject).join(', ')}`).toBeDefined()
    expect(email?.to).toBe(crashRecipient)
    expect(email?.html).toContain('crash_reports')
    expect(email?.html).toContain('daily_active_users')
  }, 60_000)
})
