/**
 * Tier-3 tests for `AskCmdrSection.svelte`: the chat memory size row, and what the section
 * says when the store refuses a consent change or a memory wipe.
 *
 * Pins what the user can actually do and see: the presets are all there with Automatic
 * first, and a size larger than the window Cmdr believes the model has WARNS while keeping
 * the value. Cmdr never overrules the choice — what it knows about a model can be out of
 * date, and the user may be right about their own model.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, tick } from 'svelte'

const settings: Record<string, unknown> = {
  'ai.provider': 'cloud',
  'askCmdr.interactiveModel': '',
  'askCmdr.chatMemorySize': 'auto',
}

vi.mock('$lib/settings/settings-store', () => ({
  getSetting: vi.fn((key: string) => settings[key]),
  setSetting: vi.fn((key: string, value: unknown) => {
    settings[key] = value
    return Promise.resolve()
  }),
  resetSetting: vi.fn(),
  isModified: vi.fn(() => false),
  onSpecificSettingChange: vi.fn(() => () => {}),
  onSettingChange: vi.fn(() => () => {}),
}))

vi.mock('$lib/ask-cmdr/ask-cmdr-consent.svelte', () => ({
  consentState: { accepted: true, acceptedAt: 1_760_000_000 },
  refreshConsent: vi.fn(() => Promise.resolve()),
  acceptConsent: vi.fn(() => Promise.resolve('done')),
  revokeConsent: vi.fn(() => Promise.resolve('done')),
  declineConsent: vi.fn(() => Promise.resolve('done')),
}))

const { modelWindow } = vi.hoisted(() => ({
  modelWindow: { model: 'gpt-4o', knownWindowTokens: null as number | null },
}))
vi.mock('$lib/tauri-commands', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  askCmdrCostSummary: vi.fn(() => Promise.resolve({ days: [] })),
  askCmdrModelWindow: vi.fn(() => Promise.resolve(modelWindow)),
  askCmdrForgetMemory: vi.fn(() => Promise.resolve(3)),
  // The forget confirmation is a `ModalDialog`, which reports itself open and closed.
  notifyDialogOpened: vi.fn(() => Promise.resolve()),
  notifyDialogClosed: vi.fn(() => Promise.resolve()),
}))

import AskCmdrSection from './AskCmdrSection.svelte'
import { declineConsent, revokeConsent } from '$lib/ask-cmdr/ask-cmdr-consent.svelte'
import { askCmdrForgetMemory } from '$lib/tauri-commands'

/** Lets a click's awaited IPC settle and the section re-render. */
async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve()
  await tick()
}

async function mountSection(): Promise<HTMLElement> {
  const target = document.createElement('div')
  document.body.appendChild(target)
  mount(AskCmdrSection, { target, props: { searchQuery: '' } })
  // Two ticks: the model-window read is a promise the warning depends on.
  await tick()
  await Promise.resolve()
  await tick()
  return target
}

function warningText(target: HTMLElement): string | null {
  return target.querySelector('.memory-warning')?.textContent.trim() ?? null
}

describe('AskCmdrSection chat memory size', () => {
  beforeEach(() => {
    settings['askCmdr.chatMemorySize'] = 'auto'
    modelWindow.model = 'gpt-4o'
    modelWindow.knownWindowTokens = null
  })

  it('renders the row, showing the current choice by name', async () => {
    // The row exists at all (a registry entry alone renders nothing), and the closed picker
    // reads as the choice rather than as a raw stored value. The preset list itself is pinned
    // in `settings-registry.test.ts`: Ark UI renders its items only once opened.
    const target = await mountSection()
    const labelFors = Array.from(target.querySelectorAll('label.setting-label')).map((el) => el.getAttribute('for'))
    expect(labelFors).toContain('askCmdr.chatMemorySize')
    expect(target.textContent).toContain('Automatic (recommended)')
    target.remove()
  })

  it('shows a chosen preset as a grouped number, so 200000 never reads as 20,000', async () => {
    settings['askCmdr.chatMemorySize'] = '200000'
    const target = await mountSection()
    expect(target.textContent).toContain('200,000')
    target.remove()
  })

  it('says nothing while the size fits the window Cmdr knows about', async () => {
    settings['askCmdr.chatMemorySize'] = '60000'
    modelWindow.knownWindowTokens = 128_000
    const target = await mountSection()
    expect(warningText(target)).toBeNull()
    target.remove()
  })

  it('warns, without overruling, when the size is larger than that window', async () => {
    settings['askCmdr.chatMemorySize'] = '200000'
    modelWindow.knownWindowTokens = 128_000
    const target = await mountSection()
    expect(warningText(target)).toBe('Your model may refuse a message this long. Cmdr keeps the value you set.')
    target.remove()
  })

  it('stays quiet when nothing knows the window: an unknown model is not a warning', async () => {
    settings['askCmdr.chatMemorySize'] = '200000'
    modelWindow.model = 'some-future-model-9000'
    modelWindow.knownWindowTokens = null
    const target = await mountSection()
    expect(warningText(target)).toBeNull()
    target.remove()
  })

  it('never warns on Automatic: it follows the window by construction', async () => {
    settings['askCmdr.chatMemorySize'] = 'auto'
    modelWindow.knownWindowTokens = 16_384
    const target = await mountSection()
    expect(warningText(target)).toBeNull()
    target.remove()
  })
})

describe('AskCmdrSection when the store says no', () => {
  it('turns Ask Cmdr off the way onboarding does, so a refused "no" is retried and then held', async () => {
    // Pre-fix Turn off called `revokeConsent` once, and a refusal left consent recorded with
    // only a line under the row. `declineConsent` is the one "no" path: retry, then hold.
    const target = await mountSection()

    target.querySelector<HTMLButtonElement>('.enable-row button')?.click()
    await settle()

    expect(declineConsent).toHaveBeenCalledOnce()
    expect(revokeConsent).not.toHaveBeenCalled()
    target.remove()
  })

  it("says so under the row when turning Ask Cmdr off didn't take, even held, instead of re-enabling silently", async () => {
    vi.mocked(declineConsent).mockResolvedValueOnce('notSaved')
    const target = await mountSection()

    target.querySelector<HTMLButtonElement>('.enable-row button')?.click()
    await settle()

    expect(target.querySelector('.consent-not-saved')?.textContent.trim()).toBe(
      'Cmdr couldn’t save your choice. Try again?',
    )
    target.remove()
  })

  it('stays quiet when the change saved', async () => {
    const target = await mountSection()

    target.querySelector<HTMLButtonElement>('.enable-row button')?.click()
    await settle()

    expect(target.querySelector('.consent-not-saved')).toBeNull()
    target.remove()
  })

  it('points at the memory folder when forgetting stopped partway, rather than closing without a word', async () => {
    vi.mocked(askCmdrForgetMemory).mockRejectedValueOnce(new Error('unwritable'))
    const target = await mountSection()

    const forgetButton = target.querySelectorAll<HTMLButtonElement>('.memory-actions button')[1]
    forgetButton.click()
    await settle()
    const dialogButtons = document.querySelectorAll<HTMLButtonElement>('[data-dialog-id="forget-memory"] button')
    dialogButtons[dialogButtons.length - 1].click()
    await settle()

    expect(target.querySelector('.memory-not-forgotten')?.textContent.trim()).toBe(
      'Cmdr couldn’t delete every note. Open the memory folder to remove the rest.',
    )
    expect(target.querySelector('.memory-forgotten')).toBeNull()
    target.remove()
  })
})
