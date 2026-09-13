/**
 * AI provider configuration plumbing shared by the settings UI, the onboarding wizard,
 * and the live-apply listener.
 *
 * **`pushConfigToBackend()`** — read-fresh push of the current AI provider config to
 * Rust. Re-reads `ai.provider` / `ai.cloudProvider` / `ai.cloudProviderConfigs` /
 * `ai.localContextSize` from `getSetting(...)` on every call and passes the cloud
 * provider ID to `configureAi(...)`, which reads that provider's key in the backend.
 * Surfaces the reported secret-store failure via a deduped persistent toast so a
 * silently-broken keyring isn't invisible.
 * Callers MUST NOT pass cached values: the helper has read-fresh semantics so that the
 * "user flips provider mid-flight" race resolves to whichever provider is current at
 * the actual IPC moment (see `settings-applier.ts` for the listener wiring).
 *
 * Lives in `lib/settings/` (not `lib/settings/sections/`) because the function isn't
 * UI-component-coupled — it's a service the wizard, the applier listener, and the
 * settings UI all reach for. `sections/` is reserved for UI subcomponents.
 */

import { getSetting, resolveCloudConfig, getCloudProvider } from '$lib/settings'
import { configureAi } from '$lib/tauri-commands'
import { getAppLogger } from '$lib/logging/logger'
import { addToast } from '$lib/ui/toast'
import { describeSecretError } from './sections/ai-secret-error'

const logger = getAppLogger('ai-settings')

/** Stable id so the same toast replaces in place across consecutive failed startup attempts. */
const secretErrorToastId = 'ai-secret-store-error'

/**
 * Push current AI config (provider, context size, cloud endpoint) to the Rust backend. Everything
 * here comes from `settings.json`; the API key does NOT travel with it. We send the cloud provider
 * ID and the backend reads that provider's key from the OS secret store itself, so a plaintext key
 * never sits in a webview. See `docs/security.md` § "AI API keys".
 *
 * The backend reports a secret-store read failure back in the outcome, which we surface as a
 * persistent toast (deduped) so a silently-broken keyring isn't invisible to the user.
 *
 * **Read-fresh contract (load-bearing).** Every relevant setting is re-read from `getSetting(...)`
 * at call time. Callers MUST NOT pass cached values. The applier listener may fire while the user
 * is still toggling things in the wizard; reading fresh means whichever provider is current at the
 * actual IPC moment wins, which matches user expectations.
 */
export async function pushConfigToBackend(): Promise<void> {
  try {
    const providerId = getSetting('ai.cloudProvider')
    const resolved = resolveCloudConfig(providerId, getSetting('ai.cloudProviderConfigs'))
    // Keyless endpoints (Ollama, LM Studio, a custom OpenAI-compatible endpoint) set this false, so
    // the backend doesn't treat their empty key as "not configured". See `resolve_backend` (Rust).
    const requiresApiKey = getCloudProvider(providerId)?.requiresApiKey ?? false

    const outcome = await configureAi(
      getSetting('ai.provider'),
      Number(getSetting('ai.localContextSize')),
      providerId,
      resolved.baseUrl,
      resolved.model,
      requiresApiKey,
    )

    if (outcome.secretStoreError != null) {
      logger.error("Couldn't read AI API key from secret store: {error}", { error: outcome.secretStoreError })
      const msg = describeSecretError(outcome.secretStoreError, 'read')
      const body = msg.body ? `\n${msg.body}` : ''
      addToast(`${msg.title}${body}`, {
        level: msg.level,
        dismissal: 'persistent',
        id: secretErrorToastId,
      })
    }
  } catch (e) {
    logger.error("Couldn't push AI config to backend: {error}", { error: e })
  }
}
