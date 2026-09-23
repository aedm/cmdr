# Listing index sizes

Delivers the drive index's folder-size updates (`IndexEvent::DirsUpdated`) to the open listings they touch, and only
the rows whose shown values moved, as `listing-index-sizes-changed` carrying the readings. The pane applies them with no
IPC. A pane on `~/Downloads` never hears about a write in `~/Library`; a pane on `~` hears about the `Library` row only
when its size, counts, or hourglass actually move.

## Module map

- `mod.rs`: the open-listing set (a `ListingLifecycle` observer), the worker (cooldown, hidden hold), the event.
- `touched.rs`: the pure rule for what one batch touched in one listing (nothing, some rows, or all of them).
- `refresh.rs`: `RowSizes`, the pure "does this reading change what the row shows" comparison.

## Must-knows

- **An ANCESTOR of a listing in a batch means nothing for it.** Every live batch carries its whole ancestor chain up to
  `/`, so reading `/` as "refresh everything" refreshed every pane on every write anywhere. Only `["/"]` ALONE (or the
  volume id) is the whole-volume signal. `touched.rs` holds the full rule.
- **A path strictly under the listing touches the child row on the way down**, not just direct children: a write deep
  in `~/Library` really moves the `Library` row's recursive size.
- **The hourglass isn't on the cached entry**, so the worker remembers which rows it last sent lit (`ListingState::lit`).
  ❌ Don't compare against the entry alone: a row that stops being pending would never be sent, and stay lit.
- **Moved rows go into `LISTING_CACHE` BEFORE the event** (`update_index_sizes_by_path`), so the status-bar totals the
  pane re-reads, and MCP, see them.
- **While the main window is hidden, nothing runs** (`main_window_visibility`); batches merge into one refresh per
  listing on show. ❌ Don't let a due deadline arm while hidden: the loop would spin.
- **The batch arrives on the index writer's thread**: `dirs_updated` only hands it to the worker. ❌ Don't do work there.

Flow, the batch shapes it reads, and decisions: `DETAILS.md`.
