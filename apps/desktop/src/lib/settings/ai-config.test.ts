/**
 * Unit tests for `ai-config.ts`: AI configuration plumbing shared by Settings, the onboarding
 * wizard, and the settings-applier listener.
 *
 * `pushConfigToBackend()` is a read-fresh push of the current AI config to Rust. It surfaces
 * secret store failures as a deduped persistent toast and keeps pushing the rest of the config
 * so the user sees something rather than a silent backend.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest'

interface KeyStatus {
  isSet: boolean
  fingerprint: string
}
interface ConfigureOutcome {
  secretStoreError: unknown
}

const getAiApiKeyStatus = vi.fn<(id: string) => Promise<KeyStatus>>(() =>
  Promise.resolve({ isSet: false, fingerprint: '' }),
)
const configureAi = vi.fn<
  (payload: {
    provider: string
    contextSize: number
    cloudProviderId: string
    cloudBaseUrl: string
    cloudModel: string
    cloudRequiresApiKey: boolean
  }) => Promise<ConfigureOutcome>
>(() => Promise.resolve({ secretStoreError: null }))

vi.mock('$lib/tauri-commands', () => ({
  getAiApiKeyStatus: (id: string) => getAiApiKeyStatus(id),
  configureAi: (
    provider: string,
    contextSize: number,
    cloudProviderId: string,
    cloudBaseUrl: string,
    cloudModel: string,
    cloudRequiresApiKey: boolean,
  ) => configureAi({ provider, contextSize, cloudProviderId, cloudBaseUrl, cloudModel, cloudRequiresApiKey }),
}))

const settingsMap: Record<string, string> = {}
vi.mock('$lib/settings', async (importOriginal) => {
  const actual = await importOriginal<Record<string, unknown>>()
  return {
    ...actual,
    getSetting: (id: string) => settingsMap[id] ?? '',
  }
})

const addToast = vi.fn<(...args: unknown[]) => void>()
vi.mock('$lib/ui/toast', () => ({
  addToast: (...args: unknown[]) => {
    addToast(...args)
  },
}))

const loggerWarn = vi.fn<(...args: unknown[]) => void>()
const loggerInfo = vi.fn<(...args: unknown[]) => void>()
const loggerError = vi.fn<(...args: unknown[]) => void>()
vi.mock('$lib/logging/logger', () => ({
  getAppLogger: () => ({
    warn: (...args: unknown[]) => {
      loggerWarn(...args)
    },
    info: (...args: unknown[]) => {
      loggerInfo(...args)
    },
    error: (...args: unknown[]) => {
      loggerError(...args)
    },
    debug: () => {},
  }),
}))

// Import AFTER mocks are wired so the module captures the mocked references.
import { pushConfigToBackend } from './ai-config'

function resetState(): void {
  for (const k of Object.keys(settingsMap)) {
    delete settingsMap[k]
  }
  getAiApiKeyStatus.mockReset()
  getAiApiKeyStatus.mockResolvedValue({ isSet: false, fingerprint: '' })
  configureAi.mockReset()
  configureAi.mockResolvedValue({ secretStoreError: null })
  addToast.mockReset()
  loggerWarn.mockReset()
  loggerInfo.mockReset()
  loggerError.mockReset()
}

describe('pushConfigToBackend', () => {
  beforeEach(resetState)

  it('reads provider + base URL fresh and pushes the provider ID, never a key, to configureAi', async () => {
    settingsMap['ai.provider'] = 'cloud'
    settingsMap['ai.cloudProvider'] = 'openai'
    settingsMap['ai.cloudProviderConfigs'] = JSON.stringify({ openai: { model: 'gpt-4o' } })
    settingsMap['ai.localContextSize'] = '32768'

    await pushConfigToBackend()

    // The backend reads the key from the OS secret store itself; this window never sees it.
    // OpenAI requires a key, so requiresApiKey is true.
    expect(configureAi).toHaveBeenCalledWith({
      provider: 'cloud',
      contextSize: 32768,
      cloudProviderId: 'openai',
      cloudBaseUrl: expect.stringContaining('openai.com') as string,
      cloudModel: 'gpt-4o',
      cloudRequiresApiKey: true,
    })
  })

  it('passes requiresApiKey=false for a keyless local endpoint (Ollama)', async () => {
    settingsMap['ai.provider'] = 'cloud'
    settingsMap['ai.cloudProvider'] = 'ollama'
    settingsMap['ai.cloudProviderConfigs'] = JSON.stringify({ ollama: { model: 'llama3.2' } })
    settingsMap['ai.localContextSize'] = '32768'

    await pushConfigToBackend()

    expect(configureAi).toHaveBeenCalledWith({
      provider: 'cloud',
      contextSize: 32768,
      cloudProviderId: 'ollama',
      cloudBaseUrl: expect.stringContaining('localhost') as string,
      cloudModel: 'llama3.2',
      cloudRequiresApiKey: false,
    })
  })

  it('surfaces a persistent toast when the backend reports a secret-store read failure', async () => {
    settingsMap['ai.provider'] = 'cloud'
    settingsMap['ai.cloudProvider'] = 'openai'
    settingsMap['ai.cloudProviderConfigs'] = JSON.stringify({ openai: { model: 'gpt-4o' } })
    settingsMap['ai.localContextSize'] = '32768'
    configureAi.mockResolvedValue({ secretStoreError: { type: 'access_denied', message: 'keyring locked' } })

    await pushConfigToBackend()

    expect(addToast).toHaveBeenCalledTimes(1)
    const [body, opts] = addToast.mock.calls[0]
    expect(typeof body).toBe('string')
    expect(opts).toMatchObject({ dismissal: 'persistent' })
    expect(loggerError).toHaveBeenCalled()
  })

  it('stays quiet when the backend read the key fine', async () => {
    settingsMap['ai.provider'] = 'cloud'
    settingsMap['ai.cloudProvider'] = 'openai'
    settingsMap['ai.cloudProviderConfigs'] = JSON.stringify({ openai: { model: 'gpt-4o' } })
    settingsMap['ai.localContextSize'] = '32768'

    await pushConfigToBackend()

    expect(addToast).not.toHaveBeenCalled()
  })

  it('logs and swallows configureAi failures', async () => {
    settingsMap['ai.provider'] = 'cloud'
    settingsMap['ai.cloudProvider'] = 'openai'
    settingsMap['ai.cloudProviderConfigs'] = '{}'
    settingsMap['ai.localContextSize'] = '65536'
    configureAi.mockRejectedValueOnce(new Error('IPC down'))

    await expect(pushConfigToBackend()).resolves.toBeUndefined()
    expect(loggerError).toHaveBeenCalled()
  })

  it('coerces ai.localContextSize via Number()', async () => {
    settingsMap['ai.provider'] = 'local'
    settingsMap['ai.cloudProvider'] = ''
    settingsMap['ai.cloudProviderConfigs'] = '{}'
    settingsMap['ai.localContextSize'] = '32768'

    await pushConfigToBackend()

    expect(configureAi).toHaveBeenCalledWith({
      provider: 'local',
      contextSize: 32768,
      cloudProviderId: '',
      cloudBaseUrl: expect.any(String) as string,
      cloudModel: expect.any(String) as string,
      cloudRequiresApiKey: false,
    })
  })

  it('never reaches for a command that reads the key back', async () => {
    settingsMap['ai.provider'] = 'cloud'
    settingsMap['ai.cloudProvider'] = 'openai'
    settingsMap['ai.cloudProviderConfigs'] = JSON.stringify({ openai: { model: 'gpt-4o' } })
    settingsMap['ai.localContextSize'] = '32768'

    await pushConfigToBackend()

    expect(getAiApiKeyStatus).not.toHaveBeenCalled()
  })
})
