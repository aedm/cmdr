import type { PageServerLoad } from './$types'
import { resolveEnv } from '$lib/server/fetch-all.js'
import { fetchLicenses } from '$lib/server/sources/licenses.js'
import type { LicenseListing } from '$lib/licenses.js'

const emptyListing: LicenseListing = { licenses: [], orphanCodes: [], missingCodes: [] }

/**
 * Loads the live license ledger from the api-server admin endpoint. The admin token resolves
 * server-side (`resolveEnv`) and never reaches the browser; the page gets only the listing and a load
 * error string. No caching: this view has to reflect the live ledger (see the source module).
 *
 * On a failure the page renders the error, never the empty listing: "no licenses" and "couldn't ask"
 * are opposite answers, and the second must not read as an all-clear.
 */
export const load: PageServerLoad = async ({ platform }) => {
  const env = await resolveEnv(platform)
  const result = await fetchLicenses(env)
  return {
    listing: result.ok ? result.data : emptyListing,
    loadError: result.ok ? null : result.error,
  }
}
