<script lang="ts">
    /**
     * "Cmdr is fetching this drive's index, hang on."
     *
     * The dialog can open before the volume it will search has its arena in memory, and
     * until now that wait looked exactly like an idle dialog. This strip fills it, and
     * only it: `QueryResults` speaks for the wait once a run has been attempted, and
     * `CoverageNote.svelte` speaks for a drive that has no index at all, after a run has
     * proved it. The three never overlap.
     *
     * Presentational: `SearchDialog.svelte` owns the decision, `index-load-hint.svelte.ts`
     * owns the clock behind it.
     *
     * The wrapper stays mounted with `role="status"` even with nothing to say, collapsing
     * to zero height. A live region has to exist BEFORE its content changes or screen
     * readers miss the update, same as `CoverageNote.svelte`.
     */
    import { tString } from '$lib/intl/messages.svelte'

    interface Props {
        /** Whether there's a wait worth mentioning. The clock lives in `index-load-hint.svelte.ts`. */
        visible: boolean
    }

    const { visible }: Props = $props()
</script>

<div class="index-load-hint" class:is-empty={!visible} role="status">
    {#if visible}
        <!-- The same sentence `QueryResults` uses for the same wait, deliberately: one
             fact, one string, and the two surfaces can't drift apart or contradict each
             other as the user moves between them. -->
        <p class="message">{tString('queryUi.results.loadingIndex')}</p>
    {/if}
</div>

<style>
    .index-load-hint {
        padding: var(--spacing-xs) var(--spacing-dialog);
        border-bottom: 1px solid var(--color-border-subtle);
        background: var(--color-bg-secondary);
        flex-shrink: 0;
    }

    /* Nothing to say: collapse completely rather than leaving a bordered empty strip.
       Still mounted, so the live region survives to announce the next wait. */
    .index-load-hint.is-empty {
        padding: 0;
        border-bottom: none;
    }

    .message {
        margin: 0;
        color: var(--color-text-secondary);
        font-size: var(--font-size-sm);
    }
</style>
