/**
 * Where an arrow key takes a network list's cursor, over `total` rows: up and down
 * one row, left and right to the first and last, like the file list's Brief view.
 * `null` for any other key. Shared by the servers hub (its "Add server…" row
 * counted in `total`) and a host's places, so the two lists move alike.
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
