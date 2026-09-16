/**
 * The badge's visibility rules. Each one exists because the wrong answer is either a scary
 * label on a machine that's fine, or silence on the machine that needs the hint.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { mount, tick, unmount } from 'svelte'

const { checkFullDiskAccessQuiet, isMacOS } = vi.hoisted(() => ({
  checkFullDiskAccessQuiet: vi.fn<() => Promise<boolean>>(() => Promise.resolve(false)),
  isMacOS: vi.fn<() => boolean>(() => true),
}))

vi.mock('$lib/tauri-commands', () => ({
  checkFullDiskAccessQuiet: () => checkFullDiskAccessQuiet(),
}))
vi.mock('$lib/shortcuts/key-capture', () => ({
  isMacOS: () => isMacOS(),
}))

import { refreshFdaStatus, _resetFdaStatusForTests } from './fda-status.svelte'
import FdaBadge from './FdaBadge.svelte'

let mounted: { target: HTMLElement; instance: Record<string, unknown> } | undefined

async function renderBadge(onboardingOnFdaStep = false): Promise<HTMLElement> {
  const target = document.createElement('div')
  document.body.appendChild(target)
  const instance = mount(FdaBadge, { target, props: { onboardingOnFdaStep, onOpenOnboarding: () => {} } })
  mounted = { target, instance }
  await tick()
  return target
}

describe('FdaBadge', () => {
  beforeEach(() => {
    _resetFdaStatusForTests()
    isMacOS.mockReturnValue(true)
    checkFullDiskAccessQuiet.mockResolvedValue(false)
  })

  afterEach(async () => {
    if (mounted) {
      await unmount(mounted.instance)
      mounted.target.remove()
      mounted = undefined
    }
  })

  it('stays hidden before the first probe answers', async () => {
    // `null` must not render as "no access", or every launch flashes the badge at people
    // who granted it long ago.
    const target = await renderBadge()
    expect(target.querySelector('.fda-badge')).toBeNull()
  })

  it('shows once the probe says the grant is missing', async () => {
    await refreshFdaStatus()
    const target = await renderBadge()
    expect(target.querySelector('.fda-badge')).not.toBeNull()
  })

  it('stays hidden when the grant is there', async () => {
    checkFullDiskAccessQuiet.mockResolvedValue(true)
    await refreshFdaStatus()
    const target = await renderBadge()
    expect(target.querySelector('.fda-badge')).toBeNull()
  })

  it('stays hidden off macOS, where the permission does not exist', async () => {
    isMacOS.mockReturnValue(false)
    await refreshFdaStatus()
    const target = await renderBadge()
    expect(target.querySelector('.fda-badge')).toBeNull()
  })

  it('stays hidden while the wizard is already on the FDA step', async () => {
    await refreshFdaStatus()
    const target = await renderBadge(true)
    expect(target.querySelector('.fda-badge')).toBeNull()
  })

  it('keeps the last answer when the probe throws, rather than inventing a missing grant', async () => {
    checkFullDiskAccessQuiet.mockResolvedValue(true)
    await refreshFdaStatus()
    checkFullDiskAccessQuiet.mockRejectedValue(new Error('IPC down'))
    await refreshFdaStatus()
    const target = await renderBadge()
    expect(target.querySelector('.fda-badge')).toBeNull()
  })
})
