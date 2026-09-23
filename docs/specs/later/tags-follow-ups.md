# What Finder tags still owe

Reading, showing, and assigning macOS Finder tags all shipped. Every design decision lives beside the code:
`apps/desktop/src-tauri/src/file_system/listing/DETAILS.md` § "Finder tags" (the parse, the deferred visible-range-first
pass, carry-forward, the write path, and the diff that stays silent for unchanged rows),
`apps/desktop/src/lib/file-explorer/views/DETAILS.md` (the dot cluster), `apps/desktop/src-tauri/src/menu/DETAILS.md`
(the tag row of color circles), and `apps/desktop/src-tauri/src/file_system/DETAILS.md` (the MCP consumer and the
analytics event). Two items are open; both are judgment calls, and neither blocks anything.

## 1. The seven tag color circles show on volumes that can't hold a tag

- **Problem**: `menu/file_context_menu.rs::append_tag_color_group` appends the seven Finder tag circles to the file
  context menu for any file or folder on macOS, with no check on which backend the path lives on. Right-click a file on
  an MTP device or a directly-attached SMB share and the menu offers to tag it; the write reaches `xattr::set` on a path
  that isn't a real filesystem entry, fails, and is only logged (target `tags`).
- **Impact**: low. Nothing breaks, but the menu promises something it can't do, and the click does nothing visible.
- **Solution**: gate the group at menu-build time, for example on `supports_local_fs_access()`. Caveat: an OS-mounted
  SMB share is a real path and tagging it genuinely works, so the honest predicate is "this path reaches a filesystem
  that stores xattrs", which is only fully knowable by trying. The read side settled the same question the other way
  (`enrich_tags` in `commands/file_system/listing.rs` runs on any volume, since an empty read is harmless, behind a 2 s
  timeout). Leaving it as is stays a valid choice.
- **Size**: S (a few lines either way).
- **Blocked on**: a David decision, on his own QA pass: should the menu offer tags there at all?

## 2. A tag assigned from search results doesn't show until you navigate

- **Problem**: the search-results pane (`file-explorer/pane/SearchResultsView.svelte`) opens the file context menu
  without a listing id (it isn't a cached directory listing, so it has none). The context-menu tag toggle
  (`menu/menu_handlers.rs`, the `tag-color:` arm) writes the tags to disk and then calls `apply_tags_to_listing` with an
  empty `MenuState.context.tags_listing_id`, which finds nothing to refresh. The dots appear only on the next navigation
  into the containing directory. A normal pane passes its listing id and refreshes in place.
- **Impact**: low to medium. Someone tagging from search results sees nothing happen and may tag again, which toggles
  the tag back off.
- **Solution**: two shapes. (a) Give the search-results snapshot its own cache identity so `apply_tags_to_listing` can
  patch it, which would also serve sort-by-tag or filter-by-tag if those ever land. (b) Add a second refresh path that
  patches the results view directly from the toggle's returned per-path tag sets. (b) is smaller.
- **Size**: S for (b), M for (a).
- **Blocked on**: nothing; a David pick between (a) and (b) if he wants a say.
