/**
 * Behavior tests for `Menu.svelte`, the surface. The controller's own contract (keys,
 * pointer mode, reorder) is pinned in `menu-controller.svelte.test.ts`; this file covers
 * what the component renders and the two rules that belong to the DOM: a row's own controls
 * acting for themselves, and the snippets decorating rather than replacing a row.
 */

import { describe, it, expect, vi, afterEach } from 'vitest'
import { mount, tick, unmount, createRawSnippet } from 'svelte'
import Menu from './Menu.svelte'
import { createMenu, type MenuController } from './menu-controller.svelte'
import type { MenuRowContext, MenuSection } from './menu-types'

function sections(): MenuSection[] {
  return [
    {
      id: 'favorites',
      heading: 'Favorites',
      reorderable: true,
      items: [
        { value: 'projects', label: 'Projects', icon: { lucide: 'folder' } },
        { value: 'downloads', label: 'Downloads' },
      ],
    },
    {
      id: 'volumes',
      heading: 'Volumes',
      items: [
        { value: 'hd', label: 'Macintosh HD', checked: true },
        { value: 'backup', label: 'Backup', disabled: true },
        {
          value: 'share',
          label: 'Team share',
          submenu: [
            { value: 'connect', label: 'Connect directly' },
            { value: 'forget', label: 'Forget this share' },
          ],
        },
      ],
    },
    { id: 'empty', heading: 'Nothing here', items: [], emptyLabel: '(This section is empty)' },
  ]
}

let mounted: (() => void)[] = []

/** Mounts the surface around a controller, opened at a point. Returns both. */
async function open(
  props: Record<string, unknown> = {},
  deps: Record<string, unknown> = {},
): Promise<{ menu: MenuController }> {
  const target = document.createElement('div')
  document.body.appendChild(target)
  const menu = createMenu({ getSections: sections, onSelect: () => {}, ...deps })
  menu.openAt({ x: 50, y: 50 })
  const component = mount(Menu, { target, props: { menu, ariaLabel: 'Volumes', ...props } })
  await tick()
  await tick()
  mounted.push(() => {
    menu.destroy()
    void unmount(component)
  })
  return { menu }
}

function surface(): HTMLElement | null {
  return document.querySelector('[data-menu]')
}

function row(value: string): HTMLElement | null {
  return document.querySelector(`[data-menu-row="${value}"]`)
}

afterEach(() => {
  for (const dispose of mounted) dispose()
  mounted = []
  document.body.innerHTML = ''
})

describe('rendering', () => {
  it('renders nothing while the menu is closed, so a consumer writes no {#if}', async () => {
    const target = document.createElement('div')
    document.body.appendChild(target)
    const menu = createMenu({ getSections: sections, onSelect: () => {} })
    const component = mount(Menu, { target, props: { menu, ariaLabel: 'Volumes' } })
    await tick()
    expect(surface()).toBeNull()
    menu.destroy()
    void unmount(component)
  })

  it('renders a labelled menu with one group per section', async () => {
    await open()
    expect(surface()?.getAttribute('aria-label')).toBe('Volumes')
    expect(document.querySelectorAll('[role="group"]')).toHaveLength(3)
    expect(document.body.textContent).toContain('Favorites')
    expect(document.body.textContent).toContain('Macintosh HD')
  })

  it('renders every row as a menuitem, marking the disabled one', async () => {
    await open()
    expect(row('hd')).not.toBeNull()
    expect(row('backup')?.getAttribute('aria-disabled')).toBe('true')
    expect(row('projects')?.getAttribute('aria-disabled')).toBeNull()
  })

  it('points aria-activedescendant at the highlighted row', async () => {
    const { menu } = await open()
    menu.highlight('downloads')
    await tick()
    const active = surface()?.getAttribute('aria-activedescendant')
    expect(active).toBe(row('downloads')?.id)
  })

  it('shows an empty section as a real, unfocusable state', async () => {
    await open()
    expect(document.body.textContent).toContain('(This section is empty)')
    const empty = document.querySelector('.menu-empty')
    expect(empty?.getAttribute('aria-disabled')).toBe('true')
  })

  it('marks a submenu parent for assistive tech', async () => {
    const { menu } = await open()
    expect(row('share')?.getAttribute('aria-haspopup')).toBe('menu')
    expect(row('share')?.getAttribute('aria-expanded')).toBe('false')
    menu.surface.openSubmenu('share', true)
    await tick()
    expect(row('share')?.getAttribute('aria-expanded')).toBe('true')
  })
})

/**
 * What counts as "outside". A pointer-down there closes the menu and still reaches what it
 * landed on, which is a deliberate break from the macOS menu (that one swallows the click).
 */
describe('closing on an outside pointer-down', () => {
  /** A chip: the anchor plus a control beside it, the shape the volume switcher's header has. */
  function chip(): { cluster: HTMLElement; anchor: HTMLElement; control: HTMLButtonElement } {
    const cluster = document.createElement('div')
    const anchor = document.createElement('span')
    const control = document.createElement('button')
    cluster.append(anchor, control)
    document.body.appendChild(cluster)
    return { cluster, anchor, control }
  }

  it('closes on a pointer-down somewhere else entirely', async () => {
    const { menu } = await open()
    document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }))
    expect(menu.isOpen).toBe(false)
  })

  // ❗ The controls BESIDE the anchor belong to the menu too. Without this, pressing the
  // switcher chip's eject button closed the list before the button could act — and ejecting
  // deliberately leaves it open so several drives can go in a row.
  it('stays open for a pointer-down anywhere in the anchor’s control cluster', async () => {
    const { cluster, anchor, control } = chip()
    const target = document.createElement('div')
    document.body.appendChild(target)
    const menu = createMenu({ getSections: sections, onSelect: () => {}, keepOpenWithin: () => cluster })
    menu.openUnder(anchor)
    const component = mount(Menu, { target, props: { menu, ariaLabel: 'Volumes' } })
    await tick()
    mounted.push(() => {
      menu.destroy()
      void unmount(component)
    })

    control.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }))
    expect(menu.isOpen).toBe(true)
    anchor.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }))
    expect(menu.isOpen).toBe(true)
  })
})

