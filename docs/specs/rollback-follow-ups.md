# Rollback follow-ups

The in-flight rollback recheck shipped: every reversal a person can trigger verifies each item right before acting and
reports what it left (`apps/desktop/src-tauri/src/file_system/write_operations/transfer/DETAILS.md` § "What the
in-flight ledgers record" and § "What a reversal does with that identity"), and a history-dialog rollback has Pause and
Cancel (`apps/desktop/src/lib/operation-log/RollbackControls.svelte`, `apps/desktop/src/lib/operation-log/DETAILS.md`).
What's left are three small history-dialog gaps David deferred while that work ran.

## 1. The history dialog keeps a finished rollback badged "Rolling back" until it's reopened

- **Problem**: the operation-log dialog reads operation headers once, on open, and nothing refreshes them while it stays
  open. A reversal that ends while the dialog is open leaves its row badged "Rolling back" until the dialog is closed
  and reopened (documented as the alpha's accepted limit in `apps/desktop/src/lib/operation-log/DETAILS.md` § "Caching
  and staleness"). The row's Pause and Cancel buttons do go away on their own, since they follow the live session.
- **Impact**: low. A stale status in a dialog people open rarely; the next open is correct.
- **Solution**: when the live session the row's controls already follow reports the reversal settled, re-read that row's
  header (or, more broadly, subscribe the dialog to header changes). No polling.
- **Size**: S–M (half a day to a day). Tradeoff: a live feed adds a subscription to a rarely open dialog.

## 2. The history dialog doesn't mark a rollback as an undo of another operation

- **Problem**: the operation log stores `rolls_back_op_id` on every reversal
  (`apps/desktop/src-tauri/src/operation_log/DETAILS.md`), and it reaches the frontend as `rollsBackOpId` in
  `bindings.ts`, but no dialog code reads it. A rollback shows up in the history as an ordinary operation with nothing
  tying it to the one it reversed.
- **Impact**: low. Someone reading their history can't tell which rows are undos of which.
- **Solution**: render reversal rows with an "Undo of …" line (or a link that scrolls to the original row), and the
  original row with a pointer to its latest reversal. Copy goes through the catalog.
- **Size**: S (half a day plus copy review). Clear win.

## 3. An operation and its rollback report item counts that differ by one

- **Problem**: observed during the 2026-08-31 rollback work: an operation's item count and its reversal's item count in
  the history dialog differ by one for the same set of files. Not re-verified since; the likeliest suspects are the
  top-level directory counted on one side only, or `item_count` being the PLANNED total on one row and a walked total on
  the other (`apps/desktop/src-tauri/src/operation_log/DETAILS.md` describes `item_count` as the planned total).
- **Impact**: low. A number that doesn't add up erodes trust in the history view.
- **Solution**: reproduce first (a copy of a folder with a few files, then Roll back from the dialog), find which side
  counts the extra item, and align the two on one definition.
- **Size**: S (a few hours once reproduced).
