/**
 * MCP `select_volume`: switch a pane to a volume by name, then tell the agent where the
 * pane came to rest.
 *
 * The switch commits the volume's root optimistically, and its background correction then
 * reopens the folder last used there (`pane/navigate.ts` § "`settled` resolve point").
 * Acking before that correction landed let it arrive AFTER the reply and supersede the
 * agent's next `nav_to_path` ("Superseded by new navigation"). So the reply waits for the
 * switch's `corrected`, then for the pane to go quiet on whatever it decided
 * (`mcp-nav-landing.ts`), and names that place.
 *
 * ❌ Don't wait on the pane's path instead: it reads as the root the moment the switch
 * commits, before the correction moves the pane on, so a follow-up navigation still races
 * the correction.
 *
 * It rides the command bus (`volume.selectByName`), with the request id in the command args
 * the way the auto-confirmed file ops carry theirs. The bus doesn't hold MCP back behind an
 * open dialog (`DETAILS.md` § The dialog gate); what it buys is the typed command id and the
 * log line and breadcrumb every command gets.
 */

import { capabilitiesFor } from '$lib/file-explorer/pane/volume-capabilities'
import { getAppLogger } from '$lib/logging/logger'
import type { ExplorerAPI } from './explorer-api'
import {
  classifyVolumeLanding,
  expectsNewListing,
  NAV_QUIET_WAIT,
  waitForPaneToGoQuiet,
  type NavReplyBody,
} from './mcp-nav-landing'

const log = getAppLogger('mcpListeners')

export async function selectVolumeForMcp(args: {
  explorer: ExplorerAPI | undefined
  pane: 'left' | 'right'
  name: string
  /** The MCP round-trip id. Absent for a fire-and-forget caller (the E2E harness's resets). */
  requestId: string | undefined
}): Promise<void> {
  const { explorer, pane, name, requestId } = args
  const reply = async (body: NavReplyBody): Promise<void> => {
    if (requestId === undefined) return
    const { emit } = await import('@tauri-apps/api/event')
    await emit('mcp-response', { requestId, ...body })
  }

  if (!explorer) {
    log.warn('mcp-volume-select dropped: no explorer is mounted ({pane} pane, {name})', { pane, name })
    await reply({ ok: false, error: 'Explorer is not ready' })
    return
  }

  // Nobody to tell, so nothing to wait for. `selectVolumeByName` logs a name it can't find.
  if (requestId === undefined) {
    await explorer.selectVolumeByName(pane, name)
    return
  }

  // Read before the switch commits: it's what tells the switch's listing from the one before.
  const before = explorer.getPaneLocation(pane)
  const listingIdBefore = explorer.getPaneListingId(pane)

  const selection = await explorer.selectVolumeByName(pane, name)
  if (selection.kind === 'not-found') {
    await reply({ ok: false, error: `Volume '${name}' not found` })
    return
  }
  const { navigation } = selection
  if (navigation.status === 'refused') {
    log.warn('mcp-volume-select refused {name} ({pane} pane): {reason}', {
      name,
      pane,
      reason: navigation.reason.message,
    })
    await reply({ ok: false, error: navigation.reason.message })
    return
  }
  try {
    await navigation.corrected
  } catch (e) {
    await reply({ ok: false, error: e instanceof Error ? e.message : String(e) })
    return
  }

  const decided = explorer.getPaneLocation(pane)
  const requireNewListing = expectsNewListing({
    before,
    landed: decided,
    hasBackendListing: capabilitiesFor(decided.volumeId).hasBackendListing,
  })
  const quiet = await waitForPaneToGoQuiet(
    {
      getListingId: () => explorer.getPaneListingId(pane),
      isLoading: () => explorer.isPaneLoading(pane),
      now: () => Date.now(),
      sleep: (ms) => new Promise<void>((resolve) => setTimeout(resolve, ms)),
    },
    { listingIdBefore, requireNewListing, ...NAV_QUIET_WAIT },
  )

  const landed = explorer.getPaneLocation(pane)
  const landing = classifyVolumeLanding({
    targetVolumeId: selection.volumeId,
    landed: { volumeId: landed.volumeId, path: landed.path },
    quiet,
  })
  // The pane's own push can trail its listing, so flush it: a `cmdr://state` read right
  // after the reply then shows the landing.
  await explorer.syncPaneStateToMcp(pane)

  if (landing.outcome === 'navigated') {
    await reply({ ok: true, ...landing })
    return
  }
  log.warn('mcp-volume-select did not land {name} on the {pane} pane: {outcome} at {landedPath}', {
    name,
    pane,
    outcome: landing.outcome,
    landedPath: landing.path,
  })
  await reply({ ok: false, ...landing })
}
