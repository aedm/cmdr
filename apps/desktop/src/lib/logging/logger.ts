/**
 * Logging configuration using LogTape.
 *
 * Usage:
 *   import { getAppLogger } from '$lib/logging/logger'
 *   const log = getAppLogger('myFeature')
 *   log.debug('Loading data for {userId}', { userId })
 *   log.info('Loaded {count} {itemsNoun}', { count, itemsNoun: pluralize(count, 'item') })
 *   log.warn('Slow operation: {ms}ms', { ms })
 *   log.error('Failed to load: {error}', { error })
 *
 * Log levels (in order): debug < info < warning < error < fatal
 *
 * Default behavior:
 *   - Dev mode: info+ in browser console, debug+ sent to Rust (filtered by RUST_LOG)
 *   - Prod mode: warn+ from every category sent to Rust, so the log file and error-report bundles
 *     carry it (debug+ for debugCategories); error+ in the browser console
 *   - Verbose logging setting: when enabled, all categories get debug level in both sinks
 *
 * To make a feature's debug logs reach production logs and bundles, add it to debugCategories below.
 * To enable debug logs in the terminal, use RUST_LOG: `RUST_LOG=FE:fileExplorer=debug,info`
 * Or enable the "Verbose logging" setting in Developer settings for both sinks.
 *
 * @module logger
 */

import { configure, getConsoleSink, getLogger as getLogTapeLogger, withFilter } from '@logtape/logtape'
import type { Logger, Sink } from '@logtape/logtape'
// eslint-disable-next-line cmdr/no-raw-bindings-import -- logging/store bootstrap infra: the tauri-commands barrel imports the logger (storage.ts), so wrapping here would create an import cycle
import { commands } from '$lib/ipc/bindings'
import { load, type Store } from '@tauri-apps/plugin-store'
import { resolveStorePath } from '$lib/settings/store-path'
import { getTauriBridgeSink, startBridge } from './log-bridge'

export type { Logger } from '@logtape/logtape'

const isDev = import.meta.env.DEV

/**
 * Features that log at debug in a production build too, so error-report bundles
 * carry their debug lines. Every other category reaches production logs from warn up.
 *
 * The browser console keeps its own gate (info in dev), so an entry here doesn't
 * change what devtools shows; the verbose setting does.
 */
export const debugCategories: readonly string[] = [
  // Always-on so error-report bundles capture pane-level diagnostics. Most
  // notably the "dialog didn't open" warn lines in `DualPaneExplorer` (rare,
  // one entry per blocked F2/F7/F8/etc. attempt) plus the existing pane-state
  // debug lines. Volume is low under normal use; tap through if needed.
  'fileExplorer',
  // 'dragDrop',
  // 'licensing',
  // 'copyProgress', // Enable to debug copy operation progress events
  // 'viewer', // Enable to debug file viewer streaming/caching
  'settings', // Enable to debug settings dialog initialization and persistence
  'reactive-settings', // Enable to debug reactive settings updates
  'shortcuts', // Enable to debug keyboard shortcut persistence
  'mtp', // Enable to debug MTP device operations
  // Always-on so error-report bundles capture the user's recent shortcut/menu
  // activity (`FE:user-action edit.copy`, etc.). Volume is small (~one entry per
  // user keystroke) and the breadcrumb-style trail is invaluable when triaging
  // why a shortcut "did nothing." See `routes/(main)/command-dispatch.ts`.
  'user-action',
]

// Track if verbose logging is enabled for reconfiguration
let verboseLoggingEnabled = false
let loggerInitialized = false

/**
 * Read the verbose logging setting directly from the store file.
 * This is needed because the logger initializes before the full settings system.
 */
async function getVerboseLoggingSetting(): Promise<boolean> {
  try {
    // Resolve the store path so isolated instances (dev, per-worktree dev, E2E)
    // don't read the real production `settings.json`. See `settings/store-path.ts`.
    const storePath = await resolveStorePath('settings.json')
    // Use empty defaults since we just want to read existing values
    const store: Store = await load(storePath, {
      autoSave: false,
      defaults: {},
    })
    const value = await store.get<boolean>('developer.verboseLogging')
    return value === true
  } catch {
    // Store doesn't exist yet or can't be read - use default
    return false
  }
}

/**
 * The LogTape configuration for one mode, without applying it.
 *
 * Exported so a test configures LogTape with the real thing, spy sinks standing
 * in for the console and the bridge.
 */
