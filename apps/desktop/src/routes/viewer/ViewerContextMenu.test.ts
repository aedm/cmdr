import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, tick, unmount } from 'svelte'

import ViewerContextMenu from './ViewerContextMenu.svelte'

/** Every instance this file mounts, torn down in `afterEach`. */
const mounted: ReturnType<typeof mount>[] = []

beforeEach(() => {
  document.body.innerHTML = ''
})

// ❗ Teardown has to survive a failing assertion. The menu's `<svelte:window>` keydown
// handler calls `stopImmediatePropagation()` on Escape, so one instance left mounted by a
// red test swallows the Escape in every test after it, turning one real failure into three.
afterEach(() => {
  for (const instance of mounted.splice(0)) void unmount(instance)
})

interface MountOpts {
  hasSelection?: boolean
  onCopy?: () => void
  onSelectAll?: () => void
  onClose?: () => void
}

/** Mounts the menu into an existing element and registers it for teardown. */
function mountInto(target: HTMLElement, opts: MountOpts = {}): void {
  mounted.push(
    mount(ViewerContextMenu, {
      target,
      props: {
        x: 50,
        y: 50,
        hasSelection: opts.hasSelection ?? true,
        onCopy: opts.onCopy ?? (() => {}),
        onSelectAll: opts.onSelectAll ?? (() => {}),
        onClose: opts.onClose ?? (() => {}),
      },
    }),
  )
}

async function mountMenu(opts: MountOpts = {}) {
  const target = document.createElement('div')
  document.body.appendChild(target)
  mountInto(target, opts)
  await tick()
  return { target }
}

describe('ViewerContextMenu outside press', () => {
  /** A stand-in for the viewer's `.file-content`, which cancels its own `pointerdown`. */
  function cancelingSurface(): HTMLDivElement {
    const surface = document.createElement('div')
    document.body.appendChild(surface)
    // ❗ `viewer-pointer-drag` calls `preventDefault()` here to block the native focus
    // move so drag-select works, and that suppresses the compatibility `mousedown` for
    // the whole gesture. A menu listening for `mousedown` never hears a press on the
    // text and stays open forever.
    surface.addEventListener('pointerdown', (e) => { e.preventDefault(); })
    return surface
  }

  it('closes on a press outside even when the target cancels the event', async () => {
    const onClose = vi.fn()
    await mountMenu({ onClose })
    const surface = cancelingSurface()

    surface.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, cancelable: true }))
    await tick()

    expect(onClose).toHaveBeenCalledTimes(1)
  })

  it('stays open for a press on the menu itself', async () => {
    const onClose = vi.fn()
    const { target } = await mountMenu({ onClose })

    const item = target.querySelector<HTMLButtonElement>('.menu-item')
    item?.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }))
    await tick()

    expect(onClose).not.toHaveBeenCalled()
  })

  it('does not close itself on the very right-click that opened it', async () => {
    // The gesture is `pointerdown` (button 2) → `contextmenu` → mount, so the window
    // listener is attached only after that press has finished dispatching. If the menu
    // ever moves to opening on `pointerdown`, this catches the menu blinking shut again.
    const onClose = vi.fn()
    const surface = cancelingSurface()
    surface.addEventListener('contextmenu', () => { mountInto(surface, { onClose }); })

    surface.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, cancelable: true, button: 2 }))
    surface.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))
    await tick()

    expect(surface.querySelector('[role="menu"]')).not.toBeNull()
    expect(onClose).not.toHaveBeenCalled()
  })

  it('closes on a right-click elsewhere, so the page can reopen it at the new spot', async () => {
    const onClose = vi.fn()
    await mountMenu({ onClose })
    const surface = cancelingSurface()

    surface.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, cancelable: true, button: 2 }))
    await tick()

    expect(onClose).toHaveBeenCalledTimes(1)
  })
})

describe('ViewerContextMenu keyboard', () => {
  it('Escape closes the menu', async () => {
    const onClose = vi.fn()
    await mountMenu({ onClose })

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    await tick()

    expect(onClose).toHaveBeenCalledTimes(1)
  })

  it('Escape calls stopImmediatePropagation so a later sibling listener does not fire', async () => {
    // This protects against future regressions where the page's Escape listener is
    // registered AFTER the menu's. Today the page registers first (the menu mounts
    // later) so the page also has its own `contextMenuPos` short-circuit; this test
    // is the defense-in-depth half.
    await mountMenu()
    const laterListener = vi.fn()
    window.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') laterListener()
    })

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    await tick()

    expect(laterListener).not.toHaveBeenCalled()
  })

  it('ArrowUp moves focus to the previous item, wrapping at the start', async () => {
    const { target } = await mountMenu()
    const items = target.querySelectorAll<HTMLButtonElement>('.menu-item')
    expect(items.length).toBe(2)

    // ArrowUp from the first item should wrap to the last one (Select all).
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }))
    await tick()
    expect(document.activeElement).toBe(items[1])

    // ArrowUp again wraps back to the first item.
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }))
    await tick()
    expect(document.activeElement).toBe(items[0])
  })

  it('ArrowDown moves focus to the next item, wrapping at the end', async () => {
    const { target } = await mountMenu()
    const items = target.querySelectorAll<HTMLButtonElement>('.menu-item')

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }))
    await tick()
    expect(document.activeElement).toBe(items[1])

    // ArrowDown from the last item wraps to the first.
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }))
    await tick()
    expect(document.activeElement).toBe(items[0])
  })
})
