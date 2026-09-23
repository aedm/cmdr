# Listing index sizes: details

## Flow

1. The drive index commits a batch and emits `IndexEvent::DirsUpdated { paths }` through its event sink.
2. `events/index_mapping.rs` routes it here (`dirs_updated`) instead of to the frontend, and reports
   `Destination::ListingIndexSizes`.
3. The worker matches the batch against every open listing (`touched`) and merges what it touched into that listing's
   pending set.
4. A listing refreshes when its pending set is non-empty, its last refresh is at least `COOLDOWN` (2 s) ago, and the main
   window is visible. A listing inside its cooldown gets a deadline, so the last change always lands (leading plus
   trailing).
5. The refresh (on the blocking pool) reads the touched rows' `dir_stats` plus the listing's own, keeps the rows whose
   `RowSizes` moved, writes them into `LISTING_CACHE`, and emits `listing-index-sizes-changed` with their readings and,
   when it moved, the listing's own reading (the `..` row). Nothing moved: nothing is sent.
6. The frontend (`src/lib/file-explorer/pane/index-events.ts` → `FilePane.applyIndexSizes`) writes the readings onto
   its cached rows and the cursor entry in place, re-reads only the status-bar totals, and pushes the MCP pane state.

A whole-volume batch (`Touched::Whole`) skips the per-row comparison: it runs the full `refresh_listing_index_sizes`
re-enrich and sends `full: true`, and the pane re-reads its window's sizes the old way. It happens once per scan.

The open-listing set is kept by a `ListingLifecycle` observer (`crate::listing_lifecycle`), registered from setup via
`start`. It reads the listing's volume id and path off the `CachedListing` record, which the open path inserts before it
notifies.

## The batch shapes

- **A live batch**: each changed directory plus its whole ancestor chain up to `/`
  (`cmdr-index` `paths::path_prefix::with_ancestor_closure`). About once a second on a busy disk.
- **A network or phone watcher's batch**: only the changed directory's parent, no chain (`transports/smb/watch.rs`,
  `transports/mtp/watch.rs`). `touched` finds the child row on the way down all the same.
- **Whole-volume moments**: `["/"]` after a full scan completes or a replay overflows, `[volume_id]` after a network
  scan completes, `[volume_root]` after a phased first index completes. The last one is handled by the frontend's
  `index-aggregation-complete` refresh, which fires in the same breath.

## Decisions

**Decision**: the backend decides which listings and rows an update touched, and whether anything shown moved. **Why**:
the frontend used to get every batch and match paths itself. Its `/` short-circuit fired on every live batch (they all
end in `/`), so both panes refreshed on every write anywhere on the disk, each refresh six IPC calls whether or not a
number moved: about 110 refreshes per three idle minutes on a pane on `~`, which made the WebContent process idle at
~1.4% of a core instead of ~0.2% (measured 2026-09-23, dev build, issue #92). The backend already knows every open
listing, and it has to read the stats anyway, so it's also where "did anything change" is cheapest to answer.

**Decision**: compare raw readings, not formatted text. **Why**: the size text depends on settings and locale the
webview owns (units, separators), and the tooltip shows exact counts. A raw change that formats the same still reaches
the pane, where Svelte leaves the DOM alone because the text is equal, and the Full list holds the Size column's width
(`views/measure-column-widths.ts::holdSizeColumnWidth`).

**Decision**: hold everything while the main window is hidden and catch up on show. **Why**: nobody reads a hidden
pane, and WebKit queues the DOM work anyway (the diagnosis saw transitions piling up in a hidden page). The catch-up is
one refresh per listing with every batch merged. The cost: the listing cache's sizes (and so MCP's) are stale while the
window is hidden, by at most what changed in that time.
