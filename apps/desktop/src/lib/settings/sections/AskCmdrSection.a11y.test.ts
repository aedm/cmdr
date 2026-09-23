/**
 * Tier 3 a11y tests for `AskCmdrSection.svelte`.
 *
 * The section: the on/off switch and its "cloud AI is off" hint, the provider hint, the
 * interactive-model and chat-memory-size rows (including the over-window warning), and the
 * spend rollup. The settings store, the cloud consent state, and the cost commands are mocked
 * so it mounts without a backend; consent is driven directly to cover the hint and its absence.
 */

import { describe, it, vi } from 'vitest'
import { mount, tick } from 'svelte'
import { expectNoA11yViolations } from '$lib/test-a11y'

vi.mock('$lib/settings/settings-store', () => ({
  getSetting: vi.fn((key: string) => {
    if (key === 'ai.provider') return 'cloud'
    if (key === 'askCmdr.enabled') return true
    if (key === 'askCmdr.interactiveModel') return ''
    if (key === 'askCmdr.chatMemorySize') return '200000'
    return undefined
  }),
  setSetting: vi.fn(() => Promise.resolve()),
  resetSetting: vi.fn(),
  isModified: vi.fn(() => false),
  onSpecificSettingChange: vi.fn(() => () => {}),
  onSettingChange: vi.fn(() => () => {}),
}))

const { consentState } = vi.hoisted(() => ({ consentState: { accepted: false } }))
vi.mock('$lib/ai/cloud-consent.svelte', () => ({
  cloudConsentState: consentState,
  refreshCloudConsent: vi.fn(() => Promise.resolve()),
  cloudAiBlocked: (provider: string) => provider === 'cloud' && !consentState.accepted,
  openCloudConsentSettings: vi.fn(),
}))
vi.mock('$lib/tauri-commands', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  askCmdrCostSummary: vi.fn(() => Promise.resolve({ days: [] })),
  // A 200,000-token pick against a 128,000-token window, so the over-window warning is part
  // of what axe sees.
  askCmdrModelWindow: vi.fn(() => Promise.resolve({ model: 'gpt-4o', knownWindowTokens: 128_000 })),
}))

import AskCmdrSection from './AskCmdrSection.svelte'

function mountSection(): HTMLElement {
  const target = document.createElement('div')
  document.body.appendChild(target)
  mount(AskCmdrSection, { target, props: { searchQuery: '' } })
  return target
}

describe('AskCmdrSection a11y', () => {
  it('has no a11y violations while cloud AI is off (the hint and its button showing)', async () => {
    consentState.accepted = false
    const target = mountSection()
    await tick()
    await expectNoA11yViolations(target)
    target.remove()
  })

  it('has no a11y violations once cloud AI is allowed', async () => {
    consentState.accepted = true
    const target = mountSection()
    await tick()
    await expectNoA11yViolations(target)
    target.remove()
  })
})
