<script lang="ts">
    /**
     * One rendered ROW of the file, gutter and all.
     *
     * A row ends at a newline or at a segment boundary (`file_viewer::rows`), so a long
     * line occupies several rows. Two things follow, and both are FACTS the backend
     * states rather than anything this component infers:
     *
     * - The gutter prints the physical line number only on the row that STARTS a line
     *   (`lineNumber !== null`), the usual editor convention, and nothing on the
     *   continuation rows below it.
     * - A row whose `continues` is true carries the continuation marker: this break is
     *   Cmdr's, not one the file contains. It is drawn identically with word wrap on and
     *   off, so the mark always means exactly one thing.
     */
    import { tooltip } from '$lib/tooltip/tooltip'
    import { tString } from '$lib/intl/messages.svelte'
    import type { LineSegment } from './line-segments'

    interface Props {
        /** 0-based ROW index. The pointer layer reads it back off `data-row`. */
        rowNumber: number
        /** The 0-based physical line this row starts, or `null` on a continuation row. */
        lineNumber: number | null
        /** Cmdr ended this row at a segment boundary rather than at a newline. */
        continues: boolean
        /** The row's text, already split into search-highlight and selection spans. */
        segments: LineSegment[]
        /** Gutter width in `ch`, sized off the file's row total by the caller. */
        gutterWidth: number
        /** Word wrap is on, so the row's text wraps instead of overflowing horizontally. */
        wordWrap: boolean
    }

    const { rowNumber, lineNumber, continues, segments, gutterWidth, wordWrap }: Props = $props()
</script>

<div class="line" class:word-wrap={wordWrap} data-row={rowNumber}>
    <span class="line-number" style="width: {gutterWidth}ch" aria-hidden="true"
        >{lineNumber === null ? '' : lineNumber + 1}</span
    >
    <span class="line-text"
        >{#each segments as seg, segIdx (segIdx)}{#if seg.highlight}<mark
                    class:active={seg.active}
                    class:selected={seg.selected}>{seg.text}</mark
                >{:else if seg.selected}<span class="selected">{seg.text}</span>{:else}{seg.text}{/if}{/each}</span
    >{#if continues}<!--
        The glyph itself is a `::after` on `.row-continues`, so it is not in the DOM
        text: it can't be selected and can't reach the clipboard, where it would be a
        character the file doesn't contain. That costs it its accessible name, which the
        visually-hidden span beside it gives back.
    --><span class="row-continues" aria-hidden="true" use:tooltip={tString('viewer.row.continuesTooltip')}
        ></span
        ><span class="sr-only">{tString('viewer.row.continuesLabel')}</span>{/if}
</div>

<style>
    .line {
        display: flex;
        padding: 0 var(--spacing-sm);
        /* Stays in sync with `getLineHeight()` in `viewer-line-heights.svelte.ts`
         * via the `--font-scale` root variable. */
        height: calc(18px * var(--font-scale));
    }

    .line:hover {
        background: var(--color-bg-tertiary);
    }

    .line.word-wrap {
        height: auto;
    }

    .line-number {
        display: inline-block;
        text-align: right;
        color: var(--color-text-tertiary);
        padding-right: var(--spacing-sm);
        margin-right: var(--spacing-sm);
        border-right: 1px solid var(--color-border-subtle);
        flex-shrink: 0;
        user-select: none;
        -webkit-user-select: none;
    }

    .line-text {
        white-space: pre;
    }

    .word-wrap .line-text {
        white-space: pre-wrap;
        overflow-wrap: break-word;
        /* `.line-text` is a flex item; its default `min-width: auto` (= min-content)
         * would let an unbreakable run (no break opportunities, e.g. a long base64
         * blob or `WWWW…`) grow to full width and overflow instead of wrapping,
         * making `overflow-wrap: break-word` a no-op. `min-width: 0` lets the item
         * shrink so break-word actually breaks the run to fit. The height-map probe
         * in `viewer-line-heights.svelte.ts` mirrors this; keep them in sync. */
        min-width: 0;
    }

    /* Selected text: gold foreground matches the file-list "selected = gold" language
     * (see design-system.md § File list). Background uses the accent-subtle token, the
     * same tint the cursor highlight uses. Both work in light and dark. */
    .line-text .selected {
        background: var(--color-accent-subtle);
        color: var(--color-selection-fg);
    }

    /* Search hit + selection on the same span: keep the highlight background (so search
     * remains the dominant signal) and apply the selection foreground colour. */
    .line-text mark.selected {
        color: var(--color-selection-fg);
    }

    mark {
        background: var(--color-highlight);
        border-radius: var(--radius-xs);
        padding: 0 1px;
        margin: 0 -1px;
    }

    mark.active {
        background: var(--color-highlight-active);
    }

    /* The continuation marker: this row ended because Cmdr broke a long line, not because
       the file has a break here. It hangs off the end of the row's text in both wrap
       modes, so the mark always means exactly one thing.

       `content` on a `::after` keeps the glyph out of the DOM text, so it is neither
       selectable nor copyable; the `.sr-only` span beside it carries the label. Colours
       are tokens, never literals: `--color-text-primary` on `--color-bg-tertiary` clears
       AA in both themes with room to spare. */
    .row-continues {
        display: inline-block;
        margin-left: var(--spacing-xs);
        padding: 0 var(--spacing-xs);
        border-radius: var(--radius-xs);
        background: var(--color-bg-tertiary);
        color: var(--color-text-primary);
        vertical-align: baseline;
        user-select: none;
        -webkit-user-select: none;
    }

    .row-continues::after {
        content: '⋯';
    }
</style>
