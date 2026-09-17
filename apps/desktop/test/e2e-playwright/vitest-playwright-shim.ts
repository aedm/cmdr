/**
 * What `@playwright/test` and `@srsholmes/tauri-playwright` resolve to inside the
 * Vitest run (aliased in `vitest.config.ts`). The Playwright suite itself is
 * unaffected: `*.spec.ts` files run under Playwright's own runner, which never
 * sees these aliases.
 *
 * ❗ Why this exists. The E2E helper modules import `expect` for `expect(...)` and
 * `expect.poll(...)`, and their own unit tests (`helpers/*.test.ts`,
 * `i18n-capture-stage-only.test.ts`) import those helpers to exercise them for
 * real. That dragged the Playwright test runner into a happy-dom worker, where it
 * fires an HTTP request at the page's own origin and dies on an unhandled
 * `'error'` event — a whole Node process gone, five per full run, each printing a
 * `node:events:505 throw er` crash dump into the lane's output while the tests
 * around it still reported green. Reproduced down to a six-line file, and the
 * target port follows `environmentOptions.happyDOM.url`, so the page origin is
 * where it aims (vitest 4.1.10, happy-dom 20.11.1, @playwright/test 1.62.1,
 * @srsholmes/tauri-playwright 0.4.1, 2026-09-17).
 *
 * Vitest's own `expect` carries the same two entry points with the same
 * semantics, including `expect.poll(fn, { timeout }).toBe(...)`, so the helpers
 * under test still run their real code against a real polling matcher. Nothing
 * here paraphrases a helper.
 *
 * ❗ Keep the export surface minimal. Anything a Vitest-loaded module needs has to
 * be added here deliberately; a missing one fails the build naming this file,
 * which is the point. If a helper ever needs one of the Tauri matchers
 * `createTauriTest` adds (`toBeVisible` and friends, which need a live app), that
 * helper belongs behind a seam its unit test can reach without loading the
 * runner — ❌ don't widen this into a re-export of the real packages.
 */

import { expect } from 'vitest'

export { expect }

/**
 * A stand-in for Playwright's `test`. `fixtures.ts` builds the suite's real `test`
 * from it at MODULE scope (`baseTest.extend({...})`, `test.describe`, …), and that
 * happens the moment a Vitest-run helper imports the module, so the builder
 * surface has to keep working and hand back something test-shaped.
 *
 * DECLARING a test is fine, then; RUNNING one under Vitest is the mistake, so
 * that's what throws, by name, instead of failing somewhere downstream.
 */
function inertTest(): unknown {
  // A member (`.extend`, `.describe`, `.use`, …) is callable and yields a test
  // again, so any chain of them terminates back at a test-shaped value.
  const member: ProxyHandler<() => void> = {
    get: (): unknown => new Proxy(() => {}, member),
    apply: (): unknown => root,
  }
  const root: unknown = new Proxy(() => {}, {
    get: (): unknown => new Proxy(() => {}, member),
    apply: () => {
      throw new Error(
        'Playwright `test` was called inside the Vitest run. A `*.spec.ts` belongs to the Playwright ' +
          'lane (`pnpm check desktop-e2e-playwright`); see test/e2e-playwright/vitest-playwright-shim.ts.',
      )
    },
  })
  return root
}

/**
 * Stands in for `@srsholmes/tauri-playwright`'s factory, which `fixtures.ts` calls
 * at module scope. Returns the same two names so that module still loads.
 */
export function createTauriTest(): { test: unknown; expect: typeof expect } {
  return { test: inertTest(), expect }
}
