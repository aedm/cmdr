// S3 accounts: the account's secret and the unattended-reconnect query.
// Connecting, cancelling, disconnecting, pinning, and forgetting go through the
// protocol-agnostic `servers.ts`, which takes S3 places like any other.
//
// The model (an account is an endpoint plus an access key id; its places are
// buckets plus the account root) and every connect outcome:
// `crates/cmdr-s3/DETAILS.md`.

import { commands } from '$lib/ipc/bindings'
import type { S3ProviderChoice, S3UnattendedReconnect } from '$lib/ipc/bindings'
import { throwKeychainError } from '$lib/servers/keychain-failure'

export type { S3ProviderChoice, S3UnattendedReconnect }

/**
 * Saves the secret access key for one ACCOUNT (a provider's endpoint plus an
 * access key id), so every bucket under that key connects silently.
 *
 * This call is the "remember the secret" switch: `hasS3Credentials` reads it
 * back, `deleteS3Credentials` turns it off, and there's no second flag that could
 * disagree with the store.
 *
 * Throws a `KeychainFailure` if the store refused, or if the provider's fields
 * make no endpoint.
 */
export async function saveS3Credentials(
  provider: S3ProviderChoice,
  accessKeyId: string,
  secret: string,
): Promise<void> {
  const res = await commands.saveS3Credentials(provider, accessKeyId, secret)
  if (res.status === 'error') throwKeychainError(res.error)
}

/**
 * Whether a secret is stored for one account. There's deliberately no command
 * that returns the secret itself.
 */
export async function hasS3Credentials(provider: S3ProviderChoice, accessKeyId: string): Promise<boolean> {
  return await commands.hasS3Credentials(provider, accessKeyId)
}

/**
 * Forgets the stored secret for one account, and so for every place under it.
 * Throws a `KeychainFailure` if the store refused.
 */
export async function deleteS3Credentials(provider: S3ProviderChoice, accessKeyId: string): Promise<void> {
  const res = await commands.deleteS3Credentials(provider, accessKeyId)
  if (res.status === 'error') throwKeychainError(res.error)
}

/**
 * Whether a registered S3 place can come back on its own as it stands, the
 * backend's own answer. `no_stored_secret` is the one to warn about: the switch
 * is on and nothing is stored. `null` when nothing S3 is registered under that id.
 * Ask when a banner renders rather than polling; it can reach the Keychain.
 */
export async function getS3UnattendedReconnect(volumeId: string): Promise<S3UnattendedReconnect | null> {
  return await commands.getS3UnattendedReconnect(volumeId)
}
