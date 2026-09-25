<script lang="ts">
    /**
     * The add form: the protocol, an address, a name, whatever that protocol
     * signs in with, and an Advanced disclosure for the rest.
     *
     * ❗ **Protocol first, and only the person moves it.** The toggle sits on
     * top and typing never changes it: an address that read as SFTP once
     * flipped it under someone typing `sven@192.168.0.153` for their SMB NAS,
     * and SSH got dialed without them ever clicking SFTP (cmdr-reports#8). What
     * the address looks like shows as a warning under the field
     * (`addressLooksLike`), and the person decides.
     *
     * ❗ **The name is not an advanced setting**, so it sits under the address,
     * for every protocol. Its placeholder says what an empty one falls back to.
     *
     * ❗ **SMB asks for no password here.** Its connect is a share mount, and the
     * credential question comes from the listing or the mount when one refuses.
     * Putting a password field in front of a NAS that lets guests in would ask
     * for something nobody needs. An optional username is the one thing it takes:
     * it says the person means to sign in rather than browse as guest.
     *
     * ❗ **The root folder is a CEILING, the start folder is a LANDING.** Nothing
     * navigates above the root; opening the place lands on the start folder, which
     * has to be the root or under it. Empty means the root.
     */
    import { open as openFilePicker } from '@tauri-apps/plugin-dialog'
    import Button from '$lib/ui/Button.svelte'
    import Checkbox from '$lib/ui/Checkbox.svelte'
    import InfoTip from '$lib/ui/InfoTip.svelte'
    import TextInput from '$lib/ui/TextInput.svelte'
    import ToggleGroup, { type ToggleGroupOption } from '$lib/ui/ToggleGroup.svelte'
    import { tString } from '$lib/intl/messages.svelte'
    import { getAppLogger } from '$lib/logging/logger'
    import type { ServerProtocol } from '$lib/ipc/bindings'
    import type { ServerForm } from './server-form'
    import type { MessageKey } from '$lib/intl/keys.gen'

    /**
     * What the address field takes, per selected protocol. ❗ Keyed by the TOGGLE, like
     * everything else the sheet decides: an SMB user told to paste "a whole ssh line"
     * would paste the wrong thing.
     */
    const ADDRESS_HELP_KEY: Record<ServerProtocol, MessageKey> = {
        smb: 'servers.sheet.addressHelpSmb',
        sftp: 'servers.sheet.addressHelpSftp',
        webdav: 'servers.sheet.addressHelpWebdav',
    }

    interface Props {
        form: ServerForm
        disabled: boolean
        /** ❗ Off in edit mode: changing which protocol a saved server speaks makes it a different server. */
        protocolEditable: boolean
        /**
         * ❗ Off in edit mode, and the address and username go with the protocol
         * toggle. Rust mints the volume id from `(host, port, username)` and
         * `sftp_known_servers::remember` keys on the same tuple, so an edited one
         * upserts a SECOND saved entry beside the first rather than moving
         * anything: the hub grows a duplicate row and the old id's tabs are
         * orphaned. The honest path to a new identity is Forget then Add, which
         * `identityHint` says.
         */
        identityEditable: boolean
        /** The two lines under the locked identity fields, saying what to do instead. */
        identityHint?: string
        /** The sentence under the address field, when the last attempt was refused. */
        addressRefusal?: string
        /** The sentence under the address field when it looks like another protocol than the selected one. */
        addressWarning?: string
        /** Offered on `not_a_webdav_server`: appends the Nextcloud collection path. Nobody knows that path. */
        onTryNextcloudAddress?: () => void
        /**
         * Offered once an add couldn't reach the server: saves it as typed,
         * checking nothing, which the line beside it says.
         */
        onAddAnyway?: () => void
        /** The sentence under the secret field. */
        secretRefusal?: string
        /** The sentence under the root folder. */
        rootRefusal?: string
        /** The sentence under the start folder. */
        startFolderRefusal?: string
        /** Shown in edit mode when the backend says unattended reconnect can't work as things stand. */
        storedSecretWarning?: string
        /**
         * The name field's placeholder: the sentence saying what an empty name
         * falls back to. ❌ Never a bare address, which read as a value and sent
         * a person to edit the wrong field.
         */
        namePlaceholder?: string
        /** Whether the Advanced disclosure is open. The sheet opens it to show a refusal under a field inside. */
        advancedOpen?: boolean
        /** The start folder lost focus, which is when the sheet's inline "under the root" check starts speaking. */
        onStartFolderBlur?: () => void
        onChange: (patch: Partial<ServerForm>) => void
        addressInput?: HTMLInputElement
        /** The password input, so a refusal under it can put the caret there. */
        secretInput?: HTMLInputElement
        rootInput?: HTMLInputElement
        startFolderInput?: HTMLInputElement
    }

    /* eslint-disable prefer-const -- $bindable() requires `let` destructuring */
    let {
        form,
        disabled,
        protocolEditable,
        identityEditable,
        identityHint,
        addressRefusal,
        addressWarning,
        onTryNextcloudAddress,
        onAddAnyway,
        secretRefusal,
        rootRefusal,
        startFolderRefusal,
        storedSecretWarning,
        namePlaceholder,
        advancedOpen = $bindable(false),
        onStartFolderBlur,
        onChange,
        addressInput = $bindable(),
        secretInput = $bindable(),
        rootInput = $bindable(),
        startFolderInput = $bindable(),
    }: Props = $props()

    const log = getAppLogger('servers')

    const protocolOptions: ToggleGroupOption[] = $derived([
        { value: 'smb', label: tString('servers.sheet.protocolSmb') },
        { value: 'sftp', label: tString('servers.sheet.protocolSftp') },
        { value: 'webdav', label: tString('servers.sheet.protocolWebdav') },
    ])

    /** SMB signs in from the mount, not from here, and keeps no folders here either. */
    const asksForCredentials = $derived(form.protocol !== 'smb')
    /** Which sentence sits under the address: a refusal outranks a warning, which outranks the help line. */
    const addressDescribedBy = $derived.by(() => {
        if (addressRefusal) return 'server-address-refusal'
        if (addressWarning) return 'server-address-warning'
        return identityEditable ? 'server-address-help' : undefined
    })
    const isSftp = $derived(form.protocol === 'sftp')
    /** An empty start folder opens the root, so the root is what the empty field shows. */
    const startFolderPlaceholder = $derived(form.remoteRoot.trim() === '' ? '/' : form.remoteRoot.trim())

    async function browseForKeyFile() {
        try {
            const picked = await openFilePicker({ multiple: false, directory: false })
            if (typeof picked === 'string') onChange({ keyFile: picked })
        } catch (e) {
            // The picker not opening leaves the field exactly as it was, which is
            // a path the user can still type. Nothing to put on screen.
            log.warn('The key-file picker did not open: {error}', { error: String(e) })
        }
    }
