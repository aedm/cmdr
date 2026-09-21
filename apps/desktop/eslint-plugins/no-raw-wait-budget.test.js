import { RuleTester } from 'eslint'
import tseslint from 'typescript-eslint'
import rule from './no-raw-wait-budget.js'

// Flat-config RuleTester (ESLint 9+) on the typescript-eslint parser: the suite this rule
// guards is all TypeScript, and a `timeout: number` TYPE annotation has to stay valid, which
// espree can't even parse. RuleTester auto-detects Vitest's `describe`/`it` globals and emits
// one test per case, so `run` is called at the top level (it can't be nested inside our own `it`).
const ruleTester = new RuleTester({
  languageOptions: { parser: tseslint.parser, ecmaVersion: 'latest', sourceType: 'module' },
})

const SPEC = 'apps/desktop/test/e2e-playwright/file-operations.spec.ts'
const HELPER = 'apps/desktop/test/e2e-playwright/helpers/app-lifecycle.ts'
const CONFIG = 'apps/desktop/test/e2e-playwright/playwright.config.ts'

ruleTester.run('no-raw-wait-budget', rule, {
  valid: [
    // The intended shape: the literal goes through the scaler.
    {
      code: `await tauriPage.waitForSelector('.x', { timeout: waitBudget(5000) })`,
      filename: SPEC,
    },
    // Per-test ceilings take the same route.
    {
      code: `test.setTimeout(waitBudget(90_000))`,
      filename: SPEC,
    },
    {
      code: `testInfo.setTimeout(waitBudget(DEAD_APP_TEST_BUDGET_MS))`,
      filename: SPEC,
    },
    // A named constant or an imported budget is already out of the "raw literal" class.
    {
      code: `const opts = { timeout: NATIVE_TIMEOUT_MS }`,
      filename: SPEC,
    },
    {
      code: `const opts = { timeout: options.uiTimeout ?? waitBudget(10000) }`,
      filename: HELPER,
    },
    // The global `setTimeout(fn, ms)` is a sleep, governed by `no-arbitrary-sleep-in-e2e`.
    {
      code: `await new Promise((resolve) => setTimeout(resolve, 500))`,
      filename: SPEC,
    },
    {
      code: `setTimeout(finish, 500)`,
      filename: HELPER,
    },
    // App code keeps its own timeouts; this rule is about the E2E suite's wait budgets.
    {
      code: `const opts = { timeout: 5000 }`,
      filename: 'apps/desktop/src/lib/file-operations/transfer.ts',
    },
    {
      code: `test.setTimeout(30_000)`,
      filename: 'apps/desktop/test/unit/something.test.ts',
    },
    {
      code: `type Options = { timeout: number }`,
      filename: HELPER,
    },
    // Poll INTERVALS aren't budgets: a tighter one costs a few more cheap checks, never a
    // failure. `pollUntil`'s fourth argument stays raw.
    {
      code: `await pollUntil(page, ready, waitBudget(3000), 20)`,
      filename: SPEC,
    },
    // A property whose name merely CONTAINS "timeout" is left alone: `timeoutMs: 30000`
    // inside an `evaluate()` template is the backend's own connect timeout, in webview JS.
    {
      code: `await page.evaluate(\`invoke('list_shares', { timeoutMs: 30000 })\`)`,
      filename: SPEC,
    },
    {
      code: `await waitForTransferUiToSettle(page, { uiTimeout: 200 })`,
      filename: HELPER,
    },
    // Named budgets are fine once the name resolves through the scaler.
    {
      code: `const NATIVE_TIMEOUT_MS = waitBudget(15000)`,
      filename: HELPER,
    },
    // The documented per-line opt-out suppresses a real violation. RuleTester registers the
    // rule under its own `rule-to-test/` prefix, so the directive names that here; in the repo
    // it reads `cmdr/no-raw-wait-budget`, as the rule's message says.
    {
      code: `// eslint-disable-next-line rule-to-test/no-raw-wait-budget -- the ceiling itself is under measurement\nconst opts = { timeout: 1000 }`,
      filename: SPEC,
    },
    {
      code: `// eslint-disable-next-line rule-to-test/no-raw-wait-budget -- fail-fast budget, must stay small\ntestInfo.setTimeout(1000)`,
      filename: HELPER,
    },
  ],
  invalid: [
    // The common case: a per-call wait budget written as a bare number.
    {
      code: `await tauriPage.waitForSelector('.x', { timeout: 5000 })`,
      filename: SPEC,
      errors: [{ messageId: 'rawWaitBudget', data: { ms: '5000' } }],
    },
    // Numeric separators are still a numeric literal.
    {
      code: `await expect.poll(read, { timeout: 15_000 }).toBeTruthy()`,
      filename: SPEC,
      errors: [{ messageId: 'rawWaitBudget' }],
    },
    // A quoted key is the same property.
    {
      code: `const opts = { 'timeout': 3000 }`,
      filename: SPEC,
      errors: [{ messageId: 'rawWaitBudget' }],
    },
    // Helper modules are in scope too: they hold most of the shared budgets.
    {
      code: `export const settleOptions = { timeout: 2000 }`,
      filename: HELPER,
      errors: [{ messageId: 'rawWaitBudget' }],
    },
    // The per-test ceiling has to scale with the per-call waits, or the test just
    // dies at the ceiling instead of at the wait.
    {
      code: `test.setTimeout(90_000)`,
      filename: SPEC,
      errors: [{ messageId: 'rawTestCeiling', data: { ms: '90_000' } }],
    },
    {
      code: `testInfo.setTimeout(1000)`,
      filename: HELPER,
      errors: [{ messageId: 'rawTestCeiling' }],
    },
    // A describe-level ceiling is an object property, not a call.
    {
      code: `test.describe.configure({ timeout: 180000 })`,
      filename: SPEC,
      errors: [{ messageId: 'rawWaitBudget' }],
    },
    // The config's own ceiling is the other half of the pair.
    {
      code: `export default defineConfig({ timeout: 15000 })`,
      filename: CONFIG,
      errors: [{ messageId: 'rawWaitBudget' }],
    },
    // Every raw budget on a line gets its own report.
    {
      code: `const both = { a: { timeout: 3000 }, b: { timeout: 5000 } }`,
      filename: SPEC,
      errors: [{ messageId: 'rawWaitBudget' }, { messageId: 'rawWaitBudget' }],
    },
    // `pollUntil`'s third argument is the same budget spelled positionally.
    {
      code: `await pollUntil(page, ready, 3000)`,
      filename: SPEC,
      errors: [{ messageId: 'rawWaitBudget', data: { ms: '3000' } }],
    },
    {
      code: `await pollUntil(page, ready, 6000, 20)`,
      filename: HELPER,
      errors: [{ messageId: 'rawWaitBudget' }],
    },
    // A budget hidden behind a name is still a budget: wrap it at the declaration, where
    // it is written once for every call site that reads it.
    {
      code: `const NATIVE_TIMEOUT_MS = 15000`,
      filename: HELPER,
      errors: [{ messageId: 'rawNamedBudget', data: { ms: '15000' } }],
    },
    {
      code: `async function waitForIndexData(page, dir, timeoutMs = 500_000) {}`,
      filename: SPEC,
      errors: [{ messageId: 'rawNamedBudget' }],
    },
    {
      code: `await expect.poll(read, { timeout: options.uiTimeout ?? 10000 }).toEqual([])`,
      filename: HELPER,
      errors: [{ messageId: 'rawNamedBudget' }],
    },
    // Windows-style separators still resolve to the E2E directory.
    {
      code: `const opts = { timeout: 5000 }`,
      filename: 'apps\\desktop\\test\\e2e-playwright\\app.spec.ts',
      errors: [{ messageId: 'rawWaitBudget' }],
    },
  ],
})
