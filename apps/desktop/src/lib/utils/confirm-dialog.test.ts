/**
 * A confirmation with a checkbox: the native alert where there is one, and the
 * plain question elsewhere, with the option left at its safe side.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'

const ask = vi.fn(() => Promise.resolve(true))
const confirmWithCheckboxCommand = vi.fn((_request: unknown): Promise<{ kind: string; checked?: boolean }> =>
  Promise.resolve({ kind: 'cancelled' }),
)

vi.mock('@tauri-apps/plugin-dialog', () => ({ ask: (...args: unknown[]) => ask(...(args as [])) }))
vi.mock('$lib/tauri-commands', () => ({
  confirmWithCheckbox: (request: unknown) => confirmWithCheckboxCommand(request),
}))

import { confirmWithCheckbox } from './confirm-dialog'

const question = {
  message: 'Forget My NAS?',
  title: 'Forget server',
  confirmLabel: 'Forget',
  checkboxLabel: 'Also forget the saved password',
  checked: true,
}

beforeEach(() => {
  vi.clearAllMocks()
})

describe('confirmWithCheckbox', () => {
  it('answers what the native alert answered, the checkbox included', async () => {
    confirmWithCheckboxCommand.mockResolvedValueOnce({ kind: 'confirmed', checked: false })
    expect(await confirmWithCheckbox(question)).toEqual({ confirmed: true, checked: false })
    expect(confirmWithCheckboxCommand).toHaveBeenCalledWith(
      expect.objectContaining({ checkboxLabel: 'Also forget the saved password', checked: true }),
    )
    expect(ask).not.toHaveBeenCalled()
  })

  it('answers a cancel as not confirmed', async () => {
    confirmWithCheckboxCommand.mockResolvedValueOnce({ kind: 'cancelled' })
    expect((await confirmWithCheckbox(question)).confirmed).toBe(false)
  })

  /** ❗ A choice nobody saw is never made for them: without the checkbox on screen, it reads unchecked. */
  it('asks the plain question where there is no native alert, and leaves the option off', async () => {
    confirmWithCheckboxCommand.mockResolvedValueOnce({ kind: 'unsupported' })
    expect(await confirmWithCheckbox(question)).toEqual({ confirmed: true, checked: false })
    expect(ask).toHaveBeenCalledOnce()
  })
})
