/**
 * The main window's startup, as a list of independent steps run in order.
 *
 * Each step is awaited before the next one starts (a later step may read what an earlier one
 * set up), and each runs inside its own catch, so a step that throws costs only itself. Without
 * that, one rejected `await` in the layout's init skipped every step after it on every launch:
 * the AI config push, accent color, text size, shortcuts, the MCP bridges, the crash-report
 * check, the update checker, and AI state.
 *
 * A step that throws is logged at error: every step handles its own outside failures, so
 * reaching this catch means Cmdr broke.
 */

import { getAppLogger } from '$lib/logging/logger'

const log = getAppLogger('startup')

export interface InitStep {
  /** Names the step in the log line when it throws. */
  name: string
  run: () => unknown
}

export async function runInitSteps(steps: readonly InitStep[]): Promise<void> {
  for (const step of steps) {
    try {
      await step.run()
    } catch (error) {
      log.error('Startup step {step} threw; the steps after it still run: {error}', { step: step.name, error })
    }
  }
}
