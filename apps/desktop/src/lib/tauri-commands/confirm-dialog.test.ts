/** The native confirmation's IPC seam: a question nobody saw is never a yes. */
import { describe, it, expect, vi } from 'vitest'

vi.mock('$lib/ipc/bindings', () => ({ commands: { confirmWithCheckbox: vi.fn() } }))
vi.mock('$lib/logging/logger', () => ({
  getAppLogger: () => ({ warn: vi.fn(), info: vi.fn(), debug: vi.fn(), error: vi.fn() }),
}))

import { commands } from '$lib/ipc/bindings'
import { confirmWithCheckbox } from './confirm-dialog'

const request = {
  title: 'Forget server',
  message: 'Forget My NAS?',
  confirmLabel: 'Forget',
  cancelLabel: 'Cancel',
  checkboxLabel: 'Also forget the saved password',
  checked: true,
}

describe('confirmWithCheckbox', () => {
  it('passes the alert’s answer through', async () => {
    vi.mocked(commands.confirmWithCheckbox).mockResolvedValueOnce({ kind: 'confirmed', checked: false })
    expect(await confirmWithCheckbox(request)).toEqual({ kind: 'confirmed', checked: false })
  })

  it('answers cancelled when the bridge breaks down', async () => {
    vi.mocked(commands.confirmWithCheckbox).mockRejectedValueOnce(new Error('ipc closed'))
    expect(await confirmWithCheckbox(request)).toEqual({ kind: 'cancelled' })
  })
})
