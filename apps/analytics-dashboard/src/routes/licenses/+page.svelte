<!--
  Licenses page (`/licenses`): every license this server has issued, bought or handed out, one row
  each, newest claim first. The listing comes from the api-server's `/admin/licenses` via
  `+page.server.ts`, so the admin token stays server-side. The layout hides the range/day picker here:
  this is a ledger, not a time series.

  The two states that mean someone is waiting (`undelivered`, `unfinished`) and the two code
  mismatches lead the page in an attention panel, because those are what the page exists to catch.

  The note is editable, through a dialog that opens holding what's already there. It's the running
  record of one license: why it exists, what happened since. Facts about the PERSON belong in the
  vault note instead, or the two drift into half-complete copies of each other.
-->
<script lang="ts">
    import type { PageProps } from './$types'
    import { enhance } from '$app/forms'
    import SectionDescription from '$lib/components/SectionDescription.svelte'
    import { formatNumber, formatUtcDateTime } from '$lib/format.js'
    import {
        expiryPlaceholder,
        filterBySource,
        licenseTypeLabel,
        maxNoteLength,
        sourceLabel,
        stateExplanation,
        stateLabel,
        stateTone,
        summarizeLicenses,
        type LicenseRow,
        type SourceFilter,
        type StateTone,
    } from '$lib/licenses.js'

    const { data, form }: PageProps = $props()

    let noteDialog = $state<HTMLDialogElement | null>(null)
    /** The row being edited, or null when the dialog is closed. Also titles the dialog. */
    let editing = $state<LicenseRow | null>(null)
    let noteDraft = $state('')

    function startEditNote(row: LicenseRow) {
        editing = row
        noteDraft = row.note ?? ''
        noteDialog?.showModal()
    }

    function closeNoteDialog() {
        noteDialog?.close()
    }

    /** Who the note is about, for the dialog title. Falls back to the code, then the transaction. */
    function noteSubject(row: LicenseRow): string {
        if (row.customerEmail) return row.customerEmail
        return row.shortCodes.length > 0 ? row.shortCodes[0] : row.transactionId
    }

    const summary = $derived(summarizeLicenses(data.listing))

    /** Which source the table shows. Hand-issued licenses are the group David hands out and tracks. */
    let filter = $state<SourceFilter>('all')

    const filterButtons = $derived([
        { value: 'all' as const, label: 'All', count: summary.total },
        { value: 'paddle' as const, label: 'Purchased', count: summary.purchased },
        { value: 'manual' as const, label: 'Hand-issued', count: summary.handIssued },
    ])

    const rows = $derived(filterBySource(data.listing.licenses, filter))

    /** What to call the filtered-out-everything case ("No hand-issued licenses yet"). */
    const filterLabel = $derived(filter === 'all' ? '' : sourceLabel(filter).toLowerCase())

    /** Badge colors per state tone. One class string per tone, so the table markup stays readable. */
    const toneClasses: Record<StateTone, string> = {
        ok: 'border-success/40 bg-success/10 text-success',
        warning: 'border-warning/40 bg-warning/10 text-warning',
        alarm: 'border-danger/50 bg-danger/15 text-danger',
        muted: 'border-border bg-surface-elevated text-text-tertiary',
    }

    function plural(count: number, one: string, many: string): string {
        return `${formatNumber(count)} ${count === 1 ? one : many}`
    }
</script>

