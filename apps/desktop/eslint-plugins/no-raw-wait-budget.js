/**
 * ESLint rule: ban a raw numeric wait budget in the Playwright E2E suite.
 *
 * Rationale: every `{ timeout: 5000 }` is a bet on how much CPU the machine has
 * spare. The macOS lane runs on a box somebody is working on, so those bets lose
 * and the suite goes RED where it should only have gone SLOW. `waitBudget(N)`
 * (`test/e2e-playwright/wait-budget.ts`) scales each budget by the load the check
 * runner measured at lane start, so a busy machine buys patience instead of
 * failures.
 *
 * What this rule flags, inside `test/e2e-playwright/` only:
 *   { timeout: 5000 }               // a per-call wait budget
 *   { timeout: 15_000 }             // numeric separators are still a literal
 *   test.setTimeout(90_000)         // a per-test ceiling
 *   testInfo.setTimeout(1000)       // same, from a fixture
 *   pollUntil(page, ready, 3000)    // the same budget spelled positionally
 *   const NATIVE_TIMEOUT_MS = 15000 // a budget behind a name
 *   function f(timeoutMs = 10000)   // a budget behind a parameter default
 *   options.uiTimeout ?? 10000      // a budget behind a nullish fallback
 *
 * The per-test ceiling is in scope for a reason: a wait that scales to 60 s under
 * a ceiling pinned at 15 s just dies at the ceiling instead of at the wait, with a
 * worse message and nothing gained. Both halves scale, or neither does.
 *
 * What it leaves alone:
 *   { timeout: NATIVE_TIMEOUT_MS }        // a name whose declaration already scales
 *   { timeoutMs: 30000 }                  // a PROPERTY that merely contains "timeout":
 *                                         // in an `evaluate()` template that's the backend's
 *                                         // own connect timeout, written in webview JS
 *   pollUntil(page, ready, budget, 20)    // the fourth argument is a poll INTERVAL, not a budget
 *   setTimeout(resolve, 500)              // the global sleep; `no-arbitrary-sleep-in-e2e` owns it
 *   type Options = { timeout: number }    // a type annotation
 *
 * Opt out per-line, with a reason, when the number itself is what a test proves
 * (a ceiling under measurement, a fail-fast budget that must stay small):
 *
 *   // eslint-disable-next-line cmdr/no-raw-wait-budget -- <reason>
 *   testInfo.setTimeout(DEAD_APP_BUDGET_MS)
 *
 * See `test/e2e-playwright/DETAILS.md` § "The load-scaled wait budget".
 */

/** The one directory this rule governs. Normalized to forward slashes before the test. */
const E2E_DIR = 'test/e2e-playwright/'

function isInE2eSuite(context) {
  const filename = context.filename ?? context.getFilename?.() ?? ''
  return filename.replace(/\\/g, '/').includes(E2E_DIR)
}

/** A plain numeric literal, with the source text (`15_000`) preserved for the message. */
function numericLiteral(node) {
  if (!node || node.type !== 'Literal' || typeof node.value !== 'number') return null
  return node.raw ?? String(node.value)
}

/** The property name for `timeout` and `'timeout'`, and nothing else. */
function staticKeyName(node) {
  if (node.computed) return null
  const key = node.key
  if (key.type === 'Identifier') return key.name
  if (key.type === 'Literal' && typeof key.value === 'string') return key.value
  return null
}

/** `timeoutMs`, `NATIVE_TIMEOUT_MS`, `options.uiTimeout`: anything that reads as a budget. */
function readsAsBudgetName(text) {
  return /timeout/i.test(text)
}

