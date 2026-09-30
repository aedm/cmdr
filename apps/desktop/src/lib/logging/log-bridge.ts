/**
 * Batching log bridge that sends frontend logs to the Rust backend.
 *
 * Handles three concerns:
 * 1. Batching: collects entries for 100ms, sends in one IPC call
 * 2. Deduplication: collapses identical messages within a batch window
 * 3. Throttling: caps at 200 entries/second, warns on excess
 */

// eslint-disable-next-line cmdr/no-raw-bindings-import -- logging/store bootstrap infra: the tauri-commands barrel imports the logger (storage.ts), so wrapping here would create an import cycle
import { commands, type FrontendLogEntry } from '$lib/ipc/bindings'
import type { LogRecord, Sink } from '@logtape/logtape'

interface PendingEntry {
  level: FrontendLogEntry['level']
  category: string
  message: string
  count: number
}

const BATCH_INTERVAL_MS = 100
const MAX_ENTRIES_PER_SECOND = 200

let pendingEntries: PendingEntry[] = []
let batchTimer: ReturnType<typeof setTimeout> | null = null
let entriesThisSecond = 0
let throttleResetTimer: ReturnType<typeof setInterval> | null = null
let throttleWarningEmitted = false
let droppedCount = 0
const droppedCategoryToCountMap = new Map<string, number>()

/**
 * Placeholder names whose values name a path or an identity, mapped to the field key the
 * backend's report redactor recognizes (`src-tauri/src/redact/DETAILS.md` § "Keyed path fields"
 * and § "Producer-owned identity fields"). A relative path, a bare name, or a volume ID has no
 * shape the redactor can spot in prose, so the key is what gets it tokenized in a report. The
 * local log keeps the value whole.
 */
const REDACTOR_KEY_BY_PLACEHOLDER: Record<string, string> = {
  path: 'path',
  loadPath: 'path',
  landedPath: 'path',
  root: 'path',
  target: 'path',
  destinationPath: 'destination',
  dir: 'dir',
  folder: 'dir',
  folderName: 'dir',
  input: 'input',
  name: 'file',
  fileName: 'file',
  volumeId: 'volumeId',
  volumeName: 'volumeName',
  deviceId: 'deviceId',
  host: 'host',
  server: 'server',
  share: 'share',
  // External text (an error message, a server's answer): redacted and capped in a report.
  detail: 'detail',
}

/** Quote like Rust's `{:?}`, which is the escaping the redactor's quoted-value grammar reads. */
function debugQuote(value: string): string {
  let out = '"'
  for (const char of value) {
    const code = char.codePointAt(0) ?? 0
    if (char === '"' || char === '\\') out += `\\${char}`
    else if (char === '\n') out += '\\n'
    else if (char === '\r') out += '\\r'
    else if (char === '\t') out += '\\t'
    else if (code < 0x20 || code === 0x7f) out += `\\u{${code.toString(16)}}`
    else out += char
  }
  return `${out}"`
}

function formatMessage(record: LogRecord): string {
  // LogTape message is an array of interleaved template parts and values,
  // for example ["Loading ", 42, " items"]. A tagged template carries no names, so it joins as is.
  const { message, rawMessage } = record
  const names = typeof rawMessage === 'string' ? [...rawMessage.matchAll(/\{([^{}\s]+)\}/g)].map((m) => m[1]) : []
  if (names.length * 2 + 1 !== message.length) return message.map(String).join('')

  let out = ''
  message.forEach((part, index) => {
    const name = index % 2 === 1 ? names[(index - 1) / 2] : undefined
    const key = name === undefined ? undefined : REDACTOR_KEY_BY_PLACEHOLDER[name]
    if (key === undefined || name === undefined) {
      out += String(part)
      return
    }
    // A template that already reads `volumeId={volumeId}` gets the quotes, not a second key.
    const keyed = out.endsWith(`${key}=`) || out.endsWith(`${name}=`)
    out += keyed ? debugQuote(String(part)) : `${key}=${debugQuote(String(part))}`
  })
  return out
}

function getCategory(record: LogRecord): string {
  // LogTape categories are arrays like ['app', 'fileExplorer']
  // Skip the 'app' root prefix, join the rest
  const parts = record.category.length > 1 ? record.category.slice(1) : record.category
  return parts.join('.')
}

