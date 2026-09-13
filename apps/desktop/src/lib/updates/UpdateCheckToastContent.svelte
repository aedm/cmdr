<script lang="ts">
    import { updateState } from './update-state.svelte'
    import { describeUpdateFailure, formatUpdateStatus, updateFailureOffersReport } from './update-status-text'
    import Button from '$lib/ui/Button.svelte'
    import { openErrorReportDialog } from '$lib/error-reporter/error-report-flow.svelte'
    import { tString } from '$lib/intl/messages.svelte'

    const statusText = $derived(formatUpdateStatus(updateState))
    const failureText = $derived(updateState.failure === null ? null : describeUpdateFailure(updateState.failure))
    const offersReport = $derived(updateState.failure !== null && updateFailureOffersReport(updateState.failure))

    function handleSendErrorReport() {
        // The note is the sentence the toast showed; the raw detail is already in the log the report bundles.
        openErrorReportDialog(failureText ?? '')
    }
</script>

<div class="content">
    {#if failureText !== null}
        <span class="message">{failureText}</span>
        {#if offersReport}
            <div class="actions">
                <Button size="mini" variant="secondary" onclick={handleSendErrorReport}
                    >{tString('updates.checkToast.sendErrorReport')}</Button
                >
            </div>
        {/if}
    {:else}
        <span class="message">{statusText}</span>
    {/if}
</div>

<style>
    .content {
        font-size: var(--font-size-sm);
        color: var(--color-text-primary);
    }

    .actions {
        display: flex;
        justify-content: flex-end;
        gap: var(--spacing-sm);
        margin-top: var(--spacing-lg);
    }
</style>
