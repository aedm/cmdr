/**
 * What a hub row's F8, right-click, and row-menu picks do, and what the SMB host menu answers.
 *
 * A factory rather than lines in `ServersHub.svelte`, so the component stays the
 * table, the cursor, and the keys. These are the parts that ask a question,
 * write to a store, and toast — the parts worth reading on their own.
 *
 * ❗ **A one-place row and an SMB host take different paths at every branch**, and
 * that is the whole reason this module exists as a unit. A one-place row is a
 * PLACE the servers family speaks for (`forgetSavedServer`, `row-menu.ts`);
 * an SMB host is a manual-server entry whose "disconnect" unmounts shares rather
 * than dropping a session. Mixing the two is how a Forget removes the wrong
 * thing.
 */

import { disconnectNetworkHost, removeManualServer, showNetworkHostContextMenu } from '$lib/tauri-commands'
import { checkCredentialsForHost, forgetCredentials, getCredentialStatus } from './network-store.svelte'
import { forgetSavedServer, setServerAutoReconnect } from '../navigation/server-row-actions'
import {
  runRowFix,
  runVolumeRowAction,
  volumeRowMenu,
  type RowMenu,
  type RowMenuEntry,
  type RowToggleKind,
} from '../navigation/row-menu'
import { isVolumeBusy } from '$lib/stores/volume-busy-store.svelte'
import { confirmDialog } from '$lib/utils/confirm-dialog'
import { addToast } from '$lib/ui/toast'
import { tString } from '$lib/intl/messages.svelte'
import type { HubRow } from './servers-hub-rows'
import type { NetworkHost, VolumeInfo } from '../types'
import type { NetworkHostContextActionKind } from '$lib/ipc/bindings'

/** What the actions read from the component, live. */
export interface HubActionDeps {
  /** The rows on screen, for resolving a menu answer back to one. */
  getRows: () => HubRow[]
  /** The discovery store's hosts, for the SMB host menu's Disconnect. */
  getHosts: () => NetworkHost[]
  /** The volume list, which is where a live place's `VolumeInfo` lives. */
  getVolumes: () => VolumeInfo[]
  /** Re-read the saved list after a write the volume list won't announce. */
  refreshSaved: () => Promise<void>
  /** Take the pane onto a one-place server, the way Enter on its row does. */
  openServer: (row: HubRow) => void
}

/** The payload the native SMB-host menu answers with. */
/**
 * What the native SMB-host menu answered.
 *
 * ❗ `action` is the typed wire enum, ❌ never a free string: `forget-secret` is
 * spelled the same here as on a server ROW (`VolumeContextActionKind`), so one
 * act has one name on both sides of the app.
 */
export interface HostContextActionPayload {
  action: NetworkHostContextActionKind
  hostId: string
  hostName: string
}

export interface HubActions {
  /** F8, and the host menu's "Forget server". */
  forget: (row: HubRow) => Promise<void>
  /** A one-place row's in-app menu, or `null` for an SMB host (which keeps its native one). */
  rowMenu: (row: HubRow) => RowMenu | null
  /** A pick from a one-place row's menu. */
  runRowEntry: (row: HubRow, entry: RowMenuEntry) => Promise<void>
  /** Right-click on an SMB host row: its native host menu. */
  openHostMenu: (row: HubRow) => Promise<void>
  /** What the native SMB-host menu answered. */
  runHostAction: (payload: HostContextActionPayload) => Promise<void>
}

/**
 * What `ServersHub.svelte` calls on its `ServersHubRowMenu`. In a `.ts` because a type read
 * off a `.svelte` instance resolves to `any` under the plain-TypeScript lint service.
 */
export interface HubRowMenuAPI {
  /** Opens `row`'s menu at the pointer; false for a row with no in-app menu (an SMB host). */
  openAt: (row: HubRow, event: MouseEvent) => boolean
}

