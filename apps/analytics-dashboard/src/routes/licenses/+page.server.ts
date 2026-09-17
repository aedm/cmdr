import type { PageServerLoad, Actions } from './$types'
import { fail } from '@sveltejs/kit'
import { resolveEnv } from '$lib/server/fetch-all.js'
import { fetchLicenses, updateLicenseNote } from '$lib/server/sources/licenses.js'
import { validateNote, type LicenseListing } from '$lib/licenses.js'

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

/** Reads one text field off a submitted form, treating a non-string entry (a file) as absent. */
function textField(form: FormData, name: string): string {
  const value = form.get(name)
  return typeof value === 'string' ? value : ''
}

/**
 * The note editor. It proxies to the api-server with the server-only bearer token, then lets the
 * page reload so the table shows what actually landed rather than what was typed. A rejection comes
 * back with the submitted note, so the dialog can reopen holding the person's words.
 */
export const actions: Actions = {
  saveNote: async ({ request, platform }) => {
    const form = await request.formData()
    const raw = { transactionId: textField(form, 'transactionId'), note: textField(form, 'note') }

    const validated = validateNote(raw)
    if (!validated.ok) return fail(400, { ...raw, error: validated.error })

    const env = await resolveEnv(platform)
    const result = await updateLicenseNote(env, { transactionId: validated.transactionId, note: validated.note })
    if (!result.ok) return fail(502, { ...raw, error: result.error })

    return { transactionId: validated.transactionId, saved: true }
  },
}
