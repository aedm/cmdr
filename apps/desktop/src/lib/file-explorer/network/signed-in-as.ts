/**
 * Which account an SMB server or share is signed in as, and how the Servers list
 * and a share list say it ("as testuser", "as guest").
 *
 * ❗ Only from what's already known: a live mount's account (the mount table's
 * user), a share listing's own outcome, and ❌ never a Keychain read, since each
 * one can raise a system prompt (`CLAUDE.md`).
 */

export type SignedInAs = { kind: 'user'; username: string } | { kind: 'guest' }

/** The account a mount table's user names (`GUEST` for a guest mount), or `null` when it names none. */
export function signedInAsOfMount(mountAccount: string | null | undefined): SignedInAs | null {
  if (!mountAccount) return null
  return mountAccount.toLowerCase() === 'guest' ? { kind: 'guest' } : { kind: 'user', username: mountAccount }
}

/** An account a person typed or saved (`null` is none), as a user. */
export function signedInAsUser(username: string | null | undefined): SignedInAs | null {
  return username ? { kind: 'user', username } : null
}

/** Whether two answers name the same account. */
export function sameAccount(a: SignedInAs, b: SignedInAs): boolean {
  return a.kind === b.kind && (a.kind === 'guest' || (b.kind === 'user' && a.username === b.username))
}