describe('pointer selection', () => {
  it('activates the row that was clicked', async () => {
    const onSelect = vi.fn()
    await open({}, { onSelect })
    row('hd')?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ value: 'hd' }))
  })

  it('never activates a row from a click on a control inside it', async () => {
    const onSelect = vi.fn()
    const onEject = vi.fn()
    // A trailing button, the shape the switcher's eject control takes.
    const trailing = createRawSnippet<[MenuRowContext]>(() => ({
      render: () => '<button type="button" class="eject">Eject</button>',
    }))
    await open({ trailing }, { onSelect })
    const button = document.querySelector('.eject') as HTMLElement
    button.addEventListener('click', onEject)
    button.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    expect(onEject).toHaveBeenCalled()
    // The row must stay put: no call site should need `stopPropagation`.
    expect(onSelect).not.toHaveBeenCalled()
  })
})

/**
 * The `data-*` hooks are a contract other suites select on (`lib/ui/DETAILS.md` § Menu),
 * so they're asserted here rather than left to rot as decoration.
 */
describe('test hooks', () => {
  it('names the surface, each row by value, and the checked and disabled states', async () => {
    await open()
    expect(surface()).not.toBeNull()
    expect(row('hd')?.hasAttribute('data-checked')).toBe(true)
    expect(row('backup')?.hasAttribute('data-disabled')).toBe(true)
    expect(row('projects')?.hasAttribute('data-checked')).toBe(false)
    expect(document.querySelector('[data-menu-section="volumes"]')).not.toBeNull()
    expect(document.querySelector('[data-menu-section="volumes"] [data-menu-heading]')?.textContent).toBe('Volumes')
    expect(document.querySelector('[data-menu-empty]')).not.toBeNull()
  })

  it('moves data-highlighted with the cursor, and marks keyboard mode', async () => {
    const { menu } = await open()
    menu.highlight('downloads')
    await tick()
    expect(document.querySelector('[data-menu-row][data-highlighted]')?.getAttribute('data-menu-row')).toBe('downloads')
    expect(surface()?.hasAttribute('data-keyboard-mode')).toBe(false)
    menu.handleKey(new KeyboardEvent('keydown', { key: 'ArrowDown', cancelable: true }))
    await tick()
    expect(surface()?.hasAttribute('data-keyboard-mode')).toBe(true)
  })

  it('names the submenu surface and its highlighted row', async () => {
    const { menu } = await open()
    menu.surface.openSubmenu('share', true)
    // Twice: the submenu renders only once its position effect has measured the parent row,
    // and that effect resolves inside a `tick().then(...)` of its own.
    await tick()
    await tick()
    const submenu = document.querySelector('[data-menu-submenu]')
    expect(submenu).not.toBeNull()
    expect(submenu?.querySelector('[data-menu-row="connect"][data-highlighted]')).not.toBeNull()
    // Exactly one row lights up: a boolean here lit every row of a multi-item submenu.
    expect(submenu?.querySelectorAll('[data-highlighted]')).toHaveLength(1)
  })

  it('marks the dragged row, and the cue row carries its insertion slot', async () => {
    const { menu } = await open()
    // Two favorite rows 20px tall from y=0: midpoints 10 and 30.
    menu.surface.bindSurface({ getRowMidpoints: () => [10, 30] })
    menu.surface.startDrag('projects', new MouseEvent('mousedown', { clientY: 10, button: 0 }))
    window.dispatchEvent(new MouseEvent('mousemove', { clientY: 45 }))
    await tick()
    expect(row('projects')?.hasAttribute('data-dragging')).toBe(true)
    // Dropping past the last row: the cue sits below it, at slot 2.
    const cueRow = document.querySelector('[data-drop-cue]')
    expect(cueRow?.getAttribute('data-menu-row')).toBe('downloads')
    expect(cueRow?.getAttribute('data-drop-cue')).toBe('below')
    expect(cueRow?.getAttribute('data-drop-slot')).toBe('2')
    window.dispatchEvent(new MouseEvent('mouseup', { clientY: 45 }))
  })
})

describe('snippets', () => {
  it('label replaces the row text', async () => {
    const label = createRawSnippet<[MenuRowContext]>((context) => ({
      render: () => `<span class="custom-label">Renaming ${context().item.label}</span>`,
    }))
    await open({ label })
    expect(document.querySelector('.custom-label')?.textContent).toContain('Renaming Projects')
  })

  it('below adds a sub-line under the row, and footer sits under the last section', async () => {
    const below = createRawSnippet<[MenuRowContext]>((context) => ({
      render: () => (context().item.value === 'hd' ? '<div class="space">312 GB free</div>' : '<div></div>'),
    }))
    const footer = createRawSnippet(() => ({ render: () => '<div class="footer">Still loading</div>' }))
    await open({ below, footer })
    expect(document.querySelector('.space')?.textContent).toBe('312 GB free')
    expect(document.querySelector('.footer')?.textContent).toBe('Still loading')
  })
})