function mapLevel(level: string): FrontendLogEntry['level'] {
  // LogTape uses "warning", Rust log uses "warn"
  if (level === 'warning') return 'warn'
  return level as FrontendLogEntry['level']
}

function addEntry(level: FrontendLogEntry['level'], category: string, message: string): void {
  // Check throttle
  if (entriesThisSecond >= MAX_ENTRIES_PER_SECOND) {
    droppedCount++
    droppedCategoryToCountMap.set(category, (droppedCategoryToCountMap.get(category) ?? 0) + 1)
    if (!throttleWarningEmitted) {
      throttleWarningEmitted = true
      // Schedule the warning to be sent at the next flush
      pendingEntries.push({
        level: 'warn',
        category: 'log-bridge',
        message: `Excessive frontend logging detected: entries are being dropped (>${String(MAX_ENTRIES_PER_SECOND)}/s). This may indicate a bug (infinite loop, runaway effect).`,
        count: 1,
      })
    }
    return
  }

  entriesThisSecond++

  // Deduplication: check if last entry in pending batch is identical
  const last = pendingEntries.at(-1)
  if (last && last.level === level && last.category === category && last.message === message) {
    last.count++
    return
  }

  pendingEntries.push({ level, category, message, count: 1 })
  scheduleBatch()
}

function scheduleBatch(): void {
  if (batchTimer !== null) return
  batchTimer = setTimeout(() => {
    void flush()
  }, BATCH_INTERVAL_MS)
}

async function flush(): Promise<void> {
  batchTimer = null
  if (pendingEntries.length === 0) return

  const entries = pendingEntries
  pendingEntries = []

  // Update the throttle warning with the actual dropped count and the categories that
  // dominated, so the offending feature is identifiable without reproducing the burst.
  if (droppedCount > 0) {
    const warningIdx = entries.findIndex((e) => e.category === 'log-bridge' && e.level === 'warn')
    if (warningIdx >= 0) {
      const topCategories = [...droppedCategoryToCountMap.entries()]
        .sort((a, b) => b[1] - a[1])
        .slice(0, 3)
        .map(([category, count]) => `${category} ×${String(count)}`)
        .join(', ')
      entries[warningIdx].message =
        `Excessive frontend logging detected: ${String(droppedCount)} entries dropped in the last second (top: ${topCategories}). This may indicate a bug (infinite loop, runaway effect).`
    }
    droppedCount = 0
    droppedCategoryToCountMap.clear()
    throttleWarningEmitted = false
  }

  // Format entries for IPC
  const ipcEntries: FrontendLogEntry[] = entries.map((e) => ({
    level: e.level,
    category: e.category,
    message: e.count > 1 ? `${e.message} (×${String(e.count)}, deduplicated)` : e.message,
  }))

  try {
    await commands.batchFeLogs(ipcEntries)
  } catch {
    // Backend not available (app shutting down, or early startup). Silently drop.
  }
}

/** LogTape sink that batches and sends logs to the Rust backend. */
export function getTauriBridgeSink(): Sink {
  return (record: LogRecord): void => {
    const level = mapLevel(record.level)
    const category = getCategory(record)
    const message = formatMessage(record)
    addEntry(level, category, message)
  }
}

/** Start the per-second throttle reset timer. Call once at init. */
export function startBridge(): void {
  if (throttleResetTimer !== null) return
  throttleResetTimer = setInterval(() => {
    entriesThisSecond = 0
  }, 1000)

  // Flush remaining logs when the page unloads
  window.addEventListener('beforeunload', () => {
    void flush()
  })
}

/** Stop the bridge (for cleanup in tests). */
export function stopBridge(): void {
  if (throttleResetTimer !== null) {
    clearInterval(throttleResetTimer)
    throttleResetTimer = null
  }
  if (batchTimer !== null) {
    clearTimeout(batchTimer)
    batchTimer = null
  }
  pendingEntries = []
  entriesThisSecond = 0
  droppedCount = 0
  droppedCategoryToCountMap.clear()
  throttleWarningEmitted = false
}
