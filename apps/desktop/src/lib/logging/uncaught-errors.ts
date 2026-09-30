/**
 * Forwards uncaught frontend errors into the app log, so a crash in the webview
 * leaves a trace instead of dying in a console nobody reads.
 *
 * Without this, an uncaught throw or an unhandled promise rejection is invisible
 * everywhere that matters: no line in the Rust log, nothing in an error-report
 * bundle, nothing in a CI E2E run's output. A two-hour CI wedge where the panes
 * stopped listing while the rest of the app stayed healthy produced zero frontend
 * evidence for exactly this reason.
 *
 * Registered once from `routes/+layout.ts` (a stable module, like
 * `hmr-recovery.ts`) so the listeners survive layout re-evaluation during HMR.
 * `log.error` is forwarded to Rust in production too — the prod sinks carry
 * `error+` — so these land in the log file and in error-report bundles.
 */

import { getAppLogger } from './logger'

const log = getAppLogger('uncaught')

/**
 * Splits a thrown value into its message (`Name: message`) and its frames.
 *
 * The message goes to `{detail}`, which the log bridge renders as a `detail="…"` field: an
 * error report redacts it (paths under any prefix included) and caps it, because an error
 * message can quote a path or a name. The frames are the app bundle's own URLs.
 *
 * ❗ **The message is always there, whatever the engine put in the stack.** WebKit's
 * `error.stack` is FRAMES ONLY, where V8's opens with `Name: message`. Cmdr ships on
 * WKWebView, so taking the stack verbatim logged every uncaught error as an anonymous pile of
 * minified offsets — which is exactly what a Svelte flush throw in the servers hub looked like,
 * and it cost a real diagnosis. (verified on macOS 26.6.2 / WKWebView and Linux WebKitGTK,
 * reading E2E logs, 2026-09-07)
 */
function describe(value: unknown): { detail: string; stack: string } {
  if (value instanceof Error) {
    const detail = `${value.name}: ${value.message}`
    const stack = value.stack ?? ''
    return { detail, stack: stack.startsWith(detail) ? stack.slice(detail.length).replace(/^\n/, '') : stack }
  }
  try {
    return { detail: typeof value === 'string' ? value : JSON.stringify(value), stack: '' }
  } catch {
    // A value that won't stringify (a cycle, a Proxy that throws) still deserves a line.
    return { detail: String(value), stack: '' }
  }
}

let registered = false

/**
 * Installs the `error` / `unhandledrejection` listeners. Idempotent, so an HMR
 * re-import can't stack duplicate handlers that log every failure twice.
 *
 * Neither listener calls `preventDefault`: the goal is to OBSERVE, never to
 * swallow. Swallowing would hide the failure from the browser's own reporting and
 * from `hmr-recovery`, which needs to see the rejection it recovers from.
 */
export function registerUncaughtErrorLogging(): void {
  if (registered || typeof window === 'undefined') return
  registered = true

  window.addEventListener('error', (event: ErrorEvent) => {
    // A failed resource load (`<img>`, `<script>`) also fires `error`, carrying
    // neither a thrown value nor a message. Those aren't crashes. The absent value
    // is `null` in some engines and `undefined` in others, so test for both.
    const thrown: unknown = event.error ?? null
    if (thrown === null && !event.message) return
    const { filename, lineno, colno } = event
    const source = filename ? `${filename}:${String(lineno)}:${String(colno)}` : 'unknown'
    const { detail, stack } = thrown !== null ? describe(thrown) : { detail: event.message, stack: '' }
    if (stack) log.error('Uncaught error at {source}: {detail}\n{stack}', { source, detail, stack })
    else log.error('Uncaught error at {source}: {detail}', { source, detail })
  })

  window.addEventListener('unhandledrejection', (event: PromiseRejectionEvent) => {
    const { detail, stack } = describe(event.reason)
    if (stack) log.error('Unhandled promise rejection: {detail}\n{stack}', { detail, stack })
    else log.error('Unhandled promise rejection: {detail}', { detail })
  })
}
