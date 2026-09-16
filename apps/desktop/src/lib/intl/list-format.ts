/**
 * Locale-aware "A, B, and C" joining, memoized.
 *
 * The one place a list of names becomes a phrase inside a sentence, so no
 * feature module hand-joins with `', '` and a hardcoded English "and". Today's
 * caller is the eject refusal that names the apps still holding a drive
 * (`$lib/file-explorer/navigation/eject-error-messages.ts`).
 *
 * The locale is {@link getUiLocale}, not the OS formatting one: this is a
 * conjunction inside catalog copy, so it has to speak the language the sentence
 * around it speaks. That's the split `./locale.ts` draws, and it's why sizes and
 * dates resolve the other way.
 *
 * Memoized per locale for the same reason `number-format.ts` is: constructing an
 * `Intl` formatter costs far more than formatting with one.
 */

import { getUiLocale } from './locale'

/** Cache of one conjunction formatter per UI locale. */
const listFormatterCache = new Map<string, Intl.ListFormat>()

/** A memoized conjunction `Intl.ListFormat` for the active UI locale. */
function getConjunctionFormatter(): Intl.ListFormat {
  const locale = getUiLocale()
  let formatter = listFormatterCache.get(locale)
  if (formatter === undefined) {
    formatter = new Intl.ListFormat(locale, { style: 'long', type: 'conjunction' })
    listFormatterCache.set(locale, formatter)
  }
  return formatter
}

/**
 * Join names the way the UI language joins them: `Preview and Warp`,
 * `Preview, Warp, and Photos`. An empty list answers an empty string, which is
 * the caller's cue that it has no list sentence to say.
 */
export function formatConjunctionList(items: readonly string[]): string {
  if (items.length === 0) return ''
  return getConjunctionFormatter().format(items)
}

/** Test seam: drop the memoization cache so a memoization assertion starts clean. */
export function _clearListFormatCacheForTests(): void {
  listFormatterCache.clear()
}
