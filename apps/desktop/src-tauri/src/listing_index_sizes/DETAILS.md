# Listing index sizes: details

## Flow

1. The drive index commits a batch and emits `IndexEvent::DirsUpdated { paths }` through its event sink.
2. `events/index_mapping.rs` routes it here (`dirs_updated`) instead of to the frontend, and reports
   `Destination::ListingIndexSizes`.
3. The worker task matches the batch against every open listing (`touched`), and emits `listing-index-sizes-changed`
   for each listing it touched.
4. The frontend (`src/lib/file-explorer/pane/index-events.ts`) finds the pane showing that listing and refreshes its
   folder sizes.

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

**Decision**: the backend decides which listings a batch touches. **Why**: the frontend used to get every batch and match
paths itself, and its `/` short-circuit fired on every live batch (they all end in `/`), so both panes refreshed on
every write anywhere on the disk: about 110 refreshes, six IPC calls each, every three idle minutes on a pane on `~`
(measured 2026-09-23). The backend already knows every open listing (`ListingLifecycle`), which is the smart-backend
shape anyway.
