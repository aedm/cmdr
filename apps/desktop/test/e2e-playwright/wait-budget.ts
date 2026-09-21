/**
 * Load-scaled wait budgets for the Playwright E2E suite.
 *
 * Every `{ timeout: N }` in this suite is a bet on how much CPU the machine has spare. On a box somebody is working on,
 * those bets lose, and the suite goes RED where it should only have gone SLOW. `waitBudget(N)` turns each one into a
 * budget that stretches with the machine's load.
 *
 * ## The contract with the check runner
 *
 * - **Variable**: `CMDR_E2E_WAIT_SCALE`, a decimal multiplier.
 * - **Range**: clamped to `[1, 4]`. Unset, empty, or unparseable reads as `1`.
 * - **Meaning**: `1` is "the machine is idle, use the literal budgets"; `4` is "four times the patience".
 * - **Who sets it**: the Go check runner exports it once, at lane start, from the machine's measured load
 *   (`scripts/check/`). Nothing else writes it, and nothing has to. Unset means `1`, so a hand `npx playwright test`
 *   and the Linux Docker lane behave exactly as they did before this existed.
 *
 * The scale is read once at module load: it describes the machine the lane started on, not a value that moves
 * mid-run.
 *
 * ## Both halves scale, or neither does
 *
 * A per-call wait that stretches to 15 s under a per-test ceiling that stays at 15 s just dies at the ceiling instead
 * of at the wait, with a worse message and nothing gained. So `playwright.config.ts`'s `timeout`, every
 * `test.describe.configure({ timeout })`, and every `test.setTimeout(...)` go through this too.
 *
 * ## The absolute cap
 *
 * Scaling stops at `CAP_MS`. A budget already in the minutes isn't a CPU-availability bet; it covers work that is
 * genuinely long (the marketing capture's 15-minute shot wait, the 3-minute dialog-inset sweep). Quadrupling those
 * buys nothing but a longer wait before the same diagnosis. The cap never SHRINKS a budget: anything already at or
 * above it passes through untouched.
 *
 * Rationale and the lane-side measurement: `DETAILS.md` § "The load-scaled wait budget".
 */

/** Below this the budgets are the literals, unscaled. */
const SCALE_MIN = 1

/** Past 4x the suite is slower than a rerun, so more patience stops being the cheaper trade. */
const SCALE_MAX = 4

/** Scaling ceiling: 10 minutes. See § "The absolute cap" above. */
const CAP_MS = 600_000

function resolveScale(): number {
  const raw = process.env.CMDR_E2E_WAIT_SCALE
  if (raw === undefined || raw.trim() === '') return SCALE_MIN
  const parsed = Number.parseFloat(raw)
  if (!Number.isFinite(parsed)) return SCALE_MIN
  return Math.min(SCALE_MAX, Math.max(SCALE_MIN, parsed))
}

/**
 * The resolved multiplier, in `[1, 4]`. `1` means every budget is its literal value.
 *
 * Exported so a run can say which regime it's in; `global-setup.ts` prints it when it isn't 1.
 */
export const WAIT_SCALE = resolveScale()

/**
 * Scales one wait budget for the current machine's load.
 *
 * `waitBudget(5000)` is 5 s on an idle box and 20 s at the maximum scale. Use it for every numeric wait budget in this
 * suite: `{ timeout: waitBudget(5000) }`, `test.setTimeout(waitBudget(90_000))`. The `no-raw-wait-budget` ESLint rule
 * fails a raw number.
 */
export function waitBudget(ms: number): number {
  if (WAIT_SCALE === SCALE_MIN) return ms
  return Math.max(ms, Math.min(Math.round(ms * WAIT_SCALE), CAP_MS))
}
