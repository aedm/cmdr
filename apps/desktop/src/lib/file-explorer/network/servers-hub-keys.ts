/**
 * Where the cursor goes when the rows are rebuilt: onto the SAME row (by id)
 * wherever it moved, the "Add server…" row stays the add row, and a row that
 * left clamps to what's there. ❗ An index alone lands on a neighbour whenever the
 * list re-sorts under it, which is what a plain Add did to its fresh selection.
 */
export function cursorAcrossRebuild(before: { id: string }[], after: { id: string }[], cursor: number): number {
  if (cursor >= before.length) return after.length
  const at = after.findIndex((row) => row.id === before[cursor].id)
  return at >= 0 ? at : Math.min(cursor, after.length)
}

/**
 * Where an arrow key takes the servers hub's cursor, over `total` rows (the "Add
 * server…" row included): up and down one row, left and right to the first and
 * last, like the file list's Brief view. `null` for any other key.
 *
 * Pure and beside the component so `ServersHub.svelte` stays the table and its
 * wiring.
 */
export function cursorAfterArrow(key: string, cursor: number, total: number): number | null {
  switch (key) {
    case 'ArrowDown':
      return Math.min(cursor + 1, total - 1)
    case 'ArrowUp':
      return Math.max(cursor - 1, 0)
    case 'ArrowLeft':
      return 0
    case 'ArrowRight':
      return total - 1
    default:
      return null
  }
}
