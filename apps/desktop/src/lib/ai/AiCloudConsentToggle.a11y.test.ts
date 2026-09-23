/**
 * Tier 3 a11y tests for `AiCloudConsentToggle.svelte`, the Allow cloud AI switch and its
 * "what Cmdr sends" disclosure, plus the promises that disclosure has to keep. It's what a
 * person reads before the one click that lets anything reach a cloud AI service, for every
 * feature, so each claim below is one the copy must make (or must never make again).
 */

import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, tick } from 'svelte'
import { expectNoA11yViolations } from '$lib/test-a11y'
import { getMessage } from '$lib/intl/messages.svelte'

const { consentState } = vi.hoisted(() => {
  // An annotation, not an `as`: the lint auto-fix strips an assertion it thinks is unnecessary.
  const consentState: { accepted: boolean | null; acceptedAt: number | null } = { accepted: false, acceptedAt: null }
  return { consentState }
})
vi.mock('./cloud-consent.svelte', () => ({
  cloudConsentState: consentState,
  refreshCloudConsent: vi.fn(() => Promise.resolve()),
  acceptCloudConsent: vi.fn(() => Promise.resolve('done')),
  declineCloudConsent: vi.fn(() => Promise.resolve('done')),
  CLOUD_CONSENT_ANCHOR: 'settings-ai-cloud-consent',
}))

import AiCloudConsentToggle from './AiCloudConsentToggle.svelte'

async function mountToggle(): Promise<HTMLElement> {
  const target = document.createElement('div')
  document.body.appendChild(target)
  mount(AiCloudConsentToggle, { target, props: { anchor: true } })
  await tick()
  return target
}

beforeEach(() => {
  consentState.accepted = false
  consentState.acceptedAt = null
})

describe('AiCloudConsentToggle a11y', () => {
  it('has no a11y violations while off, with the disclosure open', async () => {
    const target = await mountToggle()
    expect(target.querySelector('details')?.open).toBe(true)
    await expectNoA11yViolations(target)
    target.remove()
  })

  it('has no a11y violations once on, with the "on since" line', async () => {
    consentState.accepted = true
    consentState.acceptedAt = 1_760_000_000
    const target = await mountToggle()
    expect(target.querySelector('details')?.open).toBe(false)
    await expectNoA11yViolations(target)
    target.remove()
  })
})

describe('what the disclosure promises', () => {
  it('names what every AI feature sends, not only Ask Cmdr', async () => {
    const target = await mountToggle()
    for (const feature of ['New folder name suggestions', 'Search in plain words', 'Select by description']) {
      expect(target.textContent).toContain(feature)
    }
    target.remove()
  })

  it('discloses the memory the agent keeps and sends with every message', async () => {
    const target = await mountToggle()
    expect(target.textContent).toContain(getMessage('ai.cloudConsent.askCmdr.item.memory'))
    expect(target.textContent).toContain(getMessage('ai.cloudConsent.askCmdr.memory'))
    expect(target.textContent).toContain(getMessage('ai.cloudConsent.askCmdr.proactive'))
    target.remove()
  })

  /**
   * `inspect_file` reads parts of a file on request (text, PDF pages, archive entries, a
   * photo's EXIF with its location), so the list names it beside names and sizes, and the
   * reassurance paragraph must not promise "no file contents".
   */
  it('discloses that parts of files are read on request', async () => {
    const target = await mountToggle()
    expect(target.textContent).toContain(getMessage('ai.cloudConsent.askCmdr.item.contents'))
    expect(target.textContent).toContain(getMessage('ai.cloudConsent.askCmdr.contentsRule'))
    expect(target.textContent).not.toContain('no file contents')
    target.remove()
  })

  /** ❌ The old promise must not come back: the agent proposes changes and writes its notes. */
  it('never claims the agent changes nothing', async () => {
    const target = await mountToggle()
    expect(target.textContent).not.toContain('never changes anything')
    target.remove()
  })

  it('says where the data goes, including self-hosted and custom endpoints', async () => {
    const target = await mountToggle()
    expect(target.textContent).toContain(getMessage('ai.cloudConsent.whereItGoes'))
    target.remove()
  })
})
