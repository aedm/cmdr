# Listing index sizes

Routes the drive index's folder-size updates (`IndexEvent::DirsUpdated`) to the open listings they touch, and tells the
frontend with `listing-index-sizes-changed`. A pane on `~/Downloads` never hears about a write in `~/Library`.

## Module map

- `mod.rs`: the open-listing set (a `ListingLifecycle` observer), the worker task, and the event.
- `touched.rs`: the pure rule for what one batch touched in one listing (nothing, some rows, or all of them).

## Must-knows

- **An ANCESTOR of a listing in a batch means nothing for it.** Every live batch carries its whole ancestor chain up to
  `/`, so reading `/` as "refresh everything" refreshed every pane on every write anywhere. Only `["/"]` ALONE (or the
  volume id) is the whole-volume signal. `touched.rs` holds the full rule.
- **A path strictly under the listing touches the child row on the way down**, not just direct children: a write deep
  in `~/Library` really moves the `Library` row's recursive size.
- **Keyed on the listing's own record** (`CachedListing::path`, firmlink-normalized), read at open time, so MTP's
  `mtp://` spelling and `/tmp` → `/private/tmp` match the index's paths.
- **The batch arrives on the index writer's thread**: `dirs_updated` only hands it to the worker. ❌ Don't do work there.

Flow, the batch shapes it reads, and decisions: `DETAILS.md`.
