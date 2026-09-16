<script lang="ts">
    import SectionCard from '$lib/ui/SectionCard.svelte'
    import Menu from '$lib/ui/Menu.svelte'
    import type { MenuRowContext, MenuSection } from '$lib/ui/menu-types'
    import { createMenu } from '$lib/ui/menu-controller.svelte'
    import DemoAnchor from '../DemoAnchor.svelte'

    /** The demo's own copy of the reorderable section, so a drag or ⌥↑/⌥↓ visibly moves a row. */
    let places = $state([
        { value: 'downloads', label: 'Downloads' },
        { value: 'projects', label: 'Projects' },
        { value: 'screenshots', label: 'Screenshots' },
    ])

    let lastChoice = $state<string | null>(null)
    let lastContextMenu = $state<string | null>(null)
    let renaming = $state<string | null>(null)
    let renameDraft = $state('')

    let anchorEl = $state<HTMLButtonElement>()

    const sections = $derived<MenuSection[]>([
        {
            id: 'places',
            heading: 'Places',
            reorderable: true,
            emptyLabel: '(Your places will show here)',
            items: places.map((place) => ({
                ...place,
                icon: { lucide: 'folder' as const },
                tooltip: `${place.label} — drag or ⌥↑/⌥↓ to reorder`,
            })),
        },
        {
            id: 'volumes',
            heading: 'Volumes',
            items: [
                { value: 'macintosh-hd', label: 'Macintosh HD', checked: true, icon: { lucide: 'server' } },
                { value: 'backup', label: 'Backup (unavailable)', disabled: true, icon: { lucide: 'server' } },
                {
                    value: 'share',
                    label: 'Team share',
                    icon: { lucide: 'globe' },
                    submenu: [{ value: 'connect', label: 'Connect directly' }],
                },
            ],
        },
        { id: 'empty', heading: 'Nothing here', items: [], emptyLabel: '(This section is empty)' },
    ])

    const menu = createMenu({
        getSections: () => sections,
        onSelect: (item) => {
            lastChoice = item.label
        },
        onReorder: ({ orderedValues }) => {
            places = orderedValues.map((value) => places.find((place) => place.value === value)).filter((p) => p != null)
        },
        onContextMenu: (item) => {
            lastContextMenu = item.label
        },
        isEditing: () => renaming !== null,
        restoreFocus: () => anchorEl?.focus(),
    })

    function startRename(value: string, label: string): void {
        renaming = value
        renameDraft = label
    }
</script>

<SectionCard id="components-menu" label="Menu">
    <div class="cell">
        <p class="caption">
            The house menu: sections with headings, a checkmark column, disabled rows, an empty section, a submenu, and
            a reorderable section (drag a place, or ⌥↑/⌥↓). The three row snippets are all in use below: a rename field
            for <code>label</code>, a badge for <code>trailing</code>, and a disk-space line for <code>below</code>.
        </p>
        <DemoAnchor
            bind:el={anchorEl}
            onclick={() => {
                if (anchorEl) menu.toggleUnder(anchorEl)
            }}>Open menu</DemoAnchor
        >
        {#if lastChoice}<p class="caption">Last choice: {lastChoice}</p>{/if}
        {#if lastContextMenu}<p class="caption">Last right-click: {lastContextMenu}</p>{/if}

        <Menu {menu} ariaLabel="Demo menu" minWidth={260}>
            {#snippet label(context: MenuRowContext)}
                {#if renaming === context.item.value}
                    <!-- eslint-disable-next-line cmdr/prefer-ui-primitive -- Dense inline editor inside a menu row, the shape the switcher's favorite rename uses: it inherits the row's font and sits at row height, which the framed `TextInput`'s padding would blow past. -->
                    <input
                        class="rename-input"
                        bind:value={renameDraft}
                        aria-label="Rename place"
                        onkeydown={(event: KeyboardEvent) => {
                            event.stopPropagation()
                            if (event.key === 'Enter' || event.key === 'Escape') renaming = null
                        }}
                        onblur={() => {
                            renaming = null
                        }}
                    />
                {:else}
                    <span class="row-label">{context.item.label}</span>
                {/if}
            {/snippet}

            {#snippet trailing(context: MenuRowContext)}
                {#if context.section.id === 'places'}
                    <button
                        type="button"
                        class="row-button"
                        onclick={() => {
                            startRename(context.item.value, context.item.label)
                        }}>Rename</button
                    >
                {:else if context.item.value === 'macintosh-hd'}
                    <span class="row-badge">APFS</span>
                {/if}
            {/snippet}

            {#snippet below(context: MenuRowContext)}
                {#if context.item.value === 'macintosh-hd'}
                    <div class="space-line">
                        <div class="space-bar"><div class="space-fill"></div></div>
                        <span class="space-text">312 GB free</span>
                    </div>
                {/if}
            {/snippet}

            {#snippet footer()}
                <div class="menu-footer">Some volumes may still be loading.</div>
            {/snippet}
        </Menu>
    </div>
</SectionCard>

<style>
    .caption {
        margin: 0 0 var(--spacing-sm);
        font-size: var(--font-size-xs);
        color: var(--color-text-tertiary);
    }

    .row-label {
        flex: 1;
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .rename-input {
        flex: 1;
        min-width: 0;
        font: inherit;
        color: var(--color-text-primary);
        background-color: var(--color-bg-primary);
        border: 1px solid var(--color-accent);
        border-radius: var(--radius-sm);
        padding: 0 var(--spacing-xxs);
    }

    .rename-input:focus {
        outline: none;
    }

    .row-button {
        flex-shrink: 0;
        background: none;
        border: none;
        padding: 0 var(--spacing-xs);
        color: var(--color-text-secondary);
        font-size: var(--font-size-xs);
        border-radius: var(--radius-sm);
    }

    .row-button:hover {
        background-color: var(--color-bg-tertiary);
        color: var(--color-text-primary);
    }

    .row-badge {
        flex-shrink: 0;
        margin-left: auto;
        font-size: var(--font-size-xs);
        color: var(--color-text-tertiary);
    }

    .space-line {
        display: flex;
        align-items: center;
        gap: var(--spacing-sm);
        /* stylelint-disable-next-line declaration-property-value-disallowed-list -- left pad aligns to the row's computed checkmark+icon offset; 14px/16px are measured widths */
        padding: 0 var(--spacing-md) var(--spacing-xs) calc(14px + var(--spacing-sm) + 16px + var(--spacing-sm));
    }

    .space-bar {
        flex: 1;
        height: 2px;
        background-color: var(--color-disk-track);
        border-radius: var(--radius-sm);
    }

    .space-fill {
        width: 40%;
        height: 100%;
        border-radius: var(--radius-sm);
        background-color: var(--color-allow);
    }

    .space-text {
        flex-shrink: 0;
        font-size: var(--font-size-xs);
        color: var(--color-text-tertiary);
        white-space: nowrap;
    }

    .menu-footer {
        padding: var(--spacing-xs) var(--spacing-md);
        font-size: var(--font-size-xs);
        color: var(--color-warning);
    }
</style>
