import { afterEach, describe, expect, it, vi } from 'vitest'

const { setMainWindowVisible } = vi.hoisted(() => ({ setMainWindowVisible: vi.fn(() => Promise.resolve()) }))
vi.mock('$lib/tauri-commands', () => ({ setMainWindowVisible }))

import { startMainWindowVisibilityReport } from './main-window-visibility'

function setVisibility(state: DocumentVisibilityState): void {
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => state })
  document.dispatchEvent(new Event('visibilitychange'))
}

describe('startMainWindowVisibilityReport', () => {
  afterEach(() => {
    setMainWindowVisible.mockClear()
  })

  it('reports the current state at once, then every change', () => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' })
    const stop = startMainWindowVisibilityReport()
    expect(setMainWindowVisible).toHaveBeenLastCalledWith(true)

    setVisibility('hidden')
    expect(setMainWindowVisible).toHaveBeenLastCalledWith(false)

    setVisibility('visible')
    expect(setMainWindowVisible).toHaveBeenLastCalledWith(true)
    stop()
  })

  it('stops reporting after teardown', () => {
    const stop = startMainWindowVisibilityReport()
    stop()
    setMainWindowVisible.mockClear()

    setVisibility('hidden')

    expect(setMainWindowVisible).not.toHaveBeenCalled()
  })
})
