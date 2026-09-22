/**
 * What each server row action does: Disconnect, the pin pair, the two Forgets,
 * Edit, and Open.
 *
 * Lives beside the switcher rather than inside `VolumeBreadcrumb.svelte` because
 * three surfaces run the same actions (the switcher row's submenu, the hub row's
 * menu, and the palette's `servers.*` commands). WHICH actions a row offers is
 * `row-menu.ts`'s answer; this module is what running one costs.
 *
 * ❗ **A server says Disconnect, never Eject** (`DETAILS.md` § "Eject button +
 * row context menu"): "Eject" promises safe-to-unplug and a server has nothing
 * to unplug.
 * Disconnecting keeps the row: a pinned place comes back `saved`.
 */

import {
  disconnectPlace,
  forgetServer,
  forgetServerSecret,
  listSavedServers,
  setPlacePinned,
} from '$lib/tauri-commands'
import { addToast } from '$lib/ui/toast'
import { confirmDialog } from '$lib/utils/confirm-dialog'
import { tString } from '$lib/intl/messages.svelte'
import { getAppLogger } from '$lib/logging/logger'
import { isServerVolumeId } from '$lib/servers/server-path-utils'
import { openEditServerSheet } from '$lib/servers/open-sign-in'
import type { VolumeContextActionKind } from '$lib/ipc/bindings'
import type { VolumeInfo } from '../types'

const log = getAppLogger('fileExplorer')

/**
 * Whether the servers command family owns this row.
 *
 * ❗ Off the VOLUME ID, which the two id minters spell (`sftp-…`, `webdav-…`), ❌
 * never off `category === 'network'`: a mounted SMB share is one of those, and
 * its session is an OS mount that `disconnectPlace` doesn't speak. SMB shares
 * reach the switcher as ordinary mounted volumes and leave through Eject; they
 * join this family when their unmount does.
 */
export function isServerPlaceRow(volume: VolumeInfo): boolean {
  return volume.category === 'network' && isServerVolumeId(volume.id)
}

/**
 * The volume IDs a saved server entry backs, which is what decides whether a server
 * row offers Edit… and Forget server (`row-menu.ts`).
 *
 * ❗ A store that doesn't answer costs those rows two items, never the menu: it
 * answers an empty set. ❌ No Keychain read on this path, and none is needed:
 * "Forget saved password" is always offered, and `forgetServerSecret` says
 * whether an entry was there ([`forgetSavedSecret`] words a `false`). Every read
 * of a Keychain entry can cost a system prompt, which a menu opening must not
 * spend (the same rule `../network/CLAUDE.md` states for SMB).
 */
export async function listSavedPlaceIds(): Promise<Set<string>> {
  try {
    const servers = await listSavedServers()
    return new Set(servers.flatMap((server) => server.places.map((place) => place.volumeId)))
  } catch (e) {
    log.warn('Reading the saved servers for the row menus broke down: {error}', { error: String(e) })
    return new Set()
  }
}

/**
 * Drops a place's session, from the row's Disconnect control or its menu item.
 *
 * The row SURVIVES: a pinned place comes back as a `saved` row, and a pane
 * standing on it goes home through the `VolumeUnmounted` broadcast the command
 * emits. A `false` answer means there was no session left, which a click racing
 * a dropped connection legitimately is.
 */
export async function disconnectServerPlace(volumeId: string, volumeName: string): Promise<void> {
  try {
    await disconnectPlace(volumeId)
  } catch (e) {
    refused('Disconnecting', volumeId, e, 'fileExplorer.navigation.disconnectRefusedToast', volumeName)
  }
}

/**
 * Asks first, then drops the server, its places, and their pins.
 *
 * ❗ Confirmed because it is not undoable from the UI: the entry, the pins, and
 * the tab that stood on it all go. The stored password does NOT: that is
 * [`forgetSavedSecret`], a separate request the menu offers separately.
 */
export async function forgetSavedServer(volumeId: string, volumeName: string): Promise<void> {
  const confirmed = await confirmDialog(
    tString('fileExplorer.navigation.forgetServerConfirm', { name: volumeName }),
    tString('fileExplorer.navigation.forgetServerConfirmTitle'),
  )
  if (!confirmed) return
  try {
    await forgetServer(volumeId)
  } catch (e) {
    refused('Forgetting', volumeId, e, 'fileExplorer.navigation.forgetServerRefusedToast', volumeName)
  }
}

/**
 * Moves a place's pin, from the "Pin / unpin server" command.
 *
 * ❗ Not confirmed, unlike the two forgets: unpinning loses nothing (the server
 * stays saved and stays in the hub), and the same command puts it back.
 */
export async function setServerPinned(volumeId: string, volumeName: string, pinned: boolean): Promise<void> {
  try {
    await setPlacePinned(volumeId, pinned)
    addToast(
      tString(pinned ? 'fileExplorer.navigation.serverPinnedToast' : 'fileExplorer.navigation.serverUnpinnedToast', {
        name: volumeName,
      }),
      { level: 'success' },
    )
  } catch (e) {
    refused('Pinning', volumeId, e, 'fileExplorer.navigation.pinRefusedToast', volumeName)
  }
}

