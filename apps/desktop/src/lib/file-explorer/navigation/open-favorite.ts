/**
 * The ONE way a favorite opens.
 *
 * A favorite is a virtual row (`id: 'fav-<uuid>'`, `category: 'favorite'`)
 * pointing at a path on a real volume, so opening one is two steps: ask Rust
 * which volume contains the path, then switch the pane onto THAT volume with the
 * favorite's path as the destination. Every surface needs both steps and the
 * analytics emit that goes with them, which is why this is a module rather than
 * a branch each surface keeps its own copy of.
 *
 * The switch itself stays the caller's: the favorites menu hands its host a
 * `VolumeChangePayload`, while the palette / MCP route calls `navigate()`
 * directly and wants its `NavigateResult` back. `go` is that last mile, and its
 * return value passes straight through.
 *
 * **Decision: an unresolvable favorite navigates NOWHERE.** A favorite whose
 * containing volume can't be resolved is one of two things: a path in a
 * namespace that was never favoritable (a dead `search-results://` snapshot id,
 * a phone that's unplugged), or a mount that just went away. Sending the pane to
 * volume `root` at `/` carrying the raw path — what the switcher's branch did —
 * evicts the user from where they were standing and then shows them a listing
 * error about a path they never typed. Leaving the pane put and logging it is
 * the honest answer, and it's rare in practice: `volumes/mod.rs` already filters
 * a favorite whose path doesn't exist out of the list, so a row that's visible
 * and unresolvable is a transient. `kindCanBeFavorited` (`pane/volume-capabilities.ts`)
 * plus Rust's `add_favorite` keep new ones from being stored at all.
 */

import { resolvePathVolume } from '$lib/tauri-commands'
import { getAppLogger } from '$lib/logging/logger'
import { reportFavoriteOpened, type FavoriteOpenedEvent } from './favorites-analytics'
import type { VolumeChangePayload } from '../pane/types'

const log = getAppLogger('fileExplorer')

/**
 * Opened, carrying whatever `go` returned, or refused because the favorite's
 * path resolves to no volume. A caller with an outcome type of its own maps
 * `unresolved` onto its own "nothing happened" arm.
 */
export type OpenFavoriteResult<T> = { kind: 'opened'; opened: T } | { kind: 'unresolved' }

export interface OpenFavoriteArgs<T> {
  /** The favorite's target path, straight off its `VolumeInfo.path`. */
  favoritePath: string
  /** Which surface made the pick, and how. The one emit of `favorite_opened` lives here. */
  picked: FavoriteOpenedEvent
  /** The last mile: put the pane on the containing volume at the favorite's path. */
  go: (target: VolumeChangePayload) => T
}

export async function openFavorite<T>({
  favoritePath,
  picked,
  go,
}: OpenFavoriteArgs<T>): Promise<OpenFavoriteResult<T>> {
  const { volume: containingVolume, timedOut } = await resolvePathVolume(favoritePath)
  if (!containingVolume) {
    log.warn('Favorite points at a path no volume claims, so the pane stays put: {path} (timedOut: {timedOut})', {
      path: favoritePath,
      timedOut,
    })
    return { kind: 'unresolved' }
  }

  reportFavoriteOpened(picked)
  return {
    kind: 'opened',
    opened: go({
      volumeId: containingVolume.id,
      volumePath: containingVolume.path,
      targetPath: favoritePath,
    }),
  }
}
