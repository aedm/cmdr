<script lang="ts">
    /**
     * The servers hub: every server the user saved, plus every host mDNS found,
     * in one table. Rendered when a pane is on the `network` volume.
     *
     * The merge, the ordering, and the MCP encoding are pure and live next door
     * (`servers-hub-rows.ts`, `servers-hub-mcp.ts`); this file is the table, the
     * cursor, and the keys.
     */
    import { onMount, onDestroy, untrack } from 'svelte'
    import { dependOn } from '$lib/utils/reactivity'
    import Button from '$lib/ui/Button.svelte'
    import Icon from '$lib/ui/Icon.svelte'
    import Spinner from '$lib/ui/Spinner.svelte'
    import LinkButton from '$lib/ui/LinkButton.svelte'
    import DateLabel from '$lib/ui/DateLabel.svelte'
    import {
        getNetworkHosts,
        getDiscoveryState,
        getListedAccount,
        getShareCount,
        clearShareState,
        fetchShares,
        refreshAllStaleShares,
    } from './network-store.svelte'
    import { getStatusTooltip } from './host-status'
    import {
        buildHubRows,
        hubRowIcon,
        lastUsedSeconds,
        openMoveFor,
        type HubRow,
        type HubRowStatus,
    } from './servers-hub-rows'
    import { hubPaneState } from './servers-hub-mcp'
    import { createHubActions, type HubRowMenuAPI } from './servers-hub-actions'
    import { cursorAcrossRebuild, cursorAfterArrow } from './servers-hub-keys'
    import ServersHubRowMenu from './ServersHubRowMenu.svelte'
    import ServersHubStatusBar from './ServersHubStatusBar.svelte'
    import { tooltip } from '$lib/tooltip/tooltip'
    import { rowAnchorIn } from '../pane/context-menu-anchor'
    import type { NetworkHost } from '../types'
    import {
        updateLeftPaneState,
        updateRightPaneState,
        onNetworkHostContextAction,
        listSavedServers,
        type SavedServer,
    } from '$lib/tauri-commands'
    import { getVolumes } from '$lib/stores/volume-store.svelte'
    import { getNetworkEnabled } from '$lib/settings/reactive-settings.svelte'
    import { openSettingsWindow, settingAnchorId } from '$lib/settings/settings-window'
    import { handleNavigationShortcut } from '../navigation/keyboard-shortcuts'
    import { protocolLabel } from '../navigation/filesystem-label'
    import { eventMatchesCommand } from '$lib/shortcuts'
    import { claimKey } from '$lib/shortcuts/claim-key'
    import { triggerNetworkDiscovery } from './lazy-trigger'
    import { signedInAsLabel } from './signed-in-as-label'
    import { tString } from '$lib/intl/messages.svelte'
    import type { MessageKey } from '$lib/intl/keys.gen'
    import { getAppLogger } from '$lib/logging/logger'
    import { LogOnceGate } from '$lib/logging/log-once'

    const log = getAppLogger('servers')

    /** Row height (matches Full list). */
    const ROW_HEIGHT = 20

    /** The word each status wears in the Status column. */
    const STATUS_TEXT_KEY: Record<HubRowStatus, MessageKey> = {
        connected: 'servers.hub.status.connected',
        saved: 'servers.hub.status.saved',
        found_nearby: 'servers.hub.status.foundNearby',
        signed_out: 'servers.hub.status.signedOut',
        waiting_for_key: 'servers.hub.status.waitingForKey',
    }

    interface Props {
        paneId?: 'left' | 'right'
        isFocused?: boolean
        /** Enter on an SMB host: open its places list. */
        onHostSelect?: (host: NetworkHost, label?: string) => void
        /** Enter on a one-place server, or on a saved share the volume list has a place for: take the pane there. */
        onServerSelect?: (row: HubRow) => void
        /**
         * Enter on a saved share no mount went through yet: open its host's
         * share list and mount that one share, as the host's account.
         */
        onShareViaHost?: (host: NetworkHost, via: { share: string; label: string }) => void
        /** Enter on the "Add server…" row. */
        onConnectToServer?: () => void
    }

    const { paneId, isFocused = false, onHostSelect, onServerSelect, onShareViaHost, onConnectToServer }: Props =
        $props()

    /** `listSavedServers()`, refreshed whenever the volume list is. */
    let savedServers = $state<SavedServer[]>([])

    const hosts = $derived(getNetworkHosts())
    const volumes = $derived(getVolumes())
    const isSearching = $derived(getDiscoveryState() === 'searching')
    const discoveryEnabled = $derived(getNetworkEnabled())
    const rows = $derived(buildHubRows({ saved: savedServers, hosts, volumes, listedAs: getListedAccount }))

    /**
     * F8, the row menus, and the SMB host menu's answers. Live getters, ❌ never
     * snapshots: the rows change under a menu that is still open.
     */
    const actions = createHubActions({
        getRows: () => rows,
        getVolumes: () => volumes,
        refreshSaved: refreshSavedServers,
        openRow: (row) => {
            openRow(row)
        },
    })

    let rowMenu: HubRowMenuAPI | undefined = $state()

    /** ⌃⏎ (`file.contextMenu`): the cursor row's menu, just under the row as a file row's opens. None on "Add server…". */
    // noinspection JSUnusedGlobalSymbols -- used dynamically by NetworkMountView
    export async function openContextMenuAtCursor(): Promise<void> {
        const row = rowUnderCursor()
        const anchor = rowAnchorIn(listContainer ?? null, `[data-hub-row="${String(cursorIndex)}"]`)
        if (row) await rowMenu?.open(row, anchor ?? { x: 0, y: 0 }, anchor)
    }

    let cursorIndex = $state(0)
    let listContainer: HTMLDivElement | undefined = $state()
    let containerHeight = $state(0)
    let unlistenContextAction: (() => void) | undefined

    /**
     * Re-reads the saved list whenever the volume list changes.
     *
     * ❗ Subscribe, don't poll: pin, forget, connect, and disconnect all emit
     * `volumes-changed`, and the volume store's array is reassigned on each one.
     * Reading it here is the whole subscription.
     */
    $effect(() => {
        dependOn(volumes)
        void refreshSavedServers()
    })

    onMount(() => {
        // Lazy-start mDNS the first time the user enters the hub. No-op when
        // discovery is already running or the setting is off.
        triggerNetworkDiscovery()
        refreshAllStaleShares()

        void onNetworkHostContextAction((payload) => {
            void actions.runHostAction(payload)
        }).then((fn) => {
            unlistenContextAction = fn
        })
    })

    onDestroy(() => {
        unlistenContextAction?.()
    })

    // Re-sync MCP state when the rows or the cursor change.
    $effect(() => {
        dependOn(rows, cursorIndex)
        void syncPaneStateToMcp()
    })

    /** The rows the cursor last sat over: a rebuild keeps it on the same row, clamped if it left. */
    let rowsBefore: HubRow[] = []
    $effect.pre(() => {
        const after = rows
        untrack(() => {
            cursorIndex = cursorAcrossRebuild(rowsBefore, after, cursorIndex)
            rowsBefore = after
        })
    })

    /** Retried on every `volumes-changed`, so a store that stays broken logs once until a read works. */
    const savedServersReadFailures = new LogOnceGate()

    async function refreshSavedServers(): Promise<void> {
        try {
            // `Array.isArray` because this is an IPC boundary: a command that
            // answered with nothing would otherwise put `undefined` where the
            // merge iterates, and the hub would render nothing at all.
            const answer: unknown = await listSavedServers()
            savedServers = Array.isArray(answer) ? (answer as SavedServer[]) : []
            savedServersReadFailures.clear()
        } catch (e) {
            // A store that didn't answer costs the hub its saved rows, never the
            // nearby ones: the list is still useful, and the next `volumes-changed`
            // tries again.
            const error = String(e)
            if (savedServersReadFailures.shouldLog(error)) {
                log.warn('Reading the saved servers broke down: {error}', { error })
            }
        }
    }

    /** Servers, for the status bar: a share row is a place under one, not a server. */
    const serverCount = $derived(rows.filter((row) => row.kind === 'server').length)

    /** Every row plus the "Add server…" row. */
    const totalNavigableItems = $derived(rows.length + 1)

    /** Whether the cursor sits on the "Add server…" row. */
    const isCursorOnAddRow = $derived(cursorIndex === rows.length)

    /** The row under the cursor, or `null` on the add row. */
    function rowUnderCursor(): HubRow | null {
        if (isCursorOnAddRow || cursorIndex < 0 || cursorIndex >= rows.length) return null
        return rows[cursorIndex]
    }

    /**
     * Mirrors the hub into `cmdr://state`.
     *
     * The columns a person reads are encoded into each entry's `name`, because
     * MCP's `PaneFileEntry` has only `name` / `path` / `isDirectory`.
     */
    async function syncPaneStateToMcp() {
        if (!paneId) return
        try {
            const state = hubPaneState(rows, cursorIndex, tString('fileExplorer.navigation.networkVolume'), {
                appRootOf: (row) => row.saved?.places[0]?.appRoot ?? null,
                shareCountOf: (row) => (row.host ? getShareCount(row.host.id) : undefined),
            })
            await (paneId === 'left' ? updateLeftPaneState(state) : updateRightPaneState(state))
        } catch {
            // MCP mirroring is optional; a failed push must not touch the UI.
        }
    }

    function scrollToIndex(index: number) {
        if (!listContainer) return
        const targetTop = index * ROW_HEIGHT
        const targetBottom = targetTop + ROW_HEIGHT
        const scrollTop = listContainer.scrollTop
        const viewportBottom = scrollTop + containerHeight

        if (targetTop < scrollTop) {
            listContainer.scrollTop = targetTop
        } else if (targetBottom > viewportBottom) {
            listContainer.scrollTop = targetBottom - containerHeight
        }
    }

    // noinspection JSUnusedGlobalSymbols -- used dynamically by MCP move_cursor
    export function setCursorIndex(index: number) {
        cursorIndex = Math.max(0, Math.min(index, totalNavigableItems - 1))
        scrollToIndex(cursorIndex)
    }

    // noinspection JSUnusedGlobalSymbols -- used dynamically by MCP move_cursor's range check
    export function getItemCount(): number {
        return totalNavigableItems
    }

    /** Refresh everything the hub shows (⌘R). */
    export function refresh() {
        handleRefreshClick()
    }

    /** The server an "Add" just saved: the list learns of it a moment later, so this waits for the row. */
    let pendingSelection = $state<string | null>(null)

    $effect(() => {
        const id = pendingSelection
        if (id === null) return
        const index = rows.findIndex((row) => row.id === id || row.host?.id === id)
        if (index < 0) return
        pendingSelection = null
        setCursorIndex(index)
    })

    /** Selects the server `id` names (saved or discovery id), now or once listed: "Add"'s proof (cmdr-reports#6). */
    // noinspection JSUnusedGlobalSymbols -- used by NetworkMountView after an Add
    export function selectServer(id: string) {
        pendingSelection = id
        void refreshSavedServers()
    }

    /** Find a row by name, returns its index or -1. */
    // noinspection JSUnusedGlobalSymbols -- used dynamically
    export function findItemIndex(name: string): number {
        return rows.findIndex((row) => row.name.toLowerCase() === name.toLowerCase())
    }

    /**
     * The SMB host under the cursor, or `null`. Consumed by "Copy path between
     * panes", which mirrors a host into the other pane.
     */
    // noinspection JSUnusedGlobalSymbols -- used dynamically by NetworkMountView
    export function getHostUnderCursor(): NetworkHost | null {
        // A share row reaches the palette as a `server` row instead: its host is
        // the row above it, and mirroring the host would drop the share.
        const row = rowUnderCursor()
        return row?.kind === 'server' ? row.host : null
    }

    /**
     * The whole row under the cursor, or `null` on the add row.
     *
     * ❗ How the palette commands ("Pin / unpin server", "Disconnect server",
     * "Forget saved password") reach what the user is looking at: the hub IS a
     * pane, so a command acting on "the focused pane's volume" would otherwise
     * act on the synthetic hub row.
     */
    // noinspection JSUnusedGlobalSymbols -- used dynamically by NetworkMountView / FilePane
    export function getRowUnderCursor(): HubRow | null {
        return rowUnderCursor()
    }

    /** Opens whatever the cursor is on — the same action Enter triggers. */
    // noinspection JSUnusedGlobalSymbols -- used dynamically by NetworkMountView / MCP
    export function openCursorItem(): void {
        if (isCursorOnAddRow) {
            onConnectToServer?.()
            return
        }
        const row = rowUnderCursor()
        if (row) openRow(row)
    }

    /** What Enter does to a row: `openMoveFor` decides, this carries it out. */
    function openRow(row: HubRow): void {
        const move = openMoveFor(row, rows, volumes)
        if (!move) { log.warn('The hub row {name} has nowhere to open', { name: row.name }); return; }
        if (move.kind === 'host') onHostSelect?.(move.host, move.label)
        else if (move.kind === 'place') onServerSelect?.(move.row)
        else onShareViaHost?.(move.host, { share: move.share, label: move.label })
    }

    /** Arrow keys and Enter. */
    function handleArrowAndEnter(key: string): boolean {
        if (key === 'Enter') {
            openCursorItem()
            return true
        }
        const next = cursorAfterArrow(key, cursorIndex, totalNavigableItems)
        if (next === null) return false
        cursorIndex = next
        scrollToIndex(cursorIndex)
        return true
    }

    /**
     * Handle keyboard navigation. This runs BEFORE the document-level dispatcher in
     * `+page.svelte`, which is registered bubble-phase on `document` and this pane is
     * a descendant. So a branch that acts on a Tier 1 combo must `stopPropagation()`,
     * or the dispatcher runs the same command again (it has no `defaultPrevented`
     * guard). Nothing reads a return value: `preventDefault` + `stopPropagation` is
     * how a branch says it claimed the key.
     */
    // noinspection JSUnusedGlobalSymbols -- used dynamically
    export function handleKeyDown(e: KeyboardEvent): void {
        // The refresh key (⌘R by default) works regardless of row count. Read through
        // the registry so a rebind follows. `stopPropagation` keeps it to ONE refresh:
        // `pane.refresh` is centrally dispatched too, and `refreshPane` routes it back
        // into this component through `refreshNetworkHosts()` → `refresh()`, which is
        // this same `handleRefreshClick()`. Without it, every host got re-read twice.
        if (eventMatchesCommand(e, 'pane.refresh')) {
            claimKey(e)
            handleRefreshClick()
            return
        }

        // Try centralized navigation shortcuts first (PageUp, PageDown, Home, End, Option+arrows)
        const visibleItems = Math.max(1, Math.floor(containerHeight / ROW_HEIGHT))
        const navResult = handleNavigationShortcut(e, {
            currentIndex: cursorIndex,
            totalCount: totalNavigableItems,
            visibleItems,
        })
        if (navResult?.handled) {
            e.preventDefault()
            cursorIndex = navResult.newIndex
            scrollToIndex(cursorIndex)
            return
        }

        // Everything below is an unmodified key. Matching the whole combo (rather
        // than just `e.key`) keeps ⇧F8 (delete permanently), ⌘↑/⌘↓ (parent/open), and
        // ⌘←/⌘→ (copy path between panes) reaching the document dispatcher instead of
        // also moving this cursor.
        if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return

        // F8: forget the saved server under the cursor. Claimed: it's `file.delete` to the dispatcher.
        if (e.key === 'F8') {
            const row = rowUnderCursor()
            if (row) {
                claimKey(e)
                void actions.forget(row)
            }
            return
        }

        // `stopPropagation` for the same reason ⌘R has it: Enter resolves to `nav.open`
        // centrally, whose handler posts Enter straight back to the focused pane, which
        // hands the network view every key. Without it, one Enter opened the row twice.
        if (handleArrowAndEnter(e.key)) {
            claimKey(e)
        }
    }

    function handleRowClick(index: number) {
        cursorIndex = index
    }

    function handleRowDoubleClick(index: number) {
        if (index < 0 || index >= rows.length) return
        openRow(rows[index])
    }

    function handleAddRowClick() {
        cursorIndex = rows.length
    }

    /** " as testuser" / " as guest", space first: a Svelte block trims the whitespace it opens with. */
    const accountSuffix = (row: HubRow) => (row.account === null ? '' : ` ${signedInAsLabel(row.account)}`)

    /** The protocol name, from the same map the volume switcher's slot reads. */
    function typeLabel(row: HubRow): string {
        return protocolLabel(row.protocol) ?? row.protocol.toUpperCase()
    }

    /** Re-read the saved list and re-fetch every host's shares (user-initiated). */
    function handleRefreshClick() {
        void refreshSavedServers()
        for (const host of hosts) {
            clearShareState(host.id)
            if (host.hostname) {
                fetchShares(host).catch(() => {
                    // Errors are stored in shareStates, ignore here
                })
            }
        }
    }

    /** Opens Settings at the switch that turns local network discovery back on. */
    function openDiscoverySetting() {
        void openSettingsWindow(
            'servers-hub',
            ['File systems', 'SMB/Network shares'],
            settingAnchorId('network.enabled'),
        )
    }