/**
 * Asks first, then forgets the place's remembered secret, keeping the server.
 *
 * ❗ **This is where "was there one?" gets answered**, because the menu offers
 * the item unconditionally rather than paying a Keychain read to decide
 * (`row-menu.ts`). `forget_server_secret` answers `false` when the store
 * held nothing, and a person who just confirmed a Forget deserves a sentence
 * rather than silence. A `true` says nothing: the entry is gone, which is what
 * they asked for, and a toast confirming their own action is noise.
 */
export async function forgetSavedSecret(volumeId: string, volumeName: string): Promise<void> {
  const confirmed = await confirmDialog(
    tString('fileExplorer.navigation.forgetSecretConfirm', { name: volumeName }),
    tString('fileExplorer.navigation.forgetSecretConfirmTitle'),
  )
  if (!confirmed) return
  try {
    const forgotten = await forgetServerSecret(volumeId)
    if (!forgotten) {
      addToast(tString('fileExplorer.navigation.forgetSecretNoneToast', { name: volumeName }), { level: 'info' })
    }
  } catch (e) {
    refused('Forgetting the secret for', volumeId, e, 'fileExplorer.navigation.forgetSecretRefusedToast', volumeName)
  }
}

/**
 * Runs a server row action picked from a row's menu (`row-menu.ts`'s
 * `runVolumeRowAction`), or the palette command that mirrors it
 * (`command-handlers/servers-handlers.ts`).
 *
 * `eject` and the two favorite actions are NOT here — their owners are
 * `runDetach` and the favorites menu respectively.
 *
 * ❗ Open is the one that needs something from its caller: the `navigate()`
 * transaction lives in the pane, so a surface with no pane to move logs rather
 * than pretending it did something.
 */
export async function runServerRowAction(payload: {
  action: VolumeContextActionKind
  volumeId: string
  volumeName: string
  /**
   * Navigates the focused pane onto the place. Supplied by the surface that owns
   * a `navigate()` transaction; without one, Open logs rather than pretending.
   */
  onOpen?: (volumeId: string) => void
}): Promise<void> {
  const { action, volumeId, volumeName } = payload
  switch (action) {
    case 'disconnect':
      await disconnectServerPlace(volumeId, volumeName)
      return
    case 'forget-secret':
      await forgetSavedSecret(volumeId, volumeName)
      return
    case 'forget-server':
      await forgetSavedServer(volumeId, volumeName)
      return
    case 'pin':
    case 'unpin':
      await setServerPinned(volumeId, volumeName, action === 'pin')
      return
    case 'open':
      // The `navigate()` transaction lives in the pane, so the caller supplies
      // it. A menu raised somewhere with no pane to move logs rather than
      // pretending it did something.
      if (payload.onOpen) payload.onOpen(volumeId)
      else log.info('Open on {volumeId} had no pane to navigate', { volumeId })
      return
    case 'edit':
      await editServer(volumeId, volumeName)
      return
    case 'eject':
    case 'rename-favorite':
    case 'remove-favorite':
      // Owned elsewhere: `runDetach`, and the favorites menu.
      return
  }
}

/**
 * One sentence for the user, the raw value for the log.
 *
 * ❌ Never `String(e)` in the toast: these three commands answer a bool, so
 * anything thrown is the IPC transport itself — untranslated diagnostic text,
 * which is exactly what a person must not be handed.
 */
function refused(
  what: string,
  volumeId: string,
  error: unknown,
  key:
    | 'fileExplorer.navigation.disconnectRefusedToast'
    | 'fileExplorer.navigation.forgetServerRefusedToast'
    | 'fileExplorer.navigation.forgetSecretRefusedToast'
    | 'fileExplorer.navigation.pinRefusedToast',
  volumeName: string,
): void {
  log.warn('{what} {volumeId} broke down: {error}', { what, volumeId, error: String(error) })
  addToast(tString(key, { name: volumeName }), { level: 'error' })
}

/**
 * Opens the sign-in sheet on this server, prefilled.
 *
 * ❗ Looked up in the saved list rather than reconstructed from the row: a
 * `VolumeInfo` carries no key file, no remote folder, and no auto-reconnect
 * switch, and an edit form seeded from half a server would save the other half
 * away.
 */
async function editServer(volumeId: string, volumeName: string): Promise<void> {
  const saved = await listSavedServers().catch((e: unknown) => {
    log.warn('Reading the saved servers to edit {volumeId} broke down: {error}', { volumeId, error: String(e) })
    return []
  })
  const server = saved.find((entry) => entry.places.some((place) => place.volumeId === volumeId))
  if (!server) {
    // A forget that raced the menu. Nothing to edit and nothing worth saying:
    // the row is already gone from the switcher.
    log.info('Editing {volumeName} found no saved server behind it', { volumeName })
    return
  }
  await openEditServerSheet(server)
}
