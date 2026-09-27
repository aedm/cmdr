/**
 * `holdDiscoveryForServersView`: the backend browses mDNS only while a Servers view
 * is on screen, so the store tells it when the FIRST view appears and when the LAST
 * one leaves. Two panes can show the view at once.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'

const { ipc } = vi.hoisted(() => ({
  ipc: {
    setServersViewShown: vi.fn(() => Promise.resolve()),
  },
}))
vi.mock('$lib/tauri-commands', () => ipc)

import { holdDiscoveryForServersView } from './network-store.svelte'

describe('holdDiscoveryForServersView', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('asks for the browse when a view appears and lets it go when it leaves', () => {
    const release = holdDiscoveryForServersView()
    expect(ipc.setServersViewShown).toHaveBeenCalledWith(true)

    release()
    expect(ipc.setServersViewShown).toHaveBeenLastCalledWith(false)
  })

  it('keeps the browse while either of two panes still shows the view', () => {
    const releaseLeft = holdDiscoveryForServersView()
    const releaseRight = holdDiscoveryForServersView()
    releaseLeft()
    expect(ipc.setServersViewShown).toHaveBeenCalledTimes(1)
    expect(ipc.setServersViewShown).not.toHaveBeenCalledWith(false)

    releaseRight()
    expect(ipc.setServersViewShown).toHaveBeenLastCalledWith(false)
  })

  it('counts a double release once', () => {
    const releaseLeft = holdDiscoveryForServersView()
    const releaseRight = holdDiscoveryForServersView()
    releaseLeft()
    releaseLeft()
    expect(ipc.setServersViewShown).not.toHaveBeenCalledWith(false)
    releaseRight()
  })
})
