import { cloudflareTest } from '@cloudflare/vitest-pool-workers'
import { defineConfig } from 'vitest/config'

/**
 * Tests that run inside workerd instead of Node, under this Worker's own `wrangler.toml`, so the
 * compatibility date is production's and the bindings behave like the real ones.
 *
 * **This project can't catch a Node-only global.** The pool needs Node APIs to run Vitest itself
 * inside workerd, so it force-enables `nodejs_compat_v2` on the test worker: `Buffer` and
 * `process` exist here whatever `wrangler.toml` says (verified with a probe test on
 * `@cloudflare/vitest-pool-workers` 0.22.0, 2026-09-16). `src/licensing/production-runtime.test.ts`
 * is what guards that, by running the real Worker under the real flags.
 */
export const workerdTests = ['src/licensing/license.test.ts', 'src/licensing/paddle.test.ts']

export default defineConfig({
  plugins: [cloudflareTest({ wrangler: { configPath: './wrangler.toml' } })],
  test: {
    name: 'workerd',
    include: workerdTests,
  },
})
