<script lang="ts">
    /**
     * Status-corner badge saying this Mac hasn't given Cmdr Full Disk Access. Click reopens
     * onboarding at step 1, where the grant is explained and System Settings is one button away.
     *
     * It exists because the state was previously invisible: someone could dismiss the onboarding
     * step on every launch for days, never answer it, and then meet an unexplained permission
     * refusal deep inside a copy or a delete with nothing connecting the two.
     *
     * A plain inline box: `$lib/status-corner/StatusCorner.svelte` owns placement and order, and
     * it renders this immediately left of the indexing hourglass. ❗ Never give it a position of
     * its own: the corner is the one thing allowed to claim that strip, and a second claimant
     * lands on top of the hourglass.
     *
     * ❗ Hidden while the wizard is showing step 1. That page IS this badge's destination, so
     * pointing at it from behind the sheet is noise.
     */
    import Icon from '$lib/ui/Icon.svelte'
    import { t, tString } from '$lib/intl/messages.svelte'
    import { tooltip } from '$lib/tooltip/tooltip'
    import { isWizardOnFdaStep } from './onboarding-state.svelte'
    import { fdaIsMissing } from './fda-status.svelte'

    interface Props {
        /** Opens onboarding at step 1. */
        onOpenOnboarding: () => void
    }

    const { onOpenOnboarding }: Props = $props()

    const visible = $derived(fdaIsMissing() && !isWizardOnFdaStep())
</script>

{#if visible}
    <button
        type="button"
        class="fda-badge"
        onclick={onOpenOnboarding}
        use:tooltip={tString('onboarding.fdaBadge.tooltip')}
        aria-label={tString('onboarding.fdaBadge.ariaLabel')}
    >
        <Icon name="shield-off" size={13} />
        <span>{t('onboarding.fdaBadge.label')}</span>
    </button>
{/if}

<style>
    .fda-badge {
        display: inline-flex;
        align-items: center;
        gap: var(--spacing-xxs);
        /* The corner's other pills are 20px tall; the padding only sets the horizontal inset. */
        height: 20px;
        padding: 0 var(--spacing-xs);
        border: 1px solid var(--color-warning);
        border-radius: var(--radius-full);
        /* The SOLID tint, not `--color-warning-bg`: the title bar paints a mode tint over
           itself in dev/E2E, and a translucent badge would let it bleed through. */
        background-color: var(--color-warning-bg-solid);
        color: var(--color-warning-text);
        /* Capped like `.title-text`: the compounded text scale must not outgrow the
           fixed-height title bar the corner sits on. */
        font-size: min(var(--font-size-xs), 15px);
        font-weight: 500;
        white-space: nowrap;
    }

    .fda-badge:hover {
        background-color: color-mix(in srgb, var(--color-warning) 22%, var(--color-bg-secondary));
    }
</style>
