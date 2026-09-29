<script lang="ts">
    /**
     * The progress dialog's Rollback button, in its three readings: rolling
     * back, blocked (with the reason), and live (with what it promises). The
     * decision behind it, and the confirmation it raises, live in
     * `transfer-rollback.svelte.ts`.
     */
    import Button from '$lib/ui/Button.svelte'
    import { tString } from '$lib/intl/messages.svelte'
    import type { TransferRollback } from './transfer-rollback.svelte'

    interface Props {
        rollback: TransferRollback
        isRollingBack: boolean
        /** Nothing to explain: the scan (nothing written yet), a cancel under
         *  way, or the settle window (the operation is already over). */
        disabled: boolean
    }

    const { rollback, isRollingBack, disabled }: Props = $props()
</script>

{#if isRollingBack}
    <Button variant="danger" disabled>{tString('fileOperations.transferProgress.titleRollingBack')}</Button>
{:else}
    <!-- The two BLOCKED readings are `aria-disabled` rather than `disabled`:
         each has a reason worth reading, and a `disabled` button leaves the tab
         order, taking its tooltip with it. The press is guarded instead, so a
         blocked click asks nothing. `disabled` stays right for the scan and the
         settle window, and disabled rather than hidden, so the button row doesn't
         reshuffle when counting ends. -->
    <Button
        variant="danger"
        ariaDisabled={rollback.blockedTooltip !== null}
        tooltipContent={rollback.blockedTooltip ?? rollback.liveTooltip}
        onclick={rollback.request}
        {disabled}>{tString('fileOperations.transferProgress.conflictRollback')}</Button
    >
{/if}
