import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['src/**/*.test.ts'],
    environment: 'node',
    // The check runner sets VITEST_JSON_REPORT to a private per-invocation path so it
    // can log which individual tests failed and render a red run as the failures
    // rather than the whole transcript (scripts/check/checks/vitest-failure-diagnostics.go).
    reporters: process.env.VITEST_JSON_REPORT
      ? ['default', ['json', { outputFile: process.env.VITEST_JSON_REPORT }]]
      : ['default'],
  },
})
