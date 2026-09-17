<script lang="ts">
    /**
     * The control that takes a volume away: Eject on a disk, Disconnect on a phone or a
     * server place. One button for all three placements (the breadcrumb chip, a switcher
     * row's eject, a server row's disconnect), so the states can't drift apart.
     *
     * The WORDS, the glyph, the three states, and what pressing it runs are all decided in
     * `detach-control.ts`; this renders the look half of that answer.
     */
    import Icon from '$lib/ui/Icon.svelte'
    import Spinner from '$lib/ui/Spinner.svelte'
    import { tooltip } from '$lib/tooltip/tooltip'
    import type { DetachButtonLook } from './detach-control'

    interface Props extends DetachButtonLook {
        /** Chip placement: a small left margin so it sits beside the badges, not against them. */
        breadcrumb?: boolean
        onclick: (event: MouseEvent) => void
    }

    const { label, icon, disabled, ejecting, breadcrumb = false, onclick }: Props = $props()
</script>

<button
    type="button"
    class="eject-button"
    class:breadcrumb-eject-button={breadcrumb}
    class:is-ejecting={ejecting}
    aria-label={label}
    {disabled}
    use:tooltip={label}
    {onclick}
>
    {#if ejecting}
        <Spinner size="sm" />
    {:else}
        <Icon name={icon} size={14} aria-hidden="true" />
    {/if}
</button>

<style>
    .eject-button {
        display: inline-flex;
        align-items: center;
        justify-content: center;
        background: none;
        border: none;
        padding: var(--spacing-xxs);
        margin: 0;
        cursor: default;
        color: var(--color-text-secondary);
        border-radius: var(--radius-sm);
        flex-shrink: 0;
        font: inherit;
        line-height: var(--font-line-height-flat);
        transition:
            background-color var(--transition-base),
            color var(--transition-base);
    }

    .eject-button:hover:not(:disabled) {
        background-color: var(--color-bg-tertiary);
        color: var(--color-text-primary);
    }

    .eject-button:focus-visible {
        outline: 2px solid var(--color-accent);
        outline-offset: 1px;
    }

    /* Busy: a write op is reading from / writing to this volume, so ejecting is
       blocked. Greyed out, no hover affordance; the tooltip explains why. */
    .eject-button:disabled {
        opacity: 0.4;
        cursor: default;
    }

    /* Ejecting: the eject is underway, not unavailable, so the spinner keeps full
       strength. The 12px spinner plus its margin fills the 14px glyph's box, so the
       row doesn't shift when it swaps in. */
    /*noinspection CssUnusedSymbol*/
    .eject-button.is-ejecting:disabled {
        opacity: 1;
    }

    /*noinspection CssUnusedSymbol*/
    .eject-button.is-ejecting :global(.spinner) {
        margin: 1px;
    }

    /* Closed-state (chip) placement: a small left margin so it sits next to the
       SMB / USB badges instead of jamming against them. */
    /*noinspection CssUnusedSymbol*/
    .breadcrumb-eject-button {
        margin-left: var(--spacing-xs);
    }
</style>
