/**
 * Where the cursor goes when the rows are rebuilt: onto the SAME row (by id)
 * wherever it moved, the "Add server…" row stays the add row, and a row that
 * left clamps to what's there. ❗ An index alone lands on a neighbour whenever the
 * list re-sorts under it, which is what a plain Add did to its fresh selection.
 */
export function cursorAcrossRebuild(before: { id: string }[], after: { id: string }[], cursor: number): number {
  // Nothing listed yet: the cursor rests on the first row once there is one,
  // ❌ not on "Add server…", which is all an empty list has.
  if (before.length === 0) return Math.min(cursor, after.length)
  if (cursor >= before.length) return after.length
  const at = after.findIndex((row) => row.id === before[cursor].id)
  return at >= 0 ? at : Math.min(cursor, after.length)
}