/** @type {import('eslint').Rule.RuleModule} */
export default {
  meta: {
    type: 'problem',
    docs: {
      description:
        'Route E2E wait budgets through `waitBudget(N)` so they scale with machine load. See `test/e2e-playwright/wait-budget.ts`.',
      recommended: true,
    },
    messages: {
      rawWaitBudget:
        '`timeout: {{ ms }}` is a bet on how much CPU this machine has spare, and on a busy one it loses: the suite ' +
        'goes red where it should only have gone slow. Write `timeout: waitBudget({{ ms }})` instead, importing ' +
        '`waitBudget` from `wait-budget.ts`: it stretches the budget by `CMDR_E2E_WAIT_SCALE`, which the check ' +
        'runner sets from the measured load. Opt out per-line with ' +
        '`// eslint-disable-next-line cmdr/no-raw-wait-budget -- <reason>` when the number itself is the thing ' +
        'under test.',
      rawTestCeiling:
        '`setTimeout({{ ms }})` pins the per-test ceiling while the waits underneath it scale, so the test dies at ' +
        'the ceiling instead of at the wait it was really waiting on. Write `setTimeout(waitBudget({{ ms }}))` ' +
        'instead, importing `waitBudget` from `wait-budget.ts`. Opt out per-line with ' +
        '`// eslint-disable-next-line cmdr/no-raw-wait-budget -- <reason>` when the ceiling has to stay fixed.',
      rawNamedBudget:
        'A wait budget behind a name is still a wait budget, and `{{ ms }}` here is a bet on how much CPU this ' +
        'machine has spare. Wrap it where it is written, as `waitBudget({{ ms }})` (from `wait-budget.ts`), so ' +
        'every call site that reads this name scales with the machine. Opt out per-line with ' +
        '`// eslint-disable-next-line cmdr/no-raw-wait-budget -- <reason>` when the number itself is the thing ' +
        'under test.',
    },
    schema: [],
  },
  create(context) {
    if (!isInE2eSuite(context)) return {}

    return {
      Property(node) {
        if (staticKeyName(node) !== 'timeout') return
        const ms = numericLiteral(node.value)
        if (ms === null) return
        context.report({ node: node.value, messageId: 'rawWaitBudget', data: { ms } })
      },

      CallExpression(node) {
        // Member form only: the bare global `setTimeout(fn, ms)` takes a callback first, so
        // it never reaches the literal check anyway, and it belongs to `no-arbitrary-sleep-in-e2e`.
        const callee = node.callee
        if (callee.type !== 'MemberExpression' || callee.computed) return
        if (callee.property.type !== 'Identifier' || callee.property.name !== 'setTimeout') return
        const ms = numericLiteral(node.arguments[0])
        if (ms === null) return
        context.report({ node: node.arguments[0], messageId: 'rawTestCeiling', data: { ms } })
      },

      // `pollUntil(page, condition, timeout, interval?)`: the third argument is the budget,
      // the fourth is the poll interval and stays raw.
      'CallExpression[callee.name="pollUntil"]'(node) {
        const ms = numericLiteral(node.arguments[2])
        if (ms === null) return
        context.report({ node: node.arguments[2], messageId: 'rawWaitBudget', data: { ms } })
      },

      // `const NATIVE_TIMEOUT_MS = 15000`, `function f(timeoutMs = 10000)`.
      'VariableDeclarator, AssignmentPattern'(node) {
        const target = node.type === 'VariableDeclarator' ? node.id : node.left
        const value = node.type === 'VariableDeclarator' ? node.init : node.right
        if (target.type !== 'Identifier' || !readsAsBudgetName(target.name)) return
        const ms = numericLiteral(value)
        if (ms === null) return
        context.report({ node: value, messageId: 'rawNamedBudget', data: { ms } })
      },

      // `options.uiTimeout ?? 10000`.
      'LogicalExpression[operator="??"]'(node) {
        const ms = numericLiteral(node.right)
        if (ms === null) return
        const source = context.sourceCode ?? context.getSourceCode?.()
        if (!source || !readsAsBudgetName(source.getText(node.left))) return
        context.report({ node: node.right, messageId: 'rawNamedBudget', data: { ms } })
      },
    }
  },
}
