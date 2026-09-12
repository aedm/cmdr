/**
 * Where an MCP `nav_to_path` actually left the pane.
 *
 * `navigate()`'s two arms settle at different moments (`pane/navigate.ts` § "`settled`
 * resolve point, PER INTENT ARM"). The in-place arm's `settled` IS the listing promise,
 * so awaiting it is enough. The SWITCH arm commits the destination optimistically and
 * resolves `settled` immediately, before the new volume has listed anything: the pane
 * reports the target while the listing is still to come, and if that listing dies, an
 * edge-flow fallback (MTP-fatal, retry, open-home) moves the pane somewhere else
 * entirely. Replying `ok: true` on that resolve is what let a cross-volume navigation
 * ack `OK: Navigated left pane to …` for a pane that never went there.
 *
 * So after a switch the adapter waits for the pane to go QUIET — a listing that both
 * started and came to rest — and then reports the location the pane actually holds. The
 * timing rules live here as pure functions over injected probes, so they're unit-testable
 * without a real pane or a real clock.
 *
 * ❌ Don't replace the quiet wait with a plain path comparison. Both arms commit
 * optimistically (P4), so the pane reports the target from the moment `navigate()`
 * returns; only a settled listing tells success from a navigation still in flight.
 */

/** The three questions the quiet wait asks a pane, plus the clock it runs on. */
export interface PaneQuietProbe {
  /** The pane's live listing handle. A new listing means a new id. */
  getListingId: () => string | null
  /** Whether the pane's listing is mid-load. */
  isLoading: () => boolean
  /** Milliseconds since an arbitrary epoch. Injected so tests drive their own clock. */
  now: () => number
  /** Resolves after `ms`. Injected for the same reason. */
  sleep: (ms: number) => Promise<void>
}

export interface QuietWaitOptions {
  /** The pane's listing id before the navigation started. */
  listingIdBefore: string | null
  /**
   * Whether the navigation must show a listing other than `listingIdBefore` before the
   * pane counts as at rest. Off only when nothing new will list (`expectsNewListing`):
   * then an idle pane on the listing it already had is where the navigation ended.
   */
  requireNewListing: boolean
  /** How long to wait for the pane to come to rest before giving up. */
  budgetMs: number
  /** How often to look. */
  pollMs: number
  /** How long the pane must stay idle on its new listing before it counts as at rest. */
  quietMs: number
}

/**
 * How long the adapter waits for a switched pane to come to rest, and how closely it
 * watches. The budget sits under the backend's 30 s `nav_to_path` round-trip so a pane
 * that never settles is reported as such by the FE, with the location it actually holds,
 * rather than as a bare backend timeout. `quietMs` is the confirmation window: a listing
 * that settles and is immediately replaced (a failing listing, then the edge-flow
 * fallback's) must not be read as an arrival.
 */
export const NAV_QUIET_WAIT = { budgetMs: 20_000, pollMs: 100, quietMs: 250 } as const

/** A pane location, narrowed to what deciding the outcome needs. */
export interface PanePlace {
  volumeId: string
  path: string
}

/**
 * What the pane did with the navigation, as the `mcp-response` reports it. The Rust
 * `nav_to_path` handler turns each into the tool's result — mirrored by `NavAck` in
 * `apps/desktop/src-tauri/src/mcp/executor/mod.rs`, and matched on the discriminant
 * rather than on any message text.
 */
export type NavLandingOutcome = { outcome: 'navigated' | 'fell-back' | 'did-not-settle' } & PanePlace

/**
 * What `mcp-nav-to-path` and `mcp-volume-select` put on the wire. The plain `{ ok, error }`
 * shape covers the declines that happen before the pane moves (no explorer, an
 * unresolvable path or unknown volume, a synchronous refusal); the landing shapes carry
 * a typed `outcome` plus the location the pane came to rest on, which the Rust handler
 * turns into the tool result. ❌ The backend branches on `outcome`, never on the message.
 */
export type NavReplyBody = { ok: false; error: string } | ({ ok: boolean } & NavLandingOutcome)

/**
 * Wait for the listing a volume switch kicks off to start AND come to rest.
 *
 * Returns `true` once the pane has held a listing other than `listingIdBefore` with no
 * load in flight for `quietMs`, `false` when the budget runs out first. The quiet window
 * re-arms whenever a load starts again, so a failing listing followed by a fallback's
 * listing resolves against the FALLBACK's resting place, not the doomed one's.
 */
export async function waitForPaneToGoQuiet(probe: PaneQuietProbe, options: QuietWaitOptions): Promise<boolean> {
  const deadline = probe.now() + options.budgetMs
  let quietSince: number | null = null

  for (;;) {
    const hasTheListingItNeeds = !options.requireNewListing || probe.getListingId() !== options.listingIdBefore
    if (hasTheListingItNeeds && !probe.isLoading()) {
      if (quietSince === null) quietSince = probe.now()
      else if (probe.now() - quietSince >= options.quietMs) return true
    } else {
      quietSince = null
    }

    if (probe.now() >= deadline) return false
    await probe.sleep(options.pollMs)
  }
}

/** Same place? Volume ids must match exactly; a trailing slash on either path doesn't count. */
function isSamePlace(a: PanePlace, b: PanePlace): boolean {
  return a.volumeId === b.volumeId && withoutTrailingSlash(a.path) === withoutTrailingSlash(b.path)
}

function withoutTrailingSlash(path: string): string {
  return path.length > 1 ? path.replace(/\/+$/, '') : path
}

/**
 * Decide what to tell the agent: the pane reached the target, came to rest somewhere
 * else, or never came to rest at all. `quiet: false` outranks a matching location
 * because an unsettled pane reports its destination optimistically.
 */
export function classifyLanding(args: { target: PanePlace; landed: PanePlace; quiet: boolean }): NavLandingOutcome {
  const { target, landed, quiet } = args
  if (!quiet) return { outcome: 'did-not-settle', ...landed }
  return { outcome: isSamePlace(landed, target) ? 'navigated' : 'fell-back', ...landed }
}

/**
 * Whether a volume switch has to show a new listing before the pane counts as at rest.
 *
 * Not when its destination, once the background correction has decided it, is where the
 * pane already was: the pane's props don't change, so nothing re-lists (re-selecting
 * the volume a pane shows). Not on a volume with no backend listing either (the servers
 * hub). Everywhere else a new listing is the only evidence of arrival, and waiting for it
 * is what spans an undialed phone's connect.
 */
export function expectsNewListing(args: { before: PanePlace; landed: PanePlace; hasBackendListing: boolean }): boolean {
  return args.hasBackendListing && !isSamePlace(args.before, args.landed)
}

/**
 * Decide what to tell an agent that selected a volume. Only the volume is compared: the
 * select doesn't pick the folder (the switch reopens the one last used there), so any
 * path on the selected volume is an arrival.
 */
export function classifyVolumeLanding(args: {
  targetVolumeId: string
  landed: PanePlace
  quiet: boolean
}): NavLandingOutcome {
  const { targetVolumeId, landed, quiet } = args
  if (!quiet) return { outcome: 'did-not-settle', ...landed }
  return { outcome: landed.volumeId === targetVolumeId ? 'navigated' : 'fell-back', ...landed }
}
