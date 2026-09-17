import type { SourceResult } from '../types.js'
import type { LicenseListing } from '../../licenses.js'
import { fetchWorkerEndpoint } from './worker-endpoint.js'

/**
 * Server-only read of the api-server's license ledger (`GET /admin/licenses`): every license we've
 * issued, bought or handed out, plus the codes the ledger and the key store disagree about. The
 * admin bearer token (`LICENSE_SERVER_ADMIN_TOKEN`) stays in this module and `+page.server.ts`; it
 * never reaches the browser.
 *
 * No caching, like the `/links` admin list: David opens this page exactly when he's mid-delivery or
 * has just handed a license out, and a five-minute-stale "not emailed" would send him chasing a
 * problem that's already fixed. The list is tiny and the endpoint is one D1 read plus a KV scan.
 */

interface LicensesEnv {
  LICENSE_SERVER_ADMIN_TOKEN: string
  /** Optional local-QA override for the api-server base URL (defaults to production). */
  WORKER_BASE_URL?: string
}

export async function fetchLicenses(env: LicensesEnv): Promise<SourceResult<LicenseListing>> {
  try {
    const data = await fetchWorkerEndpoint<LicenseListing>(
      env.LICENSE_SERVER_ADMIN_TOKEN,
      '/admin/licenses',
      env.WORKER_BASE_URL,
    )
    return { ok: true, data }
  } catch (e) {
    return { ok: false, error: `Licenses: ${e instanceof Error ? e.message : String(e)}` }
  }
}

/**
 * Rewrite one license's note (`PUT /admin/licenses/:transactionId/note`). The api-server owns the
 * rules: a hand-issued license can't be left without one, and an empty note clears a purchase's.
 * Its message comes back verbatim so the page shows what the server actually objected to.
 */
export async function updateLicenseNote(
  env: LicensesEnv,
  input: { transactionId: string; note: string },
): Promise<SourceResult<string | null>> {
  const baseUrl = env.WORKER_BASE_URL || 'https://api.getcmdr.com'
  try {
    const response = await fetch(`${baseUrl}/admin/licenses/${encodeURIComponent(input.transactionId)}/note`, {
      method: 'PUT',
      headers: {
        Authorization: `Bearer ${env.LICENSE_SERVER_ADMIN_TOKEN}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({ note: input.note }),
    })
    const text = await response.text()
    if (!response.ok) throw new Error(errorMessage(text) ?? `returned ${String(response.status)}`)
    return { ok: true, data: (JSON.parse(text) as { note: string | null }).note }
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) }
  }
}

/** The worker's `{ error }` string, or null when the body isn't the shape we expect. */
function errorMessage(body: string): string | null {
  try {
    const parsed = JSON.parse(body) as { error?: unknown }
    return typeof parsed.error === 'string' ? parsed.error : null
  } catch {
    return null
  }
}
