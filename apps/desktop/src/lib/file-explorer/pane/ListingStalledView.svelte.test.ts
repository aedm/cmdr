/**
 * The screen a pane shows while its folder's server or drive isn't answering. It's
 * the only thing between the user and a spinner they'd read as "this folder is
 * empty", so it has to say what's happening and offer both ways forward.
 */

import { describe, it, expect, vi } from 'vitest'
import { mount, tick } from 'svelte'
import ListingStalledView from './ListingStalledView.svelte'

function mountView() {
  const onRetry = vi.fn()
  const onGoBack = vi.fn()
  const target = document.createElement('div')
  document.body.appendChild(target)
  mount(ListingStalledView, { target, props: { folderPath: '/Volumes/nas/photos', onRetry, onGoBack } })
  return { target, onRetry, onGoBack }
}

function button(target: HTMLElement, label: string): HTMLButtonElement {
  const found = Array.from(target.querySelectorAll('button')).find((b) => b.textContent.includes(label))
  if (!found) throw new Error(`no "${label}" button`)
  return found
}

describe('ListingStalledView', () => {
  it('says the folder is still coming, and where', async () => {
    const { target } = mountView()
    await tick()

    expect(target.textContent).toContain('Still waiting for this folder')
    expect(target.textContent).toContain('/Volumes/nas/photos')
    expect(target.querySelector('[role="status"]')).not.toBeNull()
  })

  it('retries and goes back on request', async () => {
    const { target, onRetry, onGoBack } = mountView()
    await tick()

    button(target, 'Try again').click()
    button(target, 'Go back').click()

    expect(onRetry).toHaveBeenCalledOnce()
    expect(onGoBack).toHaveBeenCalledOnce()
  })
})
