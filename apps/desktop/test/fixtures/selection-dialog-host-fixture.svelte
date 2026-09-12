<!--
  Test-only host for `SelectionDialog`, wired the way `routes/(main)/+page.svelte`
  wires it: the dialog renders under `{#if showSelectionDialog && selectionDialogSnapshot}`,
  reads its props off the snapshot, and closing nulls BOTH in the same tick.

  `SelectionDialog.svelte.test.ts` mounts this so a close from inside the dialog
  runs through the production components with the page's teardown order. The
  lived bug: the click that closed the dialog kept bubbling to `ModalDialog`'s
  scrim handler, which read `onclose` off a `config` that re-derived from the
  now-null snapshot.
-->
<script lang="ts">
    import SelectionDialog from '$lib/selection-dialog/SelectionDialog.svelte'
    import type { FileEntry } from '$lib/file-explorer/types'

    interface Props {
        entries: FileEntry[]
        onCommit: (indices: number[], mode: 'add' | 'remove') => void
    }

    const { entries, onCommit }: Props = $props()

    let showSelectionDialog = $state<'add' | 'remove' | null>('add')
    let selectionDialogSnapshot = $state.raw<{
        entries: FileEntry[]
        cursorIndex: number
        isSnapshotPane: boolean
    } | null>({ entries, cursorIndex: 0, isSnapshotPane: false })

    function handleSelectionDialogClose() {
        showSelectionDialog = null
        selectionDialogSnapshot = null
    }
</script>

{#if showSelectionDialog && selectionDialogSnapshot}
    <SelectionDialog
        mode={showSelectionDialog}
        entries={selectionDialogSnapshot.entries}
        cursorIndex={selectionDialogSnapshot.cursorIndex}
        isSnapshotPane={selectionDialogSnapshot.isSnapshotPane}
        {onCommit}
        onClose={handleSelectionDialogClose}
    />
{/if}