export function createHubActions(deps: HubActionDeps): HubActions {
  /**
   * F8. A saved server goes (after a confirmation); a host only mDNS knows about
   * has nothing to forget, and says so.
   */
  async function forget(row: HubRow): Promise<void> {
    if (!row.saved) {
      addToast(tString('fileExplorer.network.browser.cannotRemoveDiscovered'), { level: 'warn' })
      return
    }
    if (row.volumeId) {
      // A one-place server: the servers family owns the confirmation and the
      // toast, so the hub and the switcher's menu ask the same question.
      await forgetSavedServer(row.volumeId, row.name)
      return
    }
    await removeSavedSmbHost(row)
  }

  /** Forgets a saved SMB host, which is a manual-server entry rather than a place. */
  async function removeSavedSmbHost(row: HubRow): Promise<void> {
    const confirmed = await confirmDialog(
      tString('fileExplorer.network.browser.removeHostConfirm', { hostName: row.name }),
      tString('fileExplorer.network.browser.removeHostConfirmButton'),
    )
    if (!confirmed) return
    try {
      await removeManualServer(row.id)
      addToast(tString('fileExplorer.network.browser.hostRemoved', { hostName: row.name }), { level: 'success' })
      // The manual store is not the volume list, so nothing broadcasts this.
      await deps.refreshSaved()
    } catch {
      addToast(tString('fileExplorer.network.browser.hostRemoveFailed', { hostName: row.name }), { level: 'error' })
    }
  }

  /**
   * A one-place row's right-click menu: the servers list from `row-menu.ts`, the
   * same one the switcher row's submenu shows, so the two surfaces can't drift.
   * Read live, so a transfer starting under the open menu greys its Disconnect.
   * `null` for an SMB host, which keeps its own native host menu
   * ([`openHostMenu`]).
   */
  function rowMenu(row: HubRow): RowMenu | null {
    if (!row.volumeId) return null
    const volume = volumeForRow(row)
    return volumeRowMenu(volume, {
      busy: isVolumeBusy(volume.id),
      ejecting: false,
      isSaved: row.saved !== null,
      directConnection: undefined,
      autoReconnect: row.saved?.autoReconnect ?? undefined,
    })
  }

  /**
   * A pick from a one-place row's menu. Open takes the hub's own Enter path, so
   * it moves THIS pane; everything else runs where the switcher's runs.
   */
  async function runRowEntry(row: HubRow, entry: RowMenuEntry): Promise<void> {
    if (entry.type === 'toggle') {
      await flipToggle[entry.toggle](row)
      return
    }
    if (entry.type === 'fix') {
      await runRowFix({ volume: volumeForRow(row), fix: entry.fix })
      return
    }
    if (entry.action === 'open') {
      deps.openServer(row)
      return
    }
    await runVolumeRowAction({ volume: volumeForRow(row), action: entry.action })
  }

  /** What flipping each row switch does. A `Record`, so a new `RowToggleKind` won't compile until it's handled. */
  const flipToggle: Record<RowToggleKind, (row: HubRow) => Promise<void>> = {
    // An SMB share's switch: never on a one-place (SFTP or WebDAV) row.
    'direct-connection': () => Promise.resolve(),
    // The row shows the saved entry's switch; the `volumes-changed` the command emits is what
    // re-reads the saved list, so the next open shows the new state.
    'auto-reconnect': async (row) => {
      if (row.volumeId && row.saved) await setServerAutoReconnect(row.volumeId, !row.saved.autoReconnect)
    },
  }

  /** An SMB host's right-click: its native host menu. */
  async function openHostMenu(row: HubRow): Promise<void> {
    const host = row.host
    if (!host) return
    // ❗ Asked here rather than on Rust's popup path, where "is a secret stored?"
    // would put a Keychain read in front of the menu appearing.
    if (getCredentialStatus(host.name) === 'unknown') {
      await checkCredentialsForHost(host.name)
    }
    await showNetworkHostContextMenu(
      host.id,
      host.name,
      host.source === 'manual',
      getCredentialStatus(host.name) === 'has_creds',
    )
  }

  /**
   * The row's `VolumeInfo`, for the menu builder.
   *
   * The volume list is the source when it has the row; a saved server that is
   * neither pinned nor connected has no row there, and the stand-in carries the
   * fields the menu actually reads.
   */
  function volumeForRow(row: HubRow): VolumeInfo {
    const known = deps.getVolumes().find((volume) => volume.id === row.volumeId)
    if (known) return known
    return {
      id: row.volumeId ?? row.id,
      name: row.name,
      path: row.saved?.places[0]?.appRoot ?? '',
      category: 'network',
      isEjectable: false,
      fsType: row.protocol,
      connectionState: null,
      pinned: row.pinned,
    }
  }

  /** Actions dispatched from the native SMB-host context menu. */
  async function runHostAction(payload: HostContextActionPayload): Promise<void> {
    switch (payload.action) {
      case 'forget-server': {
        const row = deps.getRows().find((r) => r.host?.id === payload.hostId || r.id === payload.hostId)
        if (row) await forget(row)
        return
      }
      case 'forget-secret':
        await forgetHostSecret(payload.hostName)
        return
      case 'disconnect':
        await disconnectHost(payload)
        return
    }
  }

  async function forgetHostSecret(hostName: string): Promise<void> {
    try {
      await forgetCredentials(hostName)
      addToast(tString('fileExplorer.network.forgotPassword', { hostName }), { level: 'success' })
    } catch {
      addToast(tString('fileExplorer.network.deletePasswordFailed'), { level: 'error' })
    }
  }

  /**
   * An SMB host's Disconnect UNMOUNTS its shares; it does not drop a session the
   * way a place's does. Zero unmounted is a normal answer, not a fault.
   */
  async function disconnectHost(payload: HostContextActionPayload): Promise<void> {
    const host = deps.getHosts().find((h) => h.id === payload.hostId)
    if (!host) return
    try {
      const unmounted = await disconnectNetworkHost(host.id, host.name, host.ipAddress)
      if (unmounted.length > 0) {
        addToast(tString('fileExplorer.network.browser.disconnected', { hostName: payload.hostName }), {
          level: 'success',
        })
      } else {
        addToast(tString('fileExplorer.network.browser.noMountedShares', { hostName: payload.hostName }))
      }
    } catch (e) {
      addToast(tString('fileExplorer.network.browser.disconnectFailed', { message: String(e) }), { level: 'error' })
    }
  }

  return { forget, rowMenu, runRowEntry, openHostMenu, runHostAction }
}
