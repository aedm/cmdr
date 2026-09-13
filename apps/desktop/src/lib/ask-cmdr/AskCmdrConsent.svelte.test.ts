/**
 * The rail's consent screen when the store refuses the opt-in: it says so beside the buttons
 * and keeps the gate shut, instead of re-enabling Turn on as if nothing happened.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, tick, unmount } from 'svelte'
import type { ConsentOutcome } from './ask-cmdr-consent.svelte'

const { acceptConsent, openRail } = vi.hoisted(() => ({
  acceptConsent: vi.fn<() => Promise<ConsentOutcome>>(),
  openRail: vi.fn<() => Promise<void>>(() => Promise.resolve()),
}))

vi.mock('./ask-cmdr-consent.svelte', () => ({
  consentState: { accepted: false, acceptedAt: null, needsReconsent: false },
  acceptConsent: () => acceptConsent(),
}))
vi.mock('./ask-cmdr-trigger.svelte', () => ({
  closeRail: vi.fn(),
  openRail: () => openRail(),
}))

import AskCmdrConsent from './AskCmdrConsent.svelte'

async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve()
  await tick()
}

function mountConsent(): { target: HTMLElement; destroy: () => Promise<void> } {
  const target = document.createElement('div')
  document.body.appendChild(target)
  const instance = mount(AskCmdrConsent, { target })
  return {
    target,
    destroy: async () => {
      await unmount(instance)
      target.remove()
    },
  }
}

beforeEach(() => {
  acceptConsent.mockReset()
  openRail.mockClear()
})

describe('AskCmdrConsent', () => {
  it('says the opt-in didn\'t save, and doesn\'t open the chat', async () => {
    acceptConsent.mockResolvedValue('notSaved')
    const { target, destroy } = mountConsent()

    target.querySelector<HTMLButtonElement>('.consent-accept')?.click()
    await settle()

    expect(target.querySelector('.consent-not-saved')?.textContent.trim()).toBe(
      'Cmdr couldn’t save your choice. Try again?',
    )
    expect(openRail).not.toHaveBeenCalled()
    await destroy()
  })

  it('opens the chat when the opt-in saved, with nothing to say', async () => {
    acceptConsent.mockResolvedValue('done')
    const { target, destroy } = mountConsent()

    target.querySelector<HTMLButtonElement>('.consent-accept')?.click()
    await settle()

    expect(openRail).toHaveBeenCalledOnce()
    expect(target.querySelector('.consent-not-saved')).toBeNull()
    await destroy()
  })
})
