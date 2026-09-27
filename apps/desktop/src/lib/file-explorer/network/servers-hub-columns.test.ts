/**
 * Which columns the Servers list shows at a width: a narrow pane drops Address, then
 * Status, deliberately, ❌ never squeezing them to zero width (final QA: at 350 px both
 * collapsed to nothing and vanished without a trace).
 */
import { describe, it, expect } from 'vitest'
import { hubColumnsAt } from './servers-hub-columns'

describe('hubColumnsAt', () => {
  it('shows every column in a roomy pane, and before the pane is measured', () => {
    expect(hubColumnsAt(800)).toEqual({ address: true, status: true })
    expect(hubColumnsAt(0), 'unmeasured is not narrow').toEqual({ address: true, status: true })
  })

  it('drops Address first', () => {
    expect(hubColumnsAt(450)).toEqual({ address: false, status: true })
  })

  it('then Status', () => {
    expect(hubColumnsAt(350)).toEqual({ address: false, status: false })
  })
})
