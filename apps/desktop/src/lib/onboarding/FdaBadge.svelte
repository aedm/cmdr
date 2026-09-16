<script lang="ts">
    /**
     * Title-bar badge saying this Mac hasn't given Cmdr Full Disk Access. Click reopens
     * onboarding at step 1, where the grant is explained and System Settings is one button away.
     *
     * It exists because the state was previously invisible: someone could dismiss the onboarding
     * step on every launch for days, never answer it, and then meet an unexplained permission
     * refusal deep inside a copy or a delete with nothing connecting the two.
     *
     * ❗ Hidden while the wizard is showing step 1. That page IS this badge's destination, so
     * pointing at it from behind the sheet is noise.
     */
    import Icon from '$lib/ui/Icon.svelte'
    import { t, tString } from '$lib/intl/messages.svelte'
    import { tooltip } from '$lib/tooltip/tooltip'
    import { fdaIsMissing } from './fda-status.svelte'

    interface Props {
        /** True while the onboarding wizard is on step 1, its own FDA page. */
        onboardingOnFdaStep: boolean
        /** Opens onboarding at step 1. */
        onOpenOnboarding: () => void
    }

    const { onboardingOnFdaStep, onOpenOnboarding }: Props = $props()

    const visible = $derived(fdaIsMissing() && !onboardingOnFdaStep)
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
        position: absolute;
        right: var(--spacing-sm);
        top: 50%;
        transform: translateY(-50%);
        display: flex;
        align-items: center;
        gap: var(--spacing-xxs);
        /* stylelint-disable-next-line declaration-property-value-disallowed-list -- a 2px
           vertical inset has no spacing token; `--spacing-xxs` is the same 2px but reads as
           a gap, not a pad. */
        padding: 2px var(--spacing-xs);
        border: 1px solid var(--color-warning);
        border-radius: var(--radius-full);
        /* The SOLID tint, not `--color-warning-bg`: the title bar paints a mode tint over
           itself in dev/E2E, and a translucent badge would let it bleed through. */
        background-color: var(--color-warning-bg-solid);
        color: var(--color-warning-text);
        /* Capped like `.title-text`: the compounded text scale must not outgrow the
           fixed-height title bar. */
        font-size: min(var(--font-size-xs), 15px);
        font-weight: 500;
        /* Above the dev/E2E/capture mode tint, which paints an `::after` over the whole bar. */
        z-index: 1;
    }

    .fda-badge:hover {
        background-color: color-mix(in srgb, var(--color-warning) 22%, var(--color-bg-secondary));
    }
</style>
