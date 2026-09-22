<script lang="ts">
    /**
     * A one-place server row's right-click menu in the servers hub: the house `Menu` at the
     * pointer, holding the same list the volume switcher row's → submenu shows
     * (`../navigation/row-menu.ts`), so the two surfaces can't drift. WHICH entries and what a
     * pick does are `servers-hub-actions.ts`'s; this is the surface.
     */
    import { onDestroy } from 'svelte'
    import Menu from '$lib/ui/Menu.svelte'
    import { createMenu } from '$lib/ui/menu-controller.svelte'
    import { rowMenuSections, type RowMenuEntry } from '../navigation/row-menu'
    import type { HubActions } from './servers-hub-actions'
    import type { HubRow } from './servers-hub-rows'

    interface Props {
        actions: HubActions
    }

    const { actions }: Props = $props()

    /** The row whose menu is up, and where focus was when it opened. */
    let menuRow = $state<HubRow | null>(null)
    let focusBeforeOpen: HTMLElement | null = null

    // Read live, so a transfer starting under the open menu greys its Disconnect.
    const menu = createMenu<RowMenuEntry>({
        getSections: () => {
            const row = menuRow
            const rowMenu = row ? actions.rowMenu(row) : null
            return row?.volumeId && rowMenu ? rowMenuSections(row.volumeId, rowMenu, (entry) => entry) : []
        },
        onSelect: (item) => {
            if (menuRow && item.data) void actions.runRowEntry(menuRow, item.data)
        },
        restoreFocus: () => {
            focusBeforeOpen?.focus()
            focusBeforeOpen = null
        },
    })

    /**
     * Opens `row`'s menu at the pointer. Answers false for a row with no in-app menu (an SMB
     * host, which keeps its native one), so the caller raises that instead.
     */
    export function openAt(row: HubRow, event: MouseEvent): boolean {
        if (!actions.rowMenu(row)) return false
        menuRow = row
        focusBeforeOpen = document.activeElement instanceof HTMLElement ? document.activeElement : null
        menu.openAt({ x: event.clientX, y: event.clientY })
        return true
    }

    onDestroy(() => {
        menu.destroy()
    })
</script>

<!-- Named for the server it acts on, which is what a screen reader needs to hear. -->
<Menu {menu} ariaLabel={menuRow?.name ?? ''} />
