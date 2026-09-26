/** "as testuser" / "as guest": how a row or a share list's header names the account it's signed in as. */
import { tString } from '$lib/intl/messages.svelte'
import type { SignedInAs } from './signed-in-as'

export function signedInAsLabel(account: SignedInAs): string {
  return account.kind === 'guest'
    ? tString('servers.hub.guestAccount')
    : tString('servers.hub.shareAccount', { username: account.username })
}
