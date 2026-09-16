<script lang="ts">
    /**
     * The volume switcher's list: every place the pane can go, grouped, on the house `Menu`
     * primitive (`$lib/ui/Menu.svelte`). The chip that opens it is `VolumeBreadcrumb.svelte`.
     *
     * ❗ The primitive owns every interaction: open and close, anchoring, the cursor, the
     * keyboard (including ⌥↑/⌥↓ reorder and the submenu), keyboard-vs-pointer mode, drag,
     * focus, and placement. This component owns only the DATA (sections built from
     * `volume-grouping.ts`) and what a row shows: the filesystem tag, the badges, the
     * eject or disconnect control, the disk-space line, and the inline rename field.
     */
    import { onDestroy, onMount, untrack } from 'svelte'
    import type { UnlistenFn } from '@tauri-apps/api/event'
    import { onVolumeContextAction, resolvePathVolume, showVolumeRowContextMenu } from '$lib/tauri-commands'
    import { getVolumes, getVolumesTimedOut, isVolumesRefreshing, isVolumeRetryFailed, requestVolumeRefresh } from '$lib/stores/volume-store.svelte'
    import { isVolumeBusy, isVolumeEjecting } from '$lib/stores/volume-busy-store.svelte'
    import { isRestricted } from '$lib/stores/restricted-paths-store.svelte'
    import { getCachedIcon, iconCacheVersion } from '$lib/icon-cache'
    import { dependOn } from '$lib/utils/reactivity'
    import { isMacOS } from '$lib/shortcuts/key-capture'
    import { tString } from '$lib/intl/messages.svelte'
    import { restrictedFolderTooltip } from '$lib/system-strings.svelte'
    import { formatByteSize } from '$lib/units'
    import { tooltip } from '$lib/tooltip/tooltip'
    import Icon from '$lib/ui/Icon.svelte'
    import Menu from '$lib/ui/Menu.svelte'
    import Spinner from '$lib/ui/Spinner.svelte'
    import StatusGlyph from '$lib/ui/StatusGlyph.svelte'
    import { createMenu } from '$lib/ui/menu-controller.svelte'
    import type { MenuIcon, MenuItem, MenuRowContext, MenuSection } from '$lib/ui/menu-types'
    import type { VolumeContextActionKind } from '$lib/ipc/bindings'
    import { deviceVolumeLabel } from '$lib/adb/adb-volume-label'
    import { deviceRowState } from '$lib/adb/device-readiness'
    import { maybePromptFirstConnect } from '$lib/indexing/first-connect-trigger'
    import { silenceDrive } from '$lib/indexing/drive-index-prefs'
    import { setSetting } from '$lib/settings'
    import { getUsageBar, formatDiskSpaceShort } from '../disk-space-utils'
    import type { VolumeInfo } from '../types'
    import type { VolumeChangePayload } from '../pane/types'
    import ConnectionDot from './ConnectionDot.svelte'
    import DetachButton from './DetachButton.svelte'
    import DriveIndexBadge from './DriveIndexBadge.svelte'
    import ImageIndexDriveBadge from './ImageIndexDriveBadge.svelte'
    import UsbSpeedDot from './UsbSpeedDot.svelte'
    import { connectDirectlyToRow } from './connect-directly-row'
    import { showsDisconnect } from './connection-state'
    import { detachControl } from './detach-control'
    import { detachVolume } from './detach-volume'
    import { isDriveRow } from './drive-index-manager.svelte'
    import { isVolumeEjectable } from './eject-predicate'
    import { buildFavoriteTooltip } from './favorite-tooltip'
    import { createFavoritesController } from './favorites-controller.svelte'
    import { reportFavoriteOpened } from './favorites-analytics'
    import { filesystemLabel } from './filesystem-label'
    import { pathForPickedVolume } from './picked-volume-path'
    import { disconnectServerPlace, isServerPlaceRow, openServerRowMenu } from './server-row-actions'
    import { shouldShowCheckmark } from './volume-checkmark'
    import { groupByCategory } from './volume-grouping'
    import { createVolumeSpaceManager } from './volume-space-manager.svelte'
    import type { DriveBadges } from './drive-badges.svelte'

    interface Props {
        /** The volume the pane's path really sits on: the row wearing the checkmark. */
        containingVolumeId: string | null
        /** The index dots, shared with the chip so both placements fetch and subscribe once. */
        badges: DriveBadges
        /** The chip the list hangs under, read at open time. */
        getAnchor: () => HTMLElement | undefined
        /** The chip's whole control cluster: pressing a control in it doesn't close the list. */
        getChipCluster: () => HTMLElement | undefined
        onVolumeChange?: (change: VolumeChangePayload) => void
    }

    const { containingVolumeId, badges, getAnchor, getChipCluster, onVolumeChange }: Props = $props()

    const volumes = $derived(getVolumes())
    const volumesTimedOut = $derived(getVolumesTimedOut())
    const volumesRefreshing = $derived(isVolumesRefreshing())
    const volumeRetryFailed = $derived(isVolumeRetryFailed())

    const RESTRICTED_FOLDER_TOOLTIP = $derived(restrictedFolderTooltip())
    /* Short, because a screen reader reads it on every restricted volume; the instruction
       above is the tooltip the whole volume row carries. */
    const RESTRICTED_FOLDER_LABEL = $derived(tString('fileExplorer.restrictedFolder.label'))
    const READ_ONLY_TOOLTIP = $derived(tString('fileExplorer.navigation.readOnlyTooltip'))
    /** A server row's Disconnect control, disabled while a transfer touches the volume. */
    const DISCONNECT_BUSY_TOOLTIP = $derived(tString('fileExplorer.navigation.disconnectBusyTooltip'))

    const spaceManager = createVolumeSpaceManager()
    const {
        volumeSpaceMap,
        spaceTimedOutSet,
        spaceRetryingSet,
        spaceRetryFailedSet,
        spaceRetryAttemptedSet,
        spaceAutoRetryingSet,
    } = spaceManager

    let renameInputRef: HTMLInputElement | undefined = $state()

    // Favorites: inline rename, remove, and the local-first optimistic order. The reorder
    // MECHANICS are the primitive's; this holds what the backend and the eye need.
    const fav = createFavoritesController({
        getFavorites: () => favorites,
        getVolumes: () => volumes,
        getRenameInputRef: () => renameInputRef,
    })

    // Generic macOS folder icon used as fallback when a volume has no icon (for example,
    // FDA-gated favorites whose icons aren't fetched yet to avoid TCC popups). Reading
    // `$iconCacheVersion` re-evaluates this once the icon lands.
    const dirIconFallback = $derived.by(() => {
        dependOn($iconCacheVersion)
        return getCachedIcon('dir')
    })

    // `volumes` with favorites reordered per the optimistic override (each favorite SLOT keeps
    // its position; only which favorite fills it changes). Everything below derives from this,
    // so an optimistic reorder shows without waiting for the backend round-trip.
    const effectiveVolumes = $derived.by(() => {
        const order = fav.optimisticFavoriteIds
        if (!order) return volumes
        const rank = new Map(order.map((id, i) => [id, i]))
        const orderedFavs = volumes
            .filter((v) => v.category === 'favorite')
            .slice()
            .sort((a, b) => (rank.get(a.id) ?? Number.POSITIVE_INFINITY) - (rank.get(b.id) ?? Number.POSITIVE_INFINITY))
        let fi = 0
        return volumes.map((v) => (v.category === 'favorite' ? orderedFavs[fi++] : v))
    })

    const groupedVolumes = $derived(groupByCategory(effectiveVolumes))
    const allVolumes = $derived(groupedVolumes.flatMap((g) => g.items))
    const favorites = $derived(effectiveVolumes.filter((v) => v.category === 'favorite'))

    /** A submenu row's value, so a pick tells itself apart from the volume rows. */
    const CONNECT_PREFIX = 'connect:'

    function rowIcon(volume: VolumeInfo, restricted: boolean): MenuIcon | undefined {
        if (volume.category === 'cloud_drive') return { src: '/icons/sync-online-only.svg' }
        if (volume.category === 'mobile_device') return { src: '/icons/mobile-device.svg' }
        if (volume.category === 'network') return { lucide: 'globe' }
        // TCC-denied paths: `NSWorkspace.iconForFile` returns a confusing "no access"
        // placeholder, so the generic Aqua folder icon stands in.
        if (restricted && dirIconFallback) return { src: dirIconFallback }
        if (volume.icon) return { src: volume.icon }
        if (dirIconFallback) return { src: dirIconFallback }
        return { lucide: 'folder' }
    }

    function toMenuItem(volume: VolumeInfo): MenuItem<VolumeInfo> {
        const restricted = isRestricted(volume.path)
        // What the DEVICE's presence makes of the row: openable or greyed, and the sentence
        // that says why. `null` readiness (every disk, every server) answers "openable,
        // nothing to say", so this costs non-device rows nothing. ❗ A device the daemon
        // lists but can't use is `disabled`: the primitive greys it and never opens it,
        // rather than sending the pane somewhere that answers nothing. ❌ A
        // `waiting_for_authorization` row is NOT one of these: opening it ends the silence.
        const rowState = deviceRowState(volume.deviceReadiness)
        return {
            value: volume.id,
            label: deviceVolumeLabel(volume, volumes),
            icon: rowIcon(volume, restricted),
            checked: shouldShowCheckmark(volume, containingVolumeId),
            disabled: !rowState.openable,
            tooltip:
                rowState.tooltip ??
                (restricted
                    ? RESTRICTED_FOLDER_TOOLTIP
                    : volume.category === 'favorite'
                      ? buildFavoriteTooltip(volume.path, isMacOS())
                      : ''),
            // The OS mounted this share for us, so the row offers the direct session it could
            // have instead. One row today, and the primitive walks however many there are.
            submenu:
                volume.connectionState === 'os_mount'
                    ? [{ value: `${CONNECT_PREFIX}${volume.id}`, label: tString('fileExplorer.navigation.connectDirectly') }]
                    : undefined,
            data: volume,
        }
    }

    const sections: MenuSection<VolumeInfo>[] = $derived.by(() =>
        groupedVolumes.map((group) => ({
            id: group.category,
            heading: group.label || undefined,
            // Favorites are the user's own order, so they drag and ⌥↑/⌥↓ within their section.
            reorderable: group.category === 'favorite',
            // An emptied list is a real user state (they can remove every favorite), unlike
            // every other group, which `volume-grouping.ts` hides when empty.
            emptyLabel: group.category === 'favorite' ? tString('fileExplorer.navigation.favoritesEmpty') : undefined,
            items: group.items.map(toMenuItem),
        })),
    )

    /** Where focus was when the menu opened, so closing it doesn't move the focused pane. */
    let focusBeforeOpen: HTMLElement | null = null

    const menu = createMenu<VolumeInfo>({
        getSections: () => sections,
        onSelect: (item) => {
            void handleSelect(item)
        },
        onReorder: ({ sectionId, orderedValues }) => {
            if (sectionId === 'favorite') fav.applyReorder(orderedValues)
        },
        onContextMenu: (item) => {
            openRowMenu(item)
        },
        // While a favorite is being renamed inline, the `<input>` owns every keystroke:
        // arrows and Home/End move the text cursor, not the menu's.
        isEditing: () => fav.renamingFavoriteId !== null,
        onOpenChange: (open) => {
            if (!open) return
            void spaceManager.fetchVolumeSpaces(volumes)
            badges.fetchForRows(volumes)
        },
        restoreFocus: () => {
            // ❗ Back to whatever held focus, ❌ not to this pane: ⌥F2 opens the OTHER pane's
            // switcher, and closing it must not move the focus across.
            focusBeforeOpen?.focus()
            focusBeforeOpen = null
        },
        // ❗ The chip's own controls sit BESIDE the anchor, and ejecting from one deliberately
        // leaves the list open so several drives can go in a row.
        keepOpenWithin: getChipCluster,
    })

    export function open(): void {
        if (menu.isOpen) return
        const anchor = getAnchor()
        if (!anchor) return
        focusBeforeOpen = document.activeElement instanceof HTMLElement ? document.activeElement : null
        menu.openUnder(anchor)
        // Land the cursor on the row wearing the checkmark, so Enter re-opens where you
        // already are; with nothing checked the primitive's first row stands.
        const checked = allVolumes.find((volume) => shouldShowCheckmark(volume, containingVolumeId))
        if (checked) menu.highlight(checked.id)
    }

    export function close(): void {
        menu.close()
    }

    export function toggle(): void {
        if (menu.isOpen) menu.close()
        else open()
    }

    export function getIsOpen(): boolean {
        return menu.isOpen
    }

    async function handleSelect(item: MenuItem<VolumeInfo>): Promise<void> {
        if (item.value.startsWith(CONNECT_PREFIX)) {
            await connectDirectlyToRow(item.value.slice(CONNECT_PREFIX.length), volumes)
            return
        }
        const volume = item.data
        if (!volume) return

        if (volume.category === 'favorite') {
            reportFavoriteOpened('breadcrumb')
            // For favorites, navigate to the favorite's path but set the pane's volume to the
            // one that really contains it.
            const { volume: containingVolume } = await resolvePathVolume(volume.path)
            if (containingVolume) {
                onVolumeChange?.({
                    volumeId: containingVolume.id,
                    volumePath: containingVolume.path,
                    targetPath: volume.path,
                })
            } else {
                onVolumeChange?.({ volumeId: 'root', volumePath: '/', targetPath: volume.path })
            }
            return
        }

        // A saved server place opens on its start folder; anything else at its root.
        onVolumeChange?.({ volumeId: volume.id, volumePath: volume.path, targetPath: pathForPickedVolume(volume) })
        // First-connect indexing prompt (D6): self-gates on settings, per-drive silence, and
        // whether the drive is already indexed.
        if (isDriveRow(volume)) {
            void maybePromptFirstConnect(volume.id, volume.name, {
                onEnable: (vid) => { badges.runAction(vid, 'enable') },
                onSilenceDrive: (vid) => { silenceDrive(vid) },
                onSilenceAll: () => { setSetting('indexing.askForEachDrive', false) },
            })
        }
    }

    // Per-row right-click menu. Favorites get Rename / Remove; ejectable volumes get their
    // detach item; a server row gets its own menu; anything else has none. It's the NATIVE
    // (muda) menu, matching the breadcrumb / tab menus. While it tracks, the webview is
    // frozen, so the cursor can't drift onto another row — it acts on the right-clicked one.
    // The pick returns over `volume-context-action`: eject is handled in `DualPaneExplorer`;
    // rename / remove land in `handleVolumeContextAction` below.
    function openRowMenu(item: MenuItem<VolumeInfo>): void {
        const volume = item.data
        if (!volume) return
        const isFavorite = volume.category === 'favorite'
        const ejectable = isVolumeEjectable(volume)
        if (isServerPlaceRow(volume)) {
            void openServerRowMenu(volume)
            return
        }
        if (!isFavorite && !ejectable) return
        void showVolumeRowContextMenu(volume.id, volume.name, isFavorite, ejectable)
    }

    // Rename / remove a favorite when the user picks it from the native row menu. Both panes'
    // breadcrumbs receive this global event, but only the one whose menu is open owns the menu
    // it spawned (favorites are global, so the id alone can't tell the panes apart).
    function handleVolumeContextAction(payload: { action: VolumeContextActionKind; volumeId: string }): void {
        if (!menu.isOpen) return
        if (payload.action !== 'rename-favorite' && payload.action !== 'remove-favorite') return
        const volume = favorites.find((f) => f.id === payload.volumeId)
        if (!volume) return
        if (payload.action === 'rename-favorite') fav.startRename(volume)
        else void fav.remove(volume)
    }

    /** The row's Disconnect control. Guarded like Eject: never mid-transfer. */
    function handleDisconnectClick(volume: VolumeInfo): void {
        if (isVolumeBusy(volume.id)) return
        void disconnectServerPlace(volume.id, volume.name)
    }

    // Clear cached space info when the volume list changes (mount/unmount/MTP connect) and
    // re-fetch if the menu is open.
    let prevVolumeIds = ''
    $effect(() => {
        const ids = volumes.map((v) => v.id).join(',')
        if (prevVolumeIds && ids !== prevVolumeIds) {
            spaceManager.clearAll()
            if (untrack(() => menu.isOpen)) void spaceManager.fetchVolumeSpaces(volumes)
        }
        prevVolumeIds = ids
    })

    let unlistenVolumeContext: UnlistenFn | undefined

    onMount(() => {
        void onVolumeContextAction(handleVolumeContextAction).then((unlisten) => {
            unlistenVolumeContext = unlisten
        })
    })

    onDestroy(() => {
        spaceManager.destroy()
        menu.destroy()
        unlistenVolumeContext?.()
    })
