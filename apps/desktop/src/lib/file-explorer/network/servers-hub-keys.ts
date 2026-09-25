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