</script>

<div class="servers-hub" class:is-focused={isFocused}>
    <div class="header-row">
        <span class="col-name">{tString('servers.hub.colName')}</span>
        <span class="col-type">{tString('servers.hub.colType')}</span>
        <span class="col-address">{tString('servers.hub.colAddress')}</span>
        <span class="col-status">{tString('servers.hub.colStatus')}</span>
        <span class="col-last-used">{tString('servers.hub.colLastUsed')}</span>
    </div>
    <div class="row-list" bind:this={listContainer} bind:clientHeight={containerHeight}>
        {#each rows as row, index (row.id)}
            <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
            <div
                class="server-row"
                data-hub-row={index}
                class:is-under-cursor={index === cursorIndex}
                class:is-focused-and-under-cursor={isFocused && index === cursorIndex}
                role="listitem"
                onclick={() => {
                    handleRowClick(index)
                }}
                ondblclick={() => {
                    handleRowDoubleClick(index)
                }}
                oncontextmenu={(e: MouseEvent) => {
                    e.preventDefault()
                    void rowMenu?.open(row, { x: e.clientX, y: e.clientY }, null)
                }}
                onkeydown={() => {}}
            >
                <span class="col-name" class:is-share={row.kind === 'share'}>
                    <span class="row-icon"><Icon name={hubRowIcon(row)} size={16} aria-hidden="true" /></span>
                    <!-- The tooltip sits on the span that clips: an overflow check on the cell never fires. -->
                    <span class="name-text" use:tooltip={{ text: row.name + accountSuffix(row), overflowOnly: true }}
                        >{row.name}{#if row.account !== null}<span class="share-account">{accountSuffix(row)}</span
                            >{/if}</span
                    >
                </span>
                <span class="col-type">{typeLabel(row)}</span>
                <span class="col-address" use:tooltip={{ text: row.address, overflowOnly: true }}>{row.address}</span>
                <span
                    class="col-status"
                    class:needs-you={row.status === 'signed_out' || row.status === 'waiting_for_key'}
                    class:is-live={row.status === 'connected'}
                    use:tooltip={row.kind === 'server' && row.host ? getStatusTooltip(row.host) : ''}
                >
                    {tString(STATUS_TEXT_KEY[row.status])}
                </span>
                <span class="col-last-used">
                    {#if row.kind === 'share'}
                        <!-- The server row above says when it was last used. -->
                    {:else if lastUsedSeconds(row) === null}
                        <span class="never-used">{tString('servers.hub.neverUsed')}</span>
                    {:else}
                        <DateLabel modifiedAt={lastUsedSeconds(row)} />
                    {/if}
                </span>
            </div>
        {/each}

        {#if isSearching}
            <div class="searching-indicator">
                <Spinner size="sm" />
                {tString('fileExplorer.network.browser.searching')}
            </div>
        {/if}

        {#if !discoveryEnabled}
            <!--
                Discovery off: the saved servers above are unaffected (SFTP and WebDAV
                never needed the macOS Local Network permission), so this line replaces
                the nearby hosts and nothing else.
            -->
            <div class="discovery-off">
                <span>{tString('servers.hub.discoveryOff')}</span>
                <LinkButton onclick={openDiscoverySetting}
                    >{tString('servers.hub.discoveryOffLink')}</LinkButton
                >
            </div>
        {/if}

        <!-- "Add server…" pseudo-row, always at the bottom, keyboard navigable -->
        <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
        <div
            class="server-row add-row"
            class:is-under-cursor={isCursorOnAddRow}
            class:is-focused-and-under-cursor={isFocused && isCursorOnAddRow}
            role="listitem"
            onclick={handleAddRowClick}
            ondblclick={() => onConnectToServer?.()}
            onkeydown={() => {}}
        >
            <span class="col-name add-label">
                <span class="add-icon">+</span>
                <span>{tString('servers.hub.addServer')}</span>
            </span>
        </div>

        {#if !isSearching && rows.length === 0}
            <div class="empty-state">
                <img class="empty-icon" src="/icons/network-no-hosts.svg" alt="" />
                <div class="empty-title">{tString('servers.hub.emptyTitle')}</div>
                <div class="empty-message">{tString('servers.hub.emptyMessage')}</div>
                <Button variant="secondary" onclick={handleRefreshClick}
                    >{tString('fileExplorer.network.browser.refresh')}</Button
                >
            </div>
        {/if}
    </div>

    {#if rows.length > 0}
        <ServersHubStatusBar {serverCount} onRefresh={handleRefreshClick} />
    {/if}
</div>

<ServersHubRowMenu bind:this={rowMenu} {actions} />

<style>
    /* ONE grid for the header and every row (each a `subgrid`), so a column is as wide as
       its widest cell: Type, Address, Status, and Last used fit their content, and Name
       takes the rest. The name track keeps a floor, since the content-sized ones are
       maximized first and would otherwise squeeze it to nothing in a narrow pane.
       Address stops at 20%: a WebDAV URL is long, and it clips before a name does.
       Last used never clips (a cut-off date misreads), so it has no overflow of its own. */
    .servers-hub {
        display: grid;
        grid-template-columns: minmax(min(12em, 40%), 1fr) auto fit-content(20%) auto auto;
        grid-template-rows: auto minmax(0, 1fr) auto;
        column-gap: var(--spacing-lg);
        height: 100%;
        font-size: var(--font-size-sm);
        font-family: var(--font-system), sans-serif;
    }

    .header-row,
    .row-list,
    .server-row {
        grid-column: 1 / -1;
        display: grid;
        grid-template-columns: subgrid;
    }

    /* Anything in the list that isn't a row (the add row's label, the searching and
       discovery lines, the empty state) spans every column. */
    .row-list > :not(.server-row),
    .add-row .col-name {
        grid-column: 1 / -1;
    }

    .header-row {
        padding: var(--spacing-xs) var(--spacing-sm);
        background-color: var(--color-bg-secondary);
        border-bottom: 1px solid var(--color-border-strong);
        font-weight: 500;
        color: var(--color-text-secondary);
        white-space: nowrap;
    }

    .row-list {
        align-content: start;
        overflow-y: auto;
    }

    .server-row {
        height: 20px;
        padding: var(--spacing-xxs) var(--spacing-sm);
        cursor: default;
    }

    .server-row.is-under-cursor {
        background-color: var(--color-cursor-inactive);
    }

    .server-row.is-focused-and-under-cursor {
        background-color: var(--color-cursor-active);
    }

    .col-name {
        display: flex;
        align-items: center;
        gap: var(--spacing-sm);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .col-type {
        color: var(--color-text-secondary);
        white-space: nowrap;
    }

    .col-address {
        color: var(--color-text-secondary);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .col-status {
        display: flex;
        align-items: center;
        gap: var(--spacing-xxs);
        color: var(--color-text-tertiary);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .col-status.is-live {
        color: var(--color-text-secondary);
    }

    .col-status.needs-you {
        color: var(--color-warning);
    }

    .col-last-used {
        color: var(--color-text-tertiary);
        white-space: nowrap;
    }

    .never-used {
        color: var(--color-text-tertiary);
    }

    /* A share sits one icon plus the gap in. ❗ On the ICON: padding on the flex cell grew
       its base size and pushed Type, Address, and Status right on every share row. */
    .col-name.is-share .row-icon {
        margin-left: var(--spacing-xl);
    }

    /* The name and its account clip as one, with an ellipsis, short of the Type column. */
    .name-text {
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        padding-right: var(--spacing-sm);
    }

    .share-account {
        color: var(--color-text-tertiary);
    }

    .row-icon {
        display: inline-flex;
        align-items: center;
        color: var(--color-text-secondary);
    }

    .add-row .add-label {
        color: var(--color-text-tertiary);
        font-style: italic;
    }

    .add-icon {
        font-style: normal;
        font-weight: 600;
        font-size: var(--font-size-md);
        color: var(--color-text-tertiary);
    }

    .searching-indicator {
        display: flex;
        align-items: center;
        gap: var(--spacing-sm);
        padding: var(--spacing-md) var(--spacing-lg);
        color: var(--color-text-tertiary);
        font-style: italic;
    }

    .discovery-off {
        display: flex;
        align-items: center;
        gap: var(--spacing-sm);
        padding: var(--spacing-sm) var(--spacing-lg);
        color: var(--color-text-tertiary);
    }

    .empty-state {
        display: flex;
        flex-direction: column;
        align-items: center;
        justify-content: center;
        height: 100%;
        padding: var(--spacing-xl);
        gap: var(--spacing-md);
        color: var(--color-text-secondary);
    }

    .empty-icon {
        width: 96px;
        height: 96px;
    }

    .empty-title {
        font-size: var(--font-size-lg);
        font-weight: 500;
        color: var(--color-text-primary);
    }

    .empty-message {
        font-size: var(--font-size-sm);
        color: var(--color-text-tertiary);
        text-align: center;
    }
</style>
