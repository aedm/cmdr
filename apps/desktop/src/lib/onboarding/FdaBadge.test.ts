/**
 * The badge's visibility rules. Each one exists because the wrong answer is either a scary
 * label on a machine that's fine, or silence on the machine that needs the hint.
 */
import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest'
import { render, cleanup } from '@testing-library/svelte'

const checkFullDiskAccessQuiet = vi.fn<() => Promise<boolean>>(() => Promise.resolve(false))
const isMacOS = vi.fn<() => boolean>(() => true)

vi.mock('$lib/tauri-commands', () => ({
  checkFullDiskAccessQuiet: () => checkFullDiskAccessQuiet(),
}))
vi.mock('$lib/shortcuts/key-capture', () => ({
  isMacOS: () => isMacOS(),
}))

import { refreshFdaStatus, _resetFdaStatusForTests } from './fda-status.svelte'
import FdaBadge from './FdaBadge.svelte'

function renderBadge(onboardingOnFdaStep = false) {
  return render(FdaBadge, { props: { onboardingOnFdaStep, onOpenOnboarding: () => {} } })
}

describe('FdaBadge', () => {
  beforeEach(() => {
    _resetFdaStatusForTests()
    isMacOS.mockReturnValue(true)
    checkFullDiskAccessQuiet.mockResolvedValue(false)
  })

  afterEach(() => {
    cleanup()
  })

  it('stays hidden before the first probe answers', () => {
    // `null` must not render as "no access", or every launch flashes the badge at people
    // who granted it long ago.
    const { container } = renderBadge()
    expect(container.querySelector('.fda-badge')).toBeNull()
  })

  it('shows once the probe says the grant is missing', async () => {
    await refreshFdaStatus()
    const { container } = renderBadge()
    expect(container.querySelector('.fda-badge')).not.toBeNull()
  })

  it('stays hidden when the grant is there', async () => {
    checkFullDiskAccessQuiet.mockResolvedValue(true)
    await refreshFdaStatus()
    const { container } = renderBadge()
    expect(container.querySelector('.fda-badge')).toBeNull()
  })

  it('stays hidden off macOS, where the permission does not exist', async () => {
    isMacOS.mockReturnValue(false)
    await refreshFdaStatus()
    const { container } = renderBadge()
    expect(container.querySelector('.fda-badge')).toBeNull()
  })

  it('stays hidden while the wizard is already on the FDA step', async () => {
    await refreshFdaStatus()
    const { container } = renderBadge(true)
    expect(container.querySelector('.fda-badge')).toBeNull()
  })

  it('keeps the last answer when the probe throws, rather than inventing a missing grant', async () => {
    checkFullDiskAccessQuiet.mockResolvedValue(true)
    await refreshFdaStatus()
    checkFullDiskAccessQuiet.mockRejectedValue(new Error('IPC down'))
    await refreshFdaStatus()
    const { container } = renderBadge()
    expect(container.querySelector('.fda-badge')).toBeNull()
  })
})
