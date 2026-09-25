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
    import type { MenuAnchor } from '$lib/tauri-commands/file-actions'

    interface Props {
        actions: HubActions
    }

    const { actions }: Props = $props()

    /**
     * The id of the row whose menu is up, and where focus was when it opened. ❗ The
     * id, ❌ never the row object: a Pin leaves the menu up, and a held row kept
     * offering the pin it had just flipped. The row is read live by id.
     */
    let menuRowId = $state<string | null>(null)
    let focusBeforeOpen: HTMLElement | null = null
    const menuRow = $derived(menuRowId === null ? null : actions.rowById(menuRowId))

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
     * Opens `row`'s menu: the in-app one at `point` for a one-place row or a share, else
     * (an SMB host) its native host menu at `anchor`. A right-click passes the pointer as
     * `point` and `null` as `anchor`, so macOS places the native one at the pointer; ⌃⏎
     * passes the spot under the row as both.
     */
    export async function open(row: HubRow, point: MenuAnchor, anchor: MenuAnchor | null): Promise<void> {
        if (!actions.rowMenu(row)) {
            await actions.openHostMenu(row, anchor)
            return
        }
        menuRowId = row.id
        focusBeforeOpen = document.activeElement instanceof HTMLElement ? document.activeElement : null
        menu.openAt(point)
    }

    onDestroy(() => {
        menu.destroy()
    })
</script>

<!-- Named for the server it acts on, which is what a screen reader needs to hear. -->
<Menu {menu} ariaLabel={menuRow?.name ?? ''} />
