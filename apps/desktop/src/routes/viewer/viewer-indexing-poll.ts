import { viewerGetStatus, type TotalRows } from '$lib/tauri-commands'
import { getAppLogger } from '$lib/logging/logger'

const log = getAppLogger('viewer')
const INDEXING_POLL_INTERVAL = 500

interface IndexingPollDeps {
  getSessionId: () => string
  onStatus: (status: {
    backendType: 'fullLoad' | 'byteSeek' | 'lineIndex'
    isIndexing: boolean
    /** Rows, and whether they're counted or sampled. The scroll coordinate. */
    totalRows: TotalRows
    /** Physical lines, when the backend knows them. The status bar's count. */
    totalLines: number | null
  }) => void
}

/** Encapsulates the periodic `viewerGetStatus` poll the viewer runs while the backend
 *  is building a line index. Stops itself on completion or any IPC error. */
export function createIndexingPoll(deps: IndexingPollDeps) {
  let timer: ReturnType<typeof setInterval> | undefined

  async function pollOnce() {
    const sessionId = deps.getSessionId()
    if (!sessionId) return
    try {
      const status = await viewerGetStatus(sessionId)
      deps.onStatus({
        backendType: status.backendType,
        isIndexing: status.isIndexing,
        totalRows: status.totalRows,
        totalLines: status.totalLines,
      })
      if (!status.isIndexing) {
        log.info('Indexing finished, backendType={backendType}', { backendType: status.backendType })
        stop()
      }
    } catch {
      stop()
    }
  }

  function start() {
    stop()
    timer = setInterval(() => {
      void pollOnce()
    }, INDEXING_POLL_INTERVAL)
  }

  function stop() {
    if (timer) {
      clearInterval(timer)
      timer = undefined
    }
  }

  return { start, stop }
}