<section class="mt-2 rounded-xl border border-border bg-surface p-6">
    <h2 class="text-lg font-semibold text-text-primary">Licenses</h2>
    <SectionDescription
        insight="Every license this server has issued, bought or handed out, newest first. It's the only place that can answer which codes exist and who holds them: Paddle knows about money, not about codes."
        caveat="Issued means our own records are complete, so the codes exist and, for a purchase, the email went out. Whether a Paddle subscription is still running only Paddle can say, so check there before reading Issued as still paying. All times UTC."
    />

    {#if data.loadError}
        <p class="rounded-lg border border-danger/40 bg-danger/10 px-3 py-2 text-sm text-danger">
            Couldn't load the licenses: {data.loadError}
        </p>
    {:else}
        <!-- Attention panel: the states and code mismatches that mean someone should act. -->
        {#if summary.needsAttention}
            <div class="mb-6 rounded-lg border border-danger/50 bg-danger/10 p-4">
                <h3 class="mb-2 text-sm font-semibold text-danger">Needs attention</h3>
                <ul class="list-disc space-y-1 pl-4 text-xs/relaxed text-text-secondary">
                    {#if summary.undelivered > 0}
                        <li>
                            <span class="text-text-primary"
                                >{plural(summary.undelivered, 'purchase', 'purchases')} with codes that were never
                                emailed.</span
                            >
                            Someone paid and is waiting.
                        </li>
                    {/if}
                    {#if summary.unfinished > 0}
                        <li>
                            <span class="text-text-primary"
                                >{plural(summary.unfinished, 'license', 'licenses')} claimed but never minted, so there's
                                no code at all.</span
                            >
                            Either a delivery died, or one is in flight this second.
                        </li>
                    {/if}
                    {#if data.listing.orphanCodes.length > 0}
                        <li>
                            <span class="text-text-primary"
                                >{plural(data.listing.orphanCodes.length, 'code', 'codes')} in the key store that no row
                                explains:</span
                            >
                            {#each data.listing.orphanCodes as code (code)}
                                <code class="mr-1 text-text-secondary">{code}</code>
                            {/each}
                            Someone may be holding one, and nothing here says whether it works.
                        </li>
                    {/if}
                    {#if data.listing.missingCodes.length > 0}
                        <li>
                            <span class="text-text-primary"
                                >{plural(data.listing.missingCodes.length, 'code', 'codes')} on a row but gone from the
                                key store, so they can't be activated:</span
                            >
                            {#each data.listing.missingCodes as code (code)}
                                <code class="mr-1 text-text-secondary">{code}</code>
                            {/each}
                        </li>
                    {/if}
                </ul>
            </div>
        {:else if summary.total > 0}
            <p
                class="mb-6 rounded-lg border border-border-subtle bg-surface-elevated/50 px-3 py-2 text-xs text-text-secondary"
            >
                Nothing needs attention: every license has its codes, and the ledger and the key store agree.
            </p>
        {/if}

        {#if summary.total === 0}
            <div class="rounded-lg border border-border-subtle bg-surface-elevated px-4 py-6 text-center">
                <p class="text-sm text-text-secondary">No licenses yet</p>
                <p class="mt-1 text-xs text-text-tertiary">
                    A purchase or a hand-issued license shows up here the moment it's minted.
                </p>
            </div>
        {:else}
            <!-- Counts and the source filter. Both are noise on an empty ledger, so they wait for a row. -->
            <div class="mb-4 flex flex-wrap items-center justify-between gap-3">
                <p class="text-sm text-text-secondary">
                    {plural(summary.total, 'license', 'licenses')}: {formatNumber(summary.purchased)} purchased,
                    {formatNumber(summary.handIssued)} hand-issued.
                </p>
                <div class="flex rounded-lg border border-border bg-surface p-0.5">
                    {#each filterButtons as button (button.value)}
                        <button
                            type="button"
                            aria-pressed={filter === button.value}
                            onclick={() => { filter = button.value; }}
                            class="rounded-md px-3 py-1 text-sm font-medium transition-colors
                                {filter === button.value
                                ? 'bg-accent text-accent-contrast'
                                : 'text-text-secondary hover:text-text-primary'}"
                        >
                            {button.label} ({formatNumber(button.count)})
                        </button>
                    {/each}
                </div>
            </div>

            {#if rows.length === 0}
                <p class="text-sm text-text-secondary">No {filterLabel} licenses yet.</p>
            {:else}
                <div class="overflow-x-auto">
                    <table class="w-full text-left text-sm">
                        <thead>
                            <tr class="border-b border-border-subtle text-text-tertiary">
                                <th class="pr-4 pb-2 font-medium">State</th>
                                <th class="pr-4 pb-2 font-medium">Source</th>
                                <th class="pr-4 pb-2 font-medium">Codes</th>
                                <th class="pr-4 pb-2 font-medium">Type</th>
                                <th class="pr-4 pb-2 font-medium">Customer</th>
                                <th class="pr-4 pb-2 font-medium">Claimed</th>
                                <th class="pr-4 pb-2 font-medium">Issued</th>
                                <th class="pr-4 pb-2 font-medium">Emailed</th>
                                <th class="pr-4 pb-2 font-medium">Expires</th>
                                <th class="pr-4 pb-2 font-medium">Note</th>
                                <th class="pb-2 font-medium">Transaction</th>
                            </tr>
                        </thead>
                        <tbody>
                            {#each rows as row (row.transactionId)}
                                <tr class="border-b border-border-subtle/50 align-top">
                                    <td class="py-2 pr-4">
                                        <span
                                            title={stateExplanation(row.state)}
                                            class="inline-block rounded-md border px-2 py-0.5 text-xs font-medium whitespace-nowrap
                                                {toneClasses[stateTone(row.state)]}"
                                        >
                                            {stateLabel(row.state)}
                                        </span>
                                    </td>
                                    <td class="py-2 pr-4">
                                        <span
                                            class="inline-block rounded-md border px-2 py-0.5 text-xs font-medium whitespace-nowrap
                                                {row.source === 'manual'
                                                ? 'border-accent/50 bg-accent/10 text-accent'
                                                : 'border-border bg-surface-elevated text-text-secondary'}"
                                        >
                                            {sourceLabel(row.source)}
                                        </span>
                                    </td>
                                    <td class="py-2 pr-4">
                                        {#if row.shortCodes.length > 0}
                                            {#each row.shortCodes as code (code)}
                                                <code class="block whitespace-nowrap text-text-primary">{code}</code>
                                            {/each}
                                        {:else}
                                            <span class="text-text-tertiary">–</span>
                                        {/if}
                                        {#if row.quantity !== null && row.quantity > 1}
                                            <span class="text-xs text-text-tertiary">
                                                {plural(row.quantity, 'seat', 'seats')}
                                            </span>
                                        {/if}
                                    </td>
                                    <td class="py-2 pr-4 text-text-secondary">{licenseTypeLabel(row.licenseType)}</td>
                                    <td class="py-2 pr-4">
                                        <span class="text-text-secondary">{row.customerEmail ?? '–'}</span>
                                        {#if row.organizationName}
                                            <span class="block text-xs text-text-tertiary">{row.organizationName}</span>
                                        {/if}
                                    </td>
                                    <td class="py-2 pr-4 whitespace-nowrap text-text-tertiary">
                                        {formatUtcDateTime(row.claimedAt)}
                                    </td>
                                    <td class="py-2 pr-4 whitespace-nowrap text-text-tertiary">
                                        {row.issuedAt ? formatUtcDateTime(row.issuedAt) : '–'}
                                    </td>
                                    <td class="py-2 pr-4 whitespace-nowrap text-text-tertiary">
                                        {row.emailedAt ? formatUtcDateTime(row.emailedAt) : '–'}
                                    </td>
                                    <td class="py-2 pr-4 whitespace-nowrap text-text-tertiary">
                                        {row.expiresAt
                                            ? formatUtcDateTime(row.expiresAt)
                                            : expiryPlaceholder(row.source)}
                                    </td>
                                    <td class="max-w-64 py-2 pr-4 text-xs text-text-tertiary">
                                        <span class="whitespace-pre-line">{row.note ?? '–'}</span>
                                        <button
                                            type="button"
                                            onclick={() => { startEditNote(row); }}
                                            class="ml-1 cursor-pointer text-accent underline decoration-dotted
                                                underline-offset-2 transition-colors hover:text-accent-hover"
                                        >
                                            Edit
                                        </button>
                                    </td>
                                    <td class="py-2 text-xs break-all text-text-tertiary">{row.transactionId}</td>
                                </tr>
                            {/each}
                        </tbody>
                    </table>
                </div>
            {/if}
        {/if}
    {/if}
</section>

<!--
  Note editor. A native <dialog> so Esc and the focus trap come from the platform. It opens holding
  the current note, so editing is an append rather than an overwrite, and the whole text is what
  gets saved.
-->
<dialog
    bind:this={noteDialog}
    onclose={() => { editing = null; }}
    class="m-auto w-[min(40rem,calc(100vw-2rem))] rounded-xl border border-border bg-surface p-0
        text-text-primary backdrop:bg-black/60"
>
    {#if editing}
        <form
            method="POST"
            action="?/saveNote"
            use:enhance={() =>
                ({ update }) =>
                    update({ reset: false }).then(() => {
                        if (form && 'saved' in form && form.saved) closeNoteDialog()
                    })}
            class="flex flex-col gap-3 p-5"
        >
            <div>
                <h3 class="text-sm font-semibold text-text-primary">Note on {noteSubject(editing)}</h3>
                <p class="mt-1 text-xs text-text-tertiary">
                    Why this license exists and what's happened since. Facts about the person belong in the vault
                    note, so the two don't become half-complete copies of each other.
                </p>
            </div>

            <input type="hidden" name="transactionId" value={editing.transactionId} />
            <!--
              No autofocus attribute: `showModal()` focuses the first focusable descendant, and the
              hidden input above isn't one, so the textarea gets the caret without asking for it.
            -->
            <textarea
                name="note"
                bind:value={noteDraft}
                rows="8"
                maxlength={maxNoteLength}
                placeholder="First purchase ever!!"
                class="w-full resize-y rounded-md border border-border bg-surface-elevated px-3 py-2 text-sm
                    text-text-primary placeholder:text-text-tertiary focus:border-accent focus:outline-none"
            ></textarea>

            <div class="flex items-center justify-between gap-3">
                <p class="text-xs text-text-tertiary">
                    {formatNumber(noteDraft.length)} / {formatNumber(maxNoteLength)}
                    {#if editing.source === 'manual'}
                        <span class="block">A hand-issued license has to keep a note, so this one can't be emptied.</span>
                    {/if}
                </p>
                <div class="flex gap-2">
                    <button
                        type="button"
                        onclick={closeNoteDialog}
                        class="cursor-pointer rounded-md border border-border px-3 py-1.5 text-sm text-text-secondary
                            transition-colors hover:text-text-primary"
                    >
                        Cancel
                    </button>
                    <button
                        type="submit"
                        class="cursor-pointer rounded-md bg-accent px-4 py-1.5 text-sm font-medium text-accent-contrast
                            transition-colors hover:bg-accent-hover"
                    >
                        Save note
                    </button>
                </div>
            </div>

            {#if form && 'error' in form && form.error}
                <p class="text-sm text-danger">{form.error}</p>
            {/if}
        </form>
    {/if}
</dialog>
