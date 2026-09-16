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
        { value: 'share', label: 'Team share', submenu: [{ value: 'connect', label: 'Connect directly' }] },
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
  return document.querySelector('[role="menu"]')
}

function row(value: string): HTMLElement | null {
  return document.querySelector(`[data-menu-value="${value}"]`)
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
