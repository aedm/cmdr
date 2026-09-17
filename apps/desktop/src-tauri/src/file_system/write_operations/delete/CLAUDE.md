# Delete + trash

A local-FS walker (`walkdir` + `fs::remove_file`), a volume-aware walker (MTP, SMB, over the `Volume` trait,
oracle-aware), and OS-native trash.

`../CLAUDE.md` holds the shared `WriteOperationState`, `OperationIntent`, cancel, ETA, and settle contracts;
`../transfer/CLAUDE.md` is the copy + move parallel. Frontend counterpart:
`apps/desktop/src/lib/file-operations/delete/CLAUDE.md`.

## Files

- **`walker.rs`**: local and volume delete, both taking `&dyn OperationEventSink`; `delete_files_start` routes by
  `volume_id`. The volume walker asks `try_get_authoritative_listing` before every `list_directory`, so a subtree open
  in another pane is cache-fed. DETAILS § "Volume-delete internals".
- **`trash.rs`**: `move_to_trash_sync()` (macOS `trashItemAtURL`; Linux `trash` crate; reused by
  `commands/rename.rs`), `trash_files_with_progress()` (batch, per-item progress, cancel), and
  `trash_dir_for_path()` (❌ keep its ancestor walk, DETAILS § Where a trash is). Refusals are a typed `MutationError`,
  ❌ never a sentence; every item it can't take emits its own `Failed` source-item event. Existence checks use
  `symlink_metadata()`.
- **`cloud_trash.rs`**: `routing_for_selection()` behind the `trash_routing_for_paths` command, answering whether F8
  runs as a permanent delete. DETAILS § "A trash in a cloud-storage folder becomes a delete".
- **`volume_start.rs`**: a volume delete's managed lifecycle (here, not `../mod.rs`, because its body is `async`).
  DETAILS § "The volume delete's own lifecycle".
- Test siblings: the `*_tests.rs` / `*_test.rs` files beside the code.

## Must-knows

- **Delete order is files first, then directories deepest-first**: the walker collects in DFS order and deletes in
  reverse, so `remove_dir` always finds an empty one.
- **Delete is not rollbackable.** Cancel stops further deletes; it can't restore what's gone.
- **MTP/non-local volumes can't use `walkdir` or `fs::remove_*`**, hence the parallel path. Both emit identical events.
- **Both delete paths reuse the scan-preview cache via `config.preview_id`**: on a hit the `ScanResult` is consumed
  directly and an initial `phase: Deleting` event fires, so the FE keeps the right denominator. The volume path is
  oracle-aware without a preview too.
- **A `preview_id` alone doesn't authorize acting on a path set.** The LOCAL walker iterates `scan_result.files` and
  never re-reads its `sources`, so an unbound cache deletes the PREVIEWED tree, with no rollback. Bind with
  `take_cached_scan_result`; ❌ never skip it.
- **❌ Never resolve a top-level source's type with `.unwrap_or(false)`.** Hand the `Option` to
  `scan_volume_recursive`, which propagates a failed probe: a guessed "file" books zero bytes for a whole tree.
- **Trash has no scan phase**: `trashItemAtURL` is atomic per top-level item, so progress tracks items (bytes from
  pre-computed sizes). A PARTLY refused batch still COMPLETES, and its event carries `refused` (count +
  `strongest_refusal`): ❌ never `None`, or the ending reads as a clean success.
- **A trash inside `~/Library/CloudStorage/<domain>/` runs as a permanent delete**, but ONLY under a provider we know
  by name: the dialog promises the service kept a copy, and `MacDroid-<device>` there is an Android phone. ❌ Never
  widen it to "any location with no trash" either: a fresh USB stick answers the same and would lose data.
  All-or-nothing across the selection; ❌ not a volume question. DETAILS.
- **A refusal carries a typed `TrashRefusalKind`, read from the `NSError` DOMAIN + CODE**, ❌ never its localized words
  (`error-string-match` forbids it). A failed batch is `WriteOperationError::TrashRefused`, ❌ not an `IoError` (one
  flattened sentence leaves the dialog only "try again"), reporting `strongest_refusal`, ❌ not the most common one.
- **Delete and trash don't `fsync`, and ❌ never a global `sync(2)`**: a non-durable delete is annoyance-class, and that
  `sync` stalled every app on the box without making "complete" durable. Pinned by
  `tests.rs::no_global_sync_or_spawn_async_sync_in_write_operations`.
- **A recursive scan that bails with `Err(Cancelled)` must NOT emit `write-cancelled`; its top-level caller does**, via
  `emit_cancelled_if_aborted`. `scan_volume_recursive` checks cancel per level, so emitting at the bail site fires the
  terminal event once per stacked frame. Pinned by `delete_cancel_during_scan_emits_write_cancelled`.

Depth: `DETAILS.md`.
