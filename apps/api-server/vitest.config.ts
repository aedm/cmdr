import { defineConfig } from 'vitest/config'
import { workerdTests } from './vitest.workerd.config.ts'

/**
 * Two projects, two runtimes: `workerd` runs the licensing tests inside the Worker runtime
 * (`vitest.workerd.config.ts`, which owns the list), `node` runs everything else. Moving a test
 * across means adding it to `workerdTests`; the exclude below follows.
 */
export default defineConfig({
  test: {
    // The check runner sets VITEST_JSON_REPORT to a private per-invocation path so it
    // can log which individual tests failed and render a red run as the failures
    // rather than the whole transcript (scripts/check/checks/vitest-failure-diagnostics.go).
    // Reporters are a root-level option: the projects below inherit these.
    reporters: process.env.VITEST_JSON_REPORT
      ? ['default', ['json', { outputFile: process.env.VITEST_JSON_REPORT }]]
      : ['default'],
    projects: [
      {
        test: {
          name: 'node',
          include: ['src/**/*.test.ts'],
          exclude: workerdTests,
          environment: 'node',
        },
      },
      './vitest.workerd.config.ts',
    ],
  },
})
