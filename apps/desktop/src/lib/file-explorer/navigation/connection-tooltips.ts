import { tString } from '$lib/intl/messages.svelte'
import type { MessageKey } from '$lib/intl/keys.gen'
import type { ConnectionState } from '../types'

/**
 * What the row's connection dot says on hover, one sentence per state.
 *
 * ❗ Exhaustive over `ConnectionState` by a `Record`, so a seventh state can't
 * compile until someone writes its words. The dot is the only thing on a server
 * row that says how live it is, and a state that quietly borrowed another's
 * sentence is how "signed out" came to read as "using system connection".
 */
const CONNECTION_TOOLTIP_KEYS: Record<ConnectionState, MessageKey> = {
  direct: 'fileExplorer.navigation.connectionTooltipDirect',
  os_mount: 'fileExplorer.navigation.connectionTooltipSystem',
  disconnected: 'fileExplorer.navigation.connectionTooltipDisconnected',
  needs_sign_in: 'fileExplorer.navigation.connectionTooltipNeedsSignIn',
  needs_host_key_approval: 'fileExplorer.navigation.connectionTooltipNeedsHostKey',
  saved: 'fileExplorer.navigation.connectionTooltipSaved',
}

/** The dot's tooltip for one connection state. */
export function getConnectionTooltip(state: ConnectionState): string {
  return tString(CONNECTION_TOOLTIP_KEYS[state])
}