</script>

<div class="field">
    <ToggleGroup
        semantics="tabs"
        value={form.protocol}
        options={protocolOptions}
        onChange={(value: string) => {
            onChange({ protocol: value as ServerProtocol })
        }}
        disabled={disabled || !protocolEditable}
        ariaLabel={tString('servers.sheet.protocolLegend')}
        fullWidth
    />
</div>

<div class="field">
    <label for="server-address" class="field-label">{tString('servers.sheet.address')}</label>
    <TextInput
        id="server-address"
        bind:inputElement={addressInput}
        value={form.address}
        oninput={(e: Event) => {
            onChange({ address: (e.currentTarget as HTMLInputElement).value })
        }}
        disabled={disabled || !identityEditable}
        invalid={addressRefusal !== undefined}
        aria-describedby={addressDescribedBy}
        placeholder={tString('servers.sheet.addressPlaceholder')}
        autocapitalize="off"
        autocomplete="off"
        spellcheck={false}
    />
    {#if addressRefusal}
        <p id="server-address-refusal" class="field-refusal" role="alert">{addressRefusal}</p>
        {#if onTryNextcloudAddress}
            <div class="remedy-row">
                <Button size="mini" onclick={onTryNextcloudAddress} {disabled}>
                    {tString('servers.sheet.tryNextcloudAddress')}
                </Button>
            </div>
        {/if}
        {#if onAddAnyway}
            <div class="remedy-row">
                <Button size="mini" onclick={onAddAnyway} {disabled} aria-describedby="server-add-anyway-help">
                    {tString('servers.sheet.addAnyway')}
                </Button>
                <p id="server-add-anyway-help" class="field-help">{tString('servers.sheet.addAnywayHelp')}</p>
            </div>
        {/if}
    {:else if addressWarning}
        <!-- `status`, ❌ not `alert`: it arrives while someone is typing, and it
             asks for a look, not an interruption. -->
        <p id="server-address-warning" class="field-warning" role="status">{addressWarning}</p>
    {:else if identityEditable}
        <p id="server-address-help" class="field-help">{tString(ADDRESS_HELP_KEY[form.protocol])}</p>
    {/if}
    <!-- ❗ No "paste whatever you have" line under a field nobody can type in.
         The locked group's own sentence sits under the username instead, where
         it covers all three of address, protocol, and account. -->
</div>

<!-- Every protocol has a name, SMB included: it's what the Servers list shows. -->
<div class="field">
    <label for="server-name" class="field-label">{tString('servers.sheet.name')}</label>
    <TextInput
        id="server-name"
        value={form.displayName}
        oninput={(e: Event) => {
            onChange({ displayName: (e.currentTarget as HTMLInputElement).value })
        }}
        {disabled}
        placeholder={namePlaceholder}
    />
</div>

{#if form.protocol === 'smb'}
    <!-- ❗ Optional, and editable in edit mode too: for SMB the account is a
         preference (it prefills the first sign-in and keeps the share list from
         answering as guest), not the server's identity. No password here: the
         share listing or the mount asks when one is needed. -->
    <div class="field">
        <label for="server-username" class="field-label">{tString('servers.sheet.username')}</label>
        <TextInput
            id="server-username"
            value={form.username}
            oninput={(e: Event) => {
                onChange({ username: (e.currentTarget as HTMLInputElement).value })
            }}
            {disabled}
            placeholder={tString('servers.sheet.smbUsernamePlaceholder')}
            aria-describedby="server-smb-username-help"
            autocomplete="username"
            autocapitalize="off"
            spellcheck={false}
        />
        <p id="server-smb-username-help" class="field-help">{tString('servers.sheet.smbUsernameHelp')}</p>
    </div>
{/if}

{#if asksForCredentials}
    <div class="field">
        <label for="server-username" class="field-label">{tString('servers.sheet.username')}</label>
        <TextInput
            id="server-username"
            value={form.username}
            oninput={(e: Event) => {
                onChange({ username: (e.currentTarget as HTMLInputElement).value })
            }}
            disabled={disabled || !identityEditable}
            aria-describedby={identityHint ? 'server-identity-hint' : undefined}
            autocomplete="username"
            autocapitalize="off"
            spellcheck={false}
        />
        {#if identityHint}
            <p id="server-identity-hint" class="field-help">{identityHint}</p>
        {/if}
    </div>

    <div class="field">
        <label for="server-secret" class="field-label">{tString('servers.sheet.password')}</label>
        <TextInput
            id="server-secret"
            bind:inputElement={secretInput}
            type="password"
            value={form.secret}
            oninput={(e: Event) => {
                onChange({ secret: (e.currentTarget as HTMLInputElement).value })
            }}
            {disabled}
            invalid={secretRefusal !== undefined}
            aria-describedby={secretRefusal ? 'server-secret-refusal' : undefined}
            autocomplete="current-password"
        />
        {#if secretRefusal}
            <p id="server-secret-refusal" class="field-refusal" role="alert">{secretRefusal}</p>
        {/if}
    </div>

    <div class="field">
        <Checkbox
            checked={form.remember}
            onCheckedChange={(checked: boolean) => {
                onChange({ remember: checked })
            }}
            {disabled}
        >
            {tString('servers.sheet.remember')}
        </Checkbox>
    </div>

    {#if storedSecretWarning}
        <p class="stored-secret-warning" role="status">{storedSecretWarning}</p>
    {/if}
{/if}

<!-- SMB keeps nothing Advanced would hold, so it gets no disclosure that opens onto nothing. -->
{#if asksForCredentials}
    <details class="advanced" bind:open={advancedOpen}>
        <summary>{tString('servers.sheet.advanced')}</summary>
        <div class="advanced-body">
            <div class="field">
                <label for="server-remote-root" class="field-label">{tString('servers.sheet.rootFolder')}</label>
                <TextInput
                    id="server-remote-root"
                    bind:inputElement={rootInput}
                    value={form.remoteRoot}
                    oninput={(e: Event) => {
                        onChange({ remoteRoot: (e.currentTarget as HTMLInputElement).value })
                    }}
                    {disabled}
                    invalid={rootRefusal !== undefined}
                    aria-describedby={rootRefusal ? 'server-remote-root-refusal' : 'server-remote-root-help'}
                    placeholder="/"
                    autocapitalize="off"
                    spellcheck={false}
                />
                {#if rootRefusal}
                    <p id="server-remote-root-refusal" class="field-refusal" role="alert">{rootRefusal}</p>
                {:else}
                    <p id="server-remote-root-help" class="field-help">{tString('servers.sheet.rootFolderHelp')}</p>
                {/if}
            </div>

            <div class="field">
                <label for="server-start-folder" class="field-label">{tString('servers.sheet.startFolder')}</label>
                <TextInput
                    id="server-start-folder"
                    bind:inputElement={startFolderInput}
                    value={form.startFolder}
                    oninput={(e: Event) => {
                        onChange({ startFolder: (e.currentTarget as HTMLInputElement).value })
                    }}
                    onblur={() => onStartFolderBlur?.()}
                    {disabled}
                    invalid={startFolderRefusal !== undefined}
                    aria-describedby={startFolderRefusal ? 'server-start-folder-refusal' : 'server-start-folder-help'}
                    placeholder={startFolderPlaceholder}
                    autocapitalize="off"
                    spellcheck={false}
                />
                {#if startFolderRefusal}
                    <p id="server-start-folder-refusal" class="field-refusal" role="alert">{startFolderRefusal}</p>
                {:else}
                    <p id="server-start-folder-help" class="field-help">{tString('servers.sheet.startFolderHelp')}</p>
                {/if}
            </div>

            {#if isSftp}
                <div class="field">
                    <label for="server-key-file" class="field-label">{tString('servers.sheet.keyFile')}</label>
                    <div class="path-row">
                        <TextInput
                            id="server-key-file"
                            value={form.keyFile}
                            oninput={(e: Event) => {
                                onChange({ keyFile: (e.currentTarget as HTMLInputElement).value })
                            }}
                            {disabled}
                            autocapitalize="off"
                            spellcheck={false}
                        />
                        <Button onclick={() => void browseForKeyFile()} {disabled}>
                            {tString('servers.sheet.browse')}
                        </Button>
                    </div>
                </div>

                <div class="field">
                    <Checkbox
                        checked={form.useAgent}
                        onCheckedChange={(checked: boolean) => {
                            onChange({ useAgent: checked })
                        }}
                        {disabled}
                    >
                        {tString('servers.sheet.useAgent')}
                    </Checkbox>
                </div>
            {/if}

            <div class="field">
                <Checkbox
                    checked={form.autoReconnect}
                    onCheckedChange={(checked: boolean) => {
                        onChange({ autoReconnect: checked })
                    }}
                    {disabled}
                >
                    {tString('servers.sheet.autoReconnect')}
                </Checkbox>
                <!-- Beside the box, ❌ never inside its label: a click on the glyph would flip it.
                     The label alone reads as "connect at startup", which this switch doesn't do. -->
                <InfoTip
                    label={tString('servers.sheet.autoReconnectInfoLabel')}
                    text={tString('servers.sheet.autoReconnectHelp')}
                />
            </div>
        </div>
    </details>
{/if}

<style>
    .field {
        margin-bottom: var(--spacing-md);
    }

    .field-label {
        display: block;
        margin-bottom: var(--spacing-xs);
        font-size: var(--font-size-sm);
        font-weight: 500;
        color: var(--color-text-secondary);
    }

    .field-help {
        margin: var(--spacing-xs) 0 0;
        font-size: var(--font-size-sm);
        color: var(--color-text-tertiary);
    }

    .field-refusal {
        margin: var(--spacing-xs) 0 0;
        font-size: var(--font-size-sm);
        color: var(--color-error-text);
    }

    .field-warning {
        margin: var(--spacing-xs) 0 0;
        font-size: var(--font-size-sm);
        color: var(--color-warning-text);
    }

    .remedy-row {
        margin-top: var(--spacing-sm);
    }

    .stored-secret-warning {
        margin: 0 0 var(--spacing-md);
        padding: var(--spacing-sm) var(--spacing-md);
        background-color: color-mix(in srgb, var(--color-warning) 15%, transparent);
        border: 1px solid var(--color-warning);
        border-radius: var(--radius-md);
        font-size: var(--font-size-sm);
        color: var(--color-text-secondary);
    }

    /* No `cursor: pointer`: Cmdr sets `cursor: default` globally for native feel,
       and links are the only sanctioned exception (`ui/LinkButton.svelte`). */
    .advanced summary {
        user-select: none;
        color: var(--color-text-secondary);
    }

    .advanced-body {
        margin-top: var(--spacing-md);
    }

    .path-row {
        display: flex;
        align-items: center;
        gap: var(--spacing-sm);
    }

    /* `TextInput`'s own frame is `.text-field`; the Browse button sits beside it
       and the field takes the rest of the row. */
    .path-row :global(.text-field) {
        flex: 1 1 auto;
        min-width: 0;
    }
</style>
