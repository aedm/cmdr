import { Hono, type Context } from 'hono'
import { enforceIpRateLimit, type Bindings } from '../types'
import { versionPattern } from './telemetry'

const heartbeat = new Hono<{ Bindings: Bindings }>()

// Heartbeat ingestion: one row per beat (launch + hourly) for true daily-active tracking.
// Identity is the random `anal_<uuid>` analytics id; no IP is stored (the id is the dedup key).
// The whole request body is capped, and the config blob is capped again on its own so a single
// fat config can't dominate the budget.
const maxHeartbeatBytes = 32 * 1024
const maxConfigJsonBytes = 16 * 1024
const heartbeatRequiredFields = ['analId', 'appVersion', 'osVersion', 'arch'] as const
// `anal_` + a v4 UUID (36 chars: 32 hex digits plus the 4 dashes).
const analIdPattern = /^anal_[0-9a-f-]{36}$/

interface Heartbeat {
  analId: string
  appVersion: string
  osVersion: string
  arch: string
  /** Optional. `"release"` or `"debug"`; older clients don't set it (stored as NULL). */
  buildMode?: 'release' | 'debug' | null
  /** Optional. The allowlisted config-shape snapshot, stored verbatim as `config_json`. */
  config?: Record<string, unknown> | null
}

/**
 * Validate the optional `buildMode` field. `null` and `undefined` both mean "absent": Rust
 * serializes `Option::None` as `null` (specta's unified mode rejects `skip_serializing_if`), so
 * tolerate both rather than rejecting upgrade-window beats.
 */
function validateBuildMode(buildMode: unknown): string | null {
  if (buildMode !== undefined && buildMode !== null && buildMode !== 'release' && buildMode !== 'debug') {
    return 'Invalid buildMode'
  }
  return null
}

/** Validate the optional `config` blob: must be a plain object, capped at `maxConfigJsonBytes`. */
function validateConfig(config: unknown): string | null {
  if (config === undefined || config === null) return null
  if (typeof config !== 'object' || Array.isArray(config)) return 'Invalid config'
  if (JSON.stringify(config).length > maxConfigJsonBytes) return 'Config too large'
  return null
}

/**
 * Validate the runtime shape of a `POST /heartbeat` body. Returns `null` if the body is
 * well-formed; otherwise the error message to surface as 400. Input is typed as
 * `Record<string, unknown>` (not `Heartbeat`) so the optional-field checks aren't statically
 * narrowed away: values arrive from `JSON.parse` and can be any shape a client sends.
 */
function validateHeartbeatShape(beat: Record<string, unknown>): string | null {
  for (const field of heartbeatRequiredFields) {
    const value = beat[field]
    if (typeof value !== 'string' || value.length === 0) {
      return `Missing required field: ${field}`
    }
  }
  if (!analIdPattern.test(beat.analId as string)) return 'Invalid analId'
  if (!versionPattern.test(beat.appVersion as string)) return 'Invalid appVersion'
  return validateBuildMode(beat.buildMode) ?? validateConfig(beat.config)
}

/** Read and parse the request body, enforcing the size cap. Returns the parsed object or an error. */
async function readHeartbeatBody(c: Context<{ Bindings: Bindings }>): Promise<Record<string, unknown> | Response> {
  const contentLength = c.req.header('content-length')
  if (contentLength && parseInt(contentLength, 10) > maxHeartbeatBytes) {
    return c.json({ error: 'Heartbeat too large' }, 400)
  }

  let rawBody: string
  try {
    rawBody = await c.req.text()
  } catch {
    return c.json({ error: 'Could not read request body' }, 400)
  }
  if (rawBody.length > maxHeartbeatBytes) {
    return c.json({ error: 'Heartbeat too large' }, 400)
  }

  let parsed: unknown
  try {
    parsed = JSON.parse(rawBody)
  } catch {
    return c.json({ error: 'Invalid JSON' }, 400)
  }
  if (!parsed || typeof parsed !== 'object') {
    return c.json({ error: 'Invalid JSON' }, 400)
  }
  return parsed as Record<string, unknown>
}

heartbeat.post('/heartbeat', async (c) => {
  // Rate-limit by the caller IP before doing any work. The IP keys the limiter's sliding window
  // only and is never stored.
  const limited = await enforceIpRateLimit(c.env.HEARTBEAT_LIMITER, c.req)
  if (limited) return limited

  const parsed = await readHeartbeatBody(c)
  if (parsed instanceof Response) return parsed

  const validationError = validateHeartbeatShape(parsed)
  if (validationError) {
    return c.json({ error: validationError }, 400)
  }
  const beat = parsed as unknown as Heartbeat

  // The config blob is stored verbatim as a single JSON column (not per-field columns), so new
  // settings auto-absorb without a migration. We re-serialize to a canonical string for storage.
  const configJson = beat.config !== undefined && beat.config !== null ? JSON.stringify(beat.config) : null

  // Write to D1 (fire-and-forget). `build_mode` and `config_json` are nullable.
  const dbWrite = c.env.TELEMETRY_DB.prepare(
    `INSERT INTO heartbeat (anal_id, app_version, os_version, arch, build_mode, config_json)
         VALUES (?, ?, ?, ?, ?, ?)`,
  )
    .bind(beat.analId, beat.appVersion, beat.osVersion, beat.arch, beat.buildMode ?? null, configJson)
    .run()
    .catch(() => {}) // Don't let D1 failure block the response

  try {
    c.executionCtx.waitUntil(dbWrite)
  } catch {
    // executionCtx unavailable (for example, in tests); await inline as fallback
    await dbWrite
  }

  return c.body(null, 204)
})

export { heartbeat }
