# Delete and trash (frontend)

Delete files permanently or move them to macOS Trash, with a confirmation dialog, scan preview, and progress
(`TransferProgressDialog`). Backend counterpart:
`apps/desktop/src-tauri/src/file_system/write_operations/delete/CLAUDE.md`.

## Files

- **DeleteDialog.svelte**: confirmation dialog with file list (max 10 + overflow), live scan stats, symlink notice, a
  no-trash warning, and a "Move to trash" switch in the footer row (`ModalDialog`'s `footerLeading`) that flips the
  operation in-dialog, hidden wherever permanent is forced. Its `ModalDialog` role follows the mode: `dialog` for trash,
  `alertdialog` for permanent.
- **delete-dialog-utils.ts** (+ test): pure utilities `generateDeleteTitle()`, `abbreviatePath()`, `getSymlinkNotice()`,
  `countSymlinks()`.
- **TrashCompleteToastContent.svelte** + **trash-undo.ts** (journal rollback, worded) + **go-to-trash.ts** (the toast
  button and the `file.goToTrash` command).

## Must-knows

- **F8/Shift+F8 only set the INITIAL mode; the user flips it in-dialog.**
  `DualPaneExplorer.openDeleteDialog({ permanent })` builds props from selection or cursor, reading `supportsTrash` off
  the source `VolumeInfo`.
- **Holding Shift over an F8 dialog upgrades it to permanent until release; Shift NEVER demotes**, and a Shift+F8 dialog
  ignores the hold. Keep `blur` clearing the hold, or a window switch strands the dialog on "Delete permanently", and ❌
  keep the `keydown`/`keyup` listeners in the CAPTURE phase: `ModalDialog`'s overlay stops keydown, so a bubble-phase
  listener never sees the hold. DETAILS § Shift-hold upgrade.
- **`data-scan-state` on `.scan-stats`** (`counting` | `done`) is the only "counting done" signal, mirroring
  `TransferDialog`'s marker, which E2E polls.
- **`DeleteDialog` must forward `sourceVolumeId` into `startScanPreview`**, or a non-local volume (MTP, SMB) runs the
  local-FS walker, hits path-not-found, and leaves the dialog stuck at "0 files".
- **`supportsTrash` drives the mode.** Each volume exposes it from `fsType` (statfs): APFS/HFS+ yes; FAT32, exFAT,
  smbfs, nfs, afpfs, webdav no. When false, the dialog forces permanent mode with a warning banner.
- **A selection entirely inside a cloud-storage folder opens the PERMANENT delete, with its own banner**
  (`cloudStorageWithoutTrash`): the provider implements no trash, so `openDeleteDialog` asks `trashRoutingForPaths`
  first and drops `supportsTrash`. ❌ Don't re-derive that rule here, it's Rust's (`delete/cloud_trash.rs`); a thrown or
  timed-out answer keeps the trash. DETAILS § Cloud storage.
- **Confirm AWAITS the `startScanPreview` IPC**, so `onConfirm` never dispatches a null `previewId`: that leaves an
  ownerless concurrent walk nothing can cancel. `TransferDialog` awaits `scanStarted` likewise. DETAILS § Scan-preview
  detail.
- **A permanent delete waits for the WALK in the BACKEND** (`scan_bridge::await_claimed_preview`), consuming the cached
  result. **Trash is the one operation that doesn't wait**: `trashItemAtURL` is atomic per top-level item, so
  `trash_files_start` frees the preview outright, and the FE renders no bar from the scan's expected totals. DETAILS §
  Scan-preview detail.
- **A trash is undoable, a delete never is.** Its toast carries Undo and "Go to trash", both needing the journaled op id
  (no id → plain sentence). ❌ Never add a permanent delete there: it shows after EVERY trash, one misclick from the one
  op no rollback reverses. No `confirmBeforeDelete` setting: the dialog always shows.
- **The trash is PER VOLUME** (`get_trash_dir`), and revealing a trashed dotfile with hidden files off THROWS in
  `moveCursor`; keep that guarded. DETAILS § Undo and go-to-trash.
- **`TransferProgressDialog` is shared** (`operationType: 'delete' | 'trash'`); transfer-only props (`destinationPath`,
  `direction`, `conflictResolution`) are optional and hidden, and it stays visible ≥400 ms to avoid flashes.
- **After delete, the cursor keeps its row**, falling back to the same position index (clamped) when that row went away
  (`pane/listing-diff-sync.svelte.ts`). Selection is cleared; both panes refresh.
- **Existence checks use `symlink_metadata()`, not `path.exists()`**, so a dangling symlink stays a valid item.

## Backend touchpoints

`write_operations/delete/trash.rs` (`move_to_trash_sync`, `trash_files_with_progress`),
`write_operations/delete/walker.rs` (`delete_files_with_progress`), and `delete/cloud_trash.rs` (the routing).
`WriteOperationType::Trash` is its own event-payload variant. The MCP `delete` tool opens this dialog
(`delete-confirmation`).

Full details (the full F8→completion flow, partial-failure and locked-file handling): `DETAILS.md`.
