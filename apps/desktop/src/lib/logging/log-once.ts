/**
 * Logs a repeating condition once, until it clears.
 *
 * Warn and above reach the log file and every error-report bundle, so a failure
 * that recurs on each event, poll tick, or call (a store read on every
 * `volumes-changed`, a probe on a 500 ms interval, an agent retrying the same
 * navigation) would write the same line every time and bury the rest of the log.
 * Ask `shouldLog(condition)` on the failure path, and `clear()` once the thing
 * works again so the next breakage speaks.
 *
 * @example
 * const readFailures = new LogOnceGate()
 * try {
 *   servers = await listSavedServers()
 *   readFailures.clear()
 * } catch (e) {
 *   if (readFailures.shouldLog(String(e))) log.warn('Reading the saved servers broke down: {error}', { error: String(e) })
 * }
 */

/**
 * Past this many distinct conditions a gate forgets them all rather than
 * holding every key forever. A condition then logs one more time, which costs a
 * line; an unbounded set would cost memory for as long as the failure keeps
 * minting new keys (an agent trying ever-different paths).
 */
export const MAX_TRACKED_CONDITIONS = 100

export class LogOnceGate {
  readonly #logged = new Set<string>()

  /** True the first time `condition` occurs since it last cleared. */
  shouldLog(condition = ''): boolean {
    if (this.#logged.has(condition)) return false
    if (this.#logged.size >= MAX_TRACKED_CONDITIONS) this.#logged.clear()
    this.#logged.add(condition)
    return true
  }

  /** The condition went away, so its next occurrence logs. With no argument, every condition clears. */
  clear(condition?: string): void {
    if (condition === undefined) this.#logged.clear()
    else this.#logged.delete(condition)
  }
}