export function buildLoggerConfig({
  isDev,
  verbose,
  consoleSink,
  bridgeSink,
}: {
  isDev: boolean
  verbose: boolean
  consoleSink: Sink
  bridgeSink: Sink
}) {
  // The tauriBridge sink passes debug+ to Rust in dev, where RUST_LOG controls final filtering.
  // This lets `RUST_LOG=FE:fileExplorer=debug,info` work without needing to touch debugCategories.
  // The console sink (browser devtools) filters on its own level, which no logger below lowers.
  const consoleLevel: 'debug' | 'info' | 'error' = verbose ? 'debug' : isDev ? 'info' : 'error'

  const loggers: Array<{
    category: string | string[]
    lowestLevel: 'debug' | 'info' | 'warning' | 'error'
    sinks: Array<'console' | 'tauriBridge'>
    parentSinks?: 'override'
  }> = [
    // Outside dev and verbose, warn+ from every category reaches the bridge, and so the log
    // file and every error-report bundle; info and debug stay behind this gate.
    {
      category: 'app',
      lowestLevel: isDev || verbose ? 'debug' : 'warning',
      sinks: ['console', 'tauriBridge'],
    },
  ]

  // debugCategories reach the bridge at debug in production too. ❗ `override`: LogTape's default
  // `inherit` adds the parent's sinks to these same two for every level both loggers pass, which
  // sent each such line to each sink twice (`logger.test.ts` pins the counts).
  if (!verbose) {
    for (const cat of debugCategories) {
      loggers.push({
        category: ['app', cat],
        lowestLevel: 'debug',
        sinks: ['console', 'tauriBridge'],
        parentSinks: 'override',
      })
    }
  }

  return {
    sinks: {
      console: withFilter(consoleSink, consoleLevel),
      // Bridge: passes everything to Rust. RUST_LOG handles filtering there.
      tauriBridge: bridgeSink,
    },
    loggers,
  }
}

/**
 * Build and apply logger configuration.
 * @param verbose - Whether to enable debug logging for all categories
 * @param isReset - Whether this is a reconfiguration (requires reset flag)
 */
async function applyLoggerConfig(verbose: boolean, isReset: boolean): Promise<void> {
  await configure({
    ...buildLoggerConfig({ isDev, verbose, consoleSink: getConsoleSink(), bridgeSink: getTauriBridgeSink() }),
    reset: isReset,
  })
}

/**
 * Initialize the logging system. Call once at app startup.
 */
export async function initLogger(): Promise<void> {
  if (loggerInitialized) {
    return
  }

  // Read verbose logging setting from store (before full settings system is up)
  verboseLoggingEnabled = await getVerboseLoggingSetting()

  await applyLoggerConfig(verboseLoggingEnabled, false)
  loggerInitialized = true
  startBridge()

  if (isDev) {
    const log = getLogTapeLogger(['app', 'logger'])
    if (verboseLoggingEnabled) {
      log.debug('Logger initialized (verbose mode, debug+ for all)')
    } else {
      log.debug('Logger initialized (dev mode, info+)')
      if (debugCategories.length > 0) {
        log.debug('Debug enabled for: {categories}', { categories: debugCategories.join(', ') })
      }
    }
  }
}

/**
 * Enable or disable verbose logging at runtime.
 * Called when the developer.verboseLogging setting changes.
 */
export async function setVerboseLogging(enabled: boolean): Promise<void> {
  if (enabled === verboseLoggingEnabled) {
    return
  }

  verboseLoggingEnabled = enabled
  await applyLoggerConfig(enabled, true)

  // Also update the Rust-side log level to match
  try {
    await commands.setLogLevel(enabled ? 'debug' : 'info')
  } catch {
    // Backend may not be ready during early startup; silently ignore
  }

  const log = getLogTapeLogger(['app', 'logger'])
  if (enabled) {
    log.info('Verbose logging enabled - debug level for all categories')
  } else {
    log.info('Verbose logging disabled - returning to normal log levels')
  }
}

/**
 * Get a logger for a specific feature.
 * Categories are hierarchical, for example ['app', 'fileExplorer', 'selection'].
 *
 * @example
 * const log = getAppLogger('fileExplorer')
 * log.debug('Selected {count} {itemsNoun}', { count, itemsNoun: pluralize(count, 'item') })
 */
export function getAppLogger(feature: string): Logger {
  return getLogTapeLogger(['app', feature])
}