</script>

<!-- The name the switcher already carries in Settings > Keyboard shortcuts, so screen readers
     and the shortcut scope say the same thing (and M2 adds no new copy to translate). -->
<Menu {menu} ariaLabel={tString('shortcuts.scope.volumeChooser')}>
    {#snippet label(ctx: MenuRowContext<VolumeInfo>)}
        {@const volume = ctx.item.data}
        {#if volume}
            {#if fav.renamingFavoriteId === volume.id}
                <!-- eslint-disable-next-line cmdr/prefer-ui-primitive -- Dense inline editor inside a menu row: it inherits the row's font and sits at row height with 2px side padding, which the framed `TextInput`'s padding would blow past, and it carries a resting accent border to read as "editing" rather than one that appears on focus. -->
                <input
                    class="favorite-rename-input"
                    bind:this={renameInputRef}
                    bind:value={fav.renameDraft}
                    onkeydown={(e: KeyboardEvent) => { fav.handleRenameKeyDown(e, volume) }}
                    onblur={() => { void fav.commitRename(volume) }}
                    aria-label={tString('fileExplorer.navigation.renameFavoriteAriaLabel')}
                />
            {:else}
                <!-- TCC-restricted entries read quiet + italic (the shared `--color-text-quiet`
                     token, as the file list's hidden entries do); a pinned place nobody has
                     dialed is quiet too, so the connected rows above it read as the live ones.
                     ❌ Not `aria-disabled`: opening one is what dials it. -->
                <span
                    class="volume-label"
                    class:is-restricted={isRestricted(volume.path)}
                    class:is-saved-place={volume.connectionState === 'saved'}>{ctx.item.label}</span
                >
            {/if}
        {/if}
    {/snippet}

    {#snippet trailing(ctx: MenuRowContext<VolumeInfo>)}
        {@const volume = ctx.item.data}
        {#if volume}
            {@const fsLabel = filesystemLabel(volume)}
            {#if fsLabel}
                <!-- Filesystem name tag, sitting just right of the volume name. Quiet
                     secondary text so it reads as metadata, not a second title. -->
                <span class="volume-fs" class:is-saved-place={volume.connectionState === 'saved'}>{fsLabel}</span>
            {/if}
            {#if isRestricted(volume.path)}
                <!-- No tooltip of its own: the whole row already carries this same string, and
                     the row is the honest target (the italic dimmed label needs the
                     explanation as much as the glyph). -->
                <StatusGlyph name="info" label={RESTRICTED_FOLDER_LABEL} tooltip={null} />
            {/if}
            <span class="row-trailing">
                {#if volume.mountIsReadOnly}
                    <span class="read-only-indicator" use:tooltip={READ_ONLY_TOOLTIP}
                        ><Icon name="lock" size={14} aria-hidden="true" /></span
                    >
                {/if}
                {#if volume.connectionState}
                    <ConnectionDot state={volume.connectionState} />
                {/if}
                {#if volume.usbSpeed}
                    <UsbSpeedDot speed={volume.usbSpeed} />
                {/if}
                {#if isDriveRow(volume)}
                    {@const indexStatus = badges.statusFor(volume.id)}
                    {#if indexStatus}
                        <DriveIndexBadge
                            volumeId={volume.id}
                            status={indexStatus}
                            driveName={volume.name}
                            onAction={badges.runAction}
                        />
                    {/if}
                    {@const imageState = badges.imageStateFor(volume.id)}
                    {#if imageState}
                        <ImageIndexDriveBadge volumeId={volume.id} volumeState={imageState} />
                    {/if}
                {/if}
                {#if isServerPlaceRow(volume) && showsDisconnect(volume.connectionState)}
                    <!-- A server has nothing to unplug, so its slot says Disconnect (D6). The
                         place stays saved; only the session goes. -->
                    {@const busy = isVolumeBusy(volume.id)}
                    <DetachButton
                        label={busy
                            ? DISCONNECT_BUSY_TOOLTIP
                            : tString('fileExplorer.navigation.disconnectPlaceAriaLabel', { name: volume.name })}
                        icon="unplug"
                        disabled={busy}
                        ejecting={false}
                        onclick={() => { handleDisconnectClick(volume) }}
                    />
                {:else if isVolumeEjectable(volume)}
                    <!-- ❗ Gated on `isVolumeEjectable`, which for a phone reads its READINESS:
                         a device row carries `isEjectable: true` unconditionally and no
                         `connectionState` at all, so without the gate a greyed `unavailable`
                         row nobody can open still offered a live Disconnect. -->
                    <DetachButton
                        {...detachControl(volume, {
                            busy: isVolumeBusy(volume.id),
                            ejecting: isVolumeEjecting(volume.id),
                        })}
                        onclick={() => { void detachVolume(volume) }}
                    />
                {/if}
            </span>
        {/if}
    {/snippet}

    {#snippet below(ctx: MenuRowContext<VolumeInfo>)}
        {@const volume = ctx.item.data}
        {#if volume}
            {@const space = volumeSpaceMap.get(volume.id)}
            {#if space}
                {@const bar = getUsageBar(space)}
                <div class="volume-space-info">
                    <!-- No bar where there is no total to fill it against: the line carries
                         the used figure on its own. -->
                    {#if bar}
                        <div class="volume-space-bar">
                            <div
                                class="volume-space-fill"
                                style:width="{bar.usedPercent}%"
                                style:background-color="var({bar.cssVar})"
                            ></div>
                        </div>
                    {/if}
                    <span class="volume-space-text">{formatDiskSpaceShort(space, formatByteSize)}</span>
                </div>
            {:else if spaceRetryingSet.has(volume.id)}
                <div
                    class="volume-space-info volume-space-timeout"
                    use:tooltip={spaceAutoRetryingSet.has(volume.id)
                        ? tString('fileExplorer.navigation.spaceRetryingAuto')
                        : tString('fileExplorer.navigation.spaceRetrying')}
                >
                    <div class="volume-space-bar volume-space-bar-timeout"><Spinner size="sm" /></div>
                    <span class="volume-space-text volume-space-text-timeout"
                        >{tString('fileExplorer.navigation.spaceRetryingText')}</span
                    >
                </div>
            {:else if spaceTimedOutSet.has(volume.id)}
                <!-- svelte-ignore a11y_click_events_have_key_events -->
                <!-- svelte-ignore a11y_no_static_element_interactions -->
                <div
                    class="volume-space-info volume-space-timeout"
                    class:space-shake={spaceRetryFailedSet.has(volume.id)}
                    use:tooltip={spaceRetryAttemptedSet.has(volume.id)
                        ? tString('fileExplorer.navigation.spaceStillUnavailable')
                        : tString('fileExplorer.navigation.spaceFetchFailed')}
                    onclick={(e: MouseEvent) => {
                        e.stopPropagation()
                        spaceManager.retryVolumeSpace(volume)
                    }}
                >
                    <div class="volume-space-bar volume-space-bar-timeout">
                        <span class="volume-space-timeout-icon">?</span>
                    </div>
                    <span class="volume-space-text volume-space-text-timeout"
                        >{tString('fileExplorer.navigation.spaceUnavailableText')}</span
                    >
                </div>
            {/if}
        {/if}
    {/snippet}

    {#snippet footer()}
        {#if volumesTimedOut}
            <div class="footer-separator"></div>
            <div class="timeout-warning-row" class:retry-failed={volumeRetryFailed}>
                <span class="timeout-warning-text"
                    >{volumeRetryFailed
                        ? tString('fileExplorer.navigation.volumesStillUnreachable')
                        : tString('fileExplorer.navigation.volumesMayBeMissing')}</span
                >
                <button
                    class="timeout-retry-button"
                    disabled={volumesRefreshing}
                    use:tooltip={tString('fileExplorer.navigation.refreshVolumeList')}
                    onclick={() => { requestVolumeRefresh() }}
                >
                    <span class="timeout-retry-icon" class:is-retrying={volumesRefreshing}
                        ><Icon name="rotate-cw" size={14} aria-hidden="true" /></span
                    >
                </button>
            </div>
        {/if}
    {/snippet}
</Menu>

<style>
    .volume-label {
        flex: 1;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    /*noinspection CssUnusedSymbol*/
    .volume-label.is-restricted {
        font-style: italic;
        color: var(--color-text-quiet);
    }

    /*noinspection CssUnusedSymbol*/
    .is-saved-place {
        color: var(--color-text-quiet);
    }

    .favorite-rename-input {
        flex: 1;
        min-width: 0;
        font: inherit;
        color: var(--color-text-primary);
        background-color: var(--color-bg-primary);
        border: 1px solid var(--color-accent);
        border-radius: var(--radius-sm);
        padding: 0 var(--spacing-xxs);
    }

    .favorite-rename-input:focus {
        outline: none;
    }

    .volume-fs {
        flex-shrink: 0;
        margin-left: var(--spacing-sm);
        font-size: var(--font-size-xs);
        color: var(--color-text-tertiary);
        white-space: nowrap;
    }

    .read-only-indicator {
        display: inline-flex;
        align-items: center;
        opacity: 0.7;
    }

    /* The right end of a row. `display: contents` so each badge stays a direct flex child of
       the row (no box of its own, and an empty cluster costs the row nothing), while the rule
       under it spaces the badges: the row's own `gap` plus this, so two dots sit 16px apart
       the way they did before the `Menu` primitive. The children are other components, whose
       roots this component's scoped CSS can't reach without `:global`. */
    .row-trailing {
        display: contents;
    }

    .row-trailing > :global(* + *) {
        margin-left: var(--spacing-sm);
    }

    .volume-space-info {
        display: flex;
        align-items: center;
        gap: var(--spacing-sm);
        /* stylelint-disable-next-line declaration-property-value-disallowed-list -- left pad aligns to a computed icon+gap offset; 14px/16px are measured widths */
        padding: 0 var(--spacing-md) var(--spacing-xs) calc(14px + var(--spacing-sm) + 16px + var(--spacing-sm));
    }

    .volume-space-bar {
        flex: 1;
        height: 2px;
        background-color: var(--color-disk-track);
        border-radius: var(--radius-sm);
    }

    .volume-space-fill {
        height: 100%;
        border-radius: var(--radius-sm);
    }

    .volume-space-text {
        font-size: var(--font-size-xs);
        color: var(--color-text-tertiary);
        white-space: nowrap;
        flex-shrink: 0;
    }

    /* Volume space timeout placeholder */
    .volume-space-timeout {
        cursor: default;
    }

    .volume-space-bar-timeout {
        border: 1px dashed var(--color-border);
        background-color: transparent;
        display: flex;
        align-items: center;
        justify-content: center;
        height: 8px;
    }

    .volume-space-timeout-icon {
        font-size: var(--font-size-xs);
        color: var(--color-warning);
        line-height: var(--font-line-height-flat);
        transition: opacity var(--transition-base);
    }

    /* Shake on retry failure */
    /*noinspection CssUnusedSymbol*/
    .space-shake {
        animation: shake 300ms ease;
    }

    @keyframes shake {
        0%,
        100% {
            transform: translateX(0);
        }
        25% {
            transform: translateX(-3px);
        }
        75% {
            transform: translateX(3px);
        }
    }

    .volume-space-text-timeout {
        color: var(--color-warning);
    }

    /* The footer's own rule, matching the primitive's between-section separators. */
    .footer-separator {
        height: 1px;
        background-color: var(--color-border-strong);
        margin: var(--spacing-xs) var(--spacing-sm);
    }

    .timeout-warning-row {
        display: flex;
        align-items: center;
        gap: var(--spacing-sm);
        padding: var(--spacing-xs) var(--spacing-md);
    }

    .timeout-warning-text {
        font-size: var(--font-size-xs);
        color: var(--color-warning);
        flex: 1;
    }

    .timeout-retry-button {
        background: none;
        border: none;
        padding: 0 var(--spacing-xs);
        cursor: default;
        color: var(--color-warning-text);
        font-size: var(--font-size-md);
        line-height: var(--font-line-height-flat);
        border-radius: var(--radius-sm);
        transition: background-color var(--transition-base);
    }

    .timeout-retry-button:hover {
        background-color: var(--color-bg-tertiary);
    }

    .timeout-retry-button:focus-visible {
        outline: 2px solid var(--color-accent);
        outline-offset: 1px;
    }

    .timeout-retry-button:disabled {
        opacity: 0.4;
        cursor: not-allowed;
    }

    .timeout-retry-icon {
        display: inline-flex;
        align-items: center;
        justify-content: center;
    }

    /*noinspection CssUnusedSymbol*/
    .timeout-retry-icon.is-retrying {
        animation: spin 0.8s linear infinite;
    }

    /*noinspection CssUnusedSymbol*/
    .timeout-warning-row.retry-failed {
        animation: flash-warning 0.3s ease;
    }

    @keyframes flash-warning {
        0%,
        100% {
            background-color: transparent;
        }
        50% {
            background-color: var(--color-warning-bg);
        }
    }

    @media (prefers-reduced-motion: reduce) {
        /*noinspection CssUnusedSymbol*/
        .timeout-retry-icon.is-retrying {
            animation: none;
        }

        /*noinspection CssUnusedSymbol*/
        .timeout-warning-row.retry-failed {
            animation: none;
        }

        /* Reduced motion: opacity flash instead of shake */
        /*noinspection CssUnusedSymbol*/
        .space-shake {
            animation: flash-warning 300ms ease;
        }
    }
</style>
