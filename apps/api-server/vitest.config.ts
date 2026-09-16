import { defineConfig } from 'vitest/config'
import { workerdTests } from './vitest.workerd.config.ts'

/**
 * Two projects, two runtimes: `workerd` runs the licensing tests inside the Worker runtime
 * (`vitest.workerd.config.ts`, which owns the list), `node` runs everything else. Moving a test
 * across means adding it to `workerdTests`; the exclude below follows.
 */
export default defineConfig({
  test: {
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
