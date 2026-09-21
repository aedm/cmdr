import { afterEach, describe, expect, it, vi } from 'vitest'

/**
 * The scale is read once at module load, so each case re-imports the module with its own
 * `CMDR_E2E_WAIT_SCALE` rather than mutating a resolved value.
 */
async function loadWith(scale: string | undefined): Promise<typeof import('./wait-budget.js')> {
  vi.resetModules()
  if (scale === undefined) delete process.env.CMDR_E2E_WAIT_SCALE
  else process.env.CMDR_E2E_WAIT_SCALE = scale
  return import('./wait-budget.js')
}

afterEach(() => {
  delete process.env.CMDR_E2E_WAIT_SCALE
})

describe('waitBudget', () => {
  it('leaves every budget alone when the variable is unset', async () => {
    const { waitBudget, WAIT_SCALE } = await loadWith(undefined)
    expect(WAIT_SCALE).toBe(1)
    expect(waitBudget(5000)).toBe(5000)
    expect(waitBudget(15_000)).toBe(15_000)
  })

  it('multiplies by the requested scale', async () => {
    const { waitBudget } = await loadWith('2.5')
    expect(waitBudget(5000)).toBe(12_500)
    expect(waitBudget(3000)).toBe(7500)
  })

  it('rounds to a whole millisecond', async () => {
    const { waitBudget } = await loadWith('1.333')
    expect(waitBudget(1000)).toBe(1333)
    expect(waitBudget(1500)).toBe(2000) // 1999.5 rounds up
  })

  it('clamps a scale below 1 up to 1, so a bad reading can never shorten a wait', async () => {
    const { WAIT_SCALE, waitBudget } = await loadWith('0.25')
    expect(WAIT_SCALE).toBe(1)
    expect(waitBudget(5000)).toBe(5000)
  })

  it('clamps a scale above 4 down to 4', async () => {
    const { WAIT_SCALE, waitBudget } = await loadWith('99')
    expect(WAIT_SCALE).toBe(4)
    expect(waitBudget(5000)).toBe(20_000)
  })

  it.each(['', '   ', 'busy', 'NaN', 'two'])('falls back to 1 on the unparseable value %o', async (raw) => {
    const { WAIT_SCALE, waitBudget } = await loadWith(raw)
    expect(WAIT_SCALE).toBe(1)
    expect(waitBudget(5000)).toBe(5000)
  })

  it('caps the scaled result at 10 minutes', async () => {
    const { waitBudget } = await loadWith('4')
    expect(waitBudget(120_000)).toBe(480_000) // under the cap, scales fully
    expect(waitBudget(180_000)).toBe(600_000) // 720_000 would exceed it
  })

  it('never shrinks a budget that already exceeds the cap', async () => {
    const { waitBudget } = await loadWith('4')
    expect(waitBudget(900_000)).toBe(900_000)
    expect(waitBudget(1_200_000)).toBe(1_200_000)
  })
})
