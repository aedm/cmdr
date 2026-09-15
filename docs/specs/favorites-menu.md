# Favorites menu (⌘D)

**The quest**: GitHub #91. Opening a favorite takes a click on the volume switcher, and there's no shortcut. In Total
Commander and Double Commander, ⌃D opens the favorites list (TC calls it the "Directory hotlist"). A second commenter
asked for number keys: ⌘D shows `1 Foo`, `2 Bar/Baz`, `3 Qux`, and pressing 3 lands in Qux.

**What we build**: ⌘D opens a favorites menu for the focused pane, at the spot the volume switcher opens (top-left of
the pane). Digits go straight to a favorite, and `0` adds the current folder. The switcher loses its Favorites section
and gets one row instead, "See 12 favorites ⌘D", which keeps favorites one click away and teaches the key. Along the way
the switcher's hand-rolled dropdown becomes the house `Menu` primitive, so both menus (and every later one) share one
keyboard and pointer model.

```
Favorites
─────────
1 [icon] folder with 1000 files
2 [icon] folder with 5000 files
3 [icon] a
4 [icon] b
─────────
0 Add current folder to favorites
```

## What already exists

- **The favorites data layer is done.** `apps/desktop/src-tauri/src/favorites/` owns the ordered `favorites.json` store
  (seed-once platform defaults, UUID ids), and `commands/favorites.rs` has `add_favorite` / `remove_favorite` /
  `rename_favorite` / `reorder_favorites`, each re-emitting `volumes-changed`. Favorites reach the frontend as
  `VolumeInfo` with `category: 'favorite'` and `id: 'fav-<id>'`. ❗ **No store or IPC change is needed.** The Rust work
  is menu wiring only (§ M2, § M3).
- **The favorites interaction layer is done too**, just trapped inside the switcher:
  `navigation/favorites-controller.svelte.ts` (inline rename, pointer-drag and ⌥↑/⌥↓ reorder, remove, the local-first
  optimistic order with its reconciliation `$effect`), the pure `favorites-reorder.ts`, `favorite-tooltip.ts`, and
  `favorites-analytics.ts`. Why each piece is shaped the way it is (pointer drag over HTML5 drag, the four rename
  keystroke guards): `apps/desktop/src/lib/file-explorer/navigation/DETAILS.md` § Editable favorites.
- **Add surfaces**: `favorites.add` (palette and the Go menu, no default key, handler in
  `routes/(main)/command-handlers/misc-handlers.ts`), plus the folder-row and `..` context menus, which favorite a
  specific path Rust-side. They stay as they are.
- **Right-click on a favorite** opens a NATIVE (muda) menu through `show_volume_row_context_menu` (`commands/menu.rs`,
  `is_favorite: true` gives `Rename` / `Remove`), and the pick returns over the typed `volume-context-action` event to
  `VolumeBreadcrumb.handleVolumeContextAction`, which acts only while its own dropdown is open.
- **Four hand-rolled menus exist today**, which is the drift this effort ends:
  - `lib/ui/Menu.svelte`: flat, point-anchored, the caller routes keys. One consumer, the archive Enter popup
    (`file-explorer/pane/enter-menu.svelte.ts`), which catches keys with a document capture listener while open. ⚠️ Its
    `lib/ui/DETAILS.md` § Menu is stale: it describes an Ark-backed menu with `open` / `onOpenChange` /
    `defaultHighlightedValue` / `portal` props, and the component deliberately isn't Ark-backed and has none of them.
  - The volume switcher's dropdown, the rich one David likes: grouped sections with headings and separators, keyboard vs
    pointer mode (a 5 px move exits keyboard mode), wrap-around arrows, Home/End, a submenu with the single-cursor rule
    and fixed positioning, fit-to-viewport, scroll-into-view, reorder, inline rename. It lives in
    `VolumeBreadcrumb.svelte` (1,828 lines against a 1,845 allowlist entry) plus `volume-breadcrumb-handlers.svelte.ts`.
    Its UI rules are written up in `navigation/DETAILS.md` § Dropdown and submenu UI patterns.
  - `DriveIndexBadge.svelte`'s popover menu (which its own docs note gets clipped by the dropdown's `overflow-y: auto`).
  - `routes/viewer/ViewerContextMenu.svelte`.
- **Keys while the switcher is open**: `pane/key-dispatch.ts` routes to an open chooser BEFORE type-to-jump (so digits
  would reach a menu, not the jump buffer) and swallows everything else, and `+page.svelte::isExplorerOverlayOpen`
  suppresses central dispatch while `isVolumeChooserOpen()`. `FilePane.svelte` and `DualPaneExplorer.svelte` carry the
  pass-through exports and are both at their size cap.
- **⌘D is taken twice** (verified in the tree, 2026-09-16):
  - `file.duplicate` (`commands/sources/file-list.ts`), Finder's Duplicate, also the File menu accelerator
    (`menu/menu_bar.rs`, `macos("Cmd+D")`) and the context menu's (`menu/menu_structure.rs`).
  - `errorPane.toggleTechnicalDetails`, a `fixedKey` command whose capture-phase listener in `ErrorPane.svelte` beats
    any binding on ⌘D while an error screen shows (`pane/DETAILS.md` § ⌘D opens Technical details).
  - The debug window is ⌘⇧D (dev builds only, `routes/(main)/global-keydown.ts`); three docs said ⌘D and were fixed on
    this branch.
- **Analytics**: `favorite_opened { surface: 'breadcrumb' | 'command' }` from `favorites-analytics.ts`, fired by the
  switcher and by `pane/volume-selection.ts` (palette and MCP). Contract: `src-tauri/src/analytics/DETAILS.md`.

## Decisions taken

1. **"Favorites"** everywhere: UI, command ids, code, events. No "bookmarks" or "hotlist" except as palette keywords.
2. **⌘D** opens it, on the focused pane.
3. **An in-app menu on the house `Menu` primitive.** No native popup (DC's), no settings cockpit (TC's and DC's). The
   list itself is where favorites get managed: drag or ⌥↑/⌥↓ to reorder, right-click to rename or remove.
4. **One house `Menu`: grow `lib/ui/Menu.svelte`**, rather than adding a second menu component beside it. The name is
   already the house menu's and has one small consumer; two menu primitives is the drift we're ending.
5. **The right-click menu on a favorite stays native**, like every right-click in Cmdr, with `Rename` and
   `Remove from favorites`. Only the list is in-app.
6. **The switcher's Favorites section goes**, replaced by one selectable row at the top: "See 12 favorites" plus a live
   `ShortcutChip` for ⌘D. Enter or a click swaps the switcher for the favorites menu, in place.
7. **Order**: the primitive first, then ⌘D freed, then the feature, then the switcher migrates. The feature ships before
   the big refactor, and the switcher's favorites code gets deleted rather than migrated and then deleted.

## Proposed defaults (agent-proposed; say if any is wrong)

- **Anchor**: exactly where the switcher opens, under the pane's volume chip, fitted to the viewport. One menu at a
  time: opening either menu on either pane closes any other.
- **Digits**: `1`–`9` open the Nth favorite at once, `0` runs the add row. Favorites past nine are listed with an empty
  number column and reached by arrows or pointer. Match on `e.code` (`Digit0`–`Digit9`, `Numpad0`–`Numpad9`) with no
  ⌘/⌃/⌥ and Shift allowed, so AZERTY layouts (digits need Shift) work. It's a class-of-key matcher, which
  `cmdr/no-raw-key-match` accepts.
- **The keys that swap menus work inside menus.** Central dispatch is suppressed while a header menu is open, so the
  menu host matches with `eventMatchesCommand` (which follows a rebind): ⌘D inside the switcher swaps to favorites (the
  row just taught that key, so it must work right there), ⌘D inside favorites closes it, and the pane's chooser key (⌥F1
  / ⌥F2) inside favorites swaps back.
- **Opening a favorite** does what the switcher's favorite branch does today (`resolvePathVolume`, then a volume change
  onto the containing volume with the favorite's path). That branch is duplicated in
  `VolumeBreadcrumb.handleVolumeSelect` and `volume-selection.ts`; it becomes one `navigation/open-favorite.ts` used by
  the menu and the palette/MCP path.
- **The `0` row** is disabled, with its reason as a tooltip, when:
  - the current folder is already a favorite (see open question 3), or
  - the pane is somewhere a favorite can't point. Favorites are local paths in v1 and `favorites/DETAILS.md` says the
    add surfaces gate that, but `favorites.add` passes `getFocusedPanePath()` unchecked today. ❗ Verify first what a
    favorite on an SMB, MTP, SFTP, archive, or search-results pane does now (some may already work through
    `resolvePathVolume`), then gate the `0` row AND `favorites.add` on one capability predicate, ❌ never a path-string
    test.
- **Empty state**: the heading, a separator, the existing "(Your favorites will show here)" placeholder, a separator,
  and the `0` row.
- **Tooltips** reuse `favorite-tooltip.ts` (the path first, so a renamed favorite still says where it points).
- **Help window completeness**: a `Main window/Favorites menu` scope with `fixedKey` entries for `1`–`9` and `0`
  (dispatch-exempt, handled by the menu), the way `volume.select` documents the chooser's Enter.
- **Analytics** (categorical props only): `favorite_opened.surface` becomes `favorites_menu` | `command` (the switcher
  can't open a favorite anymore), plus `via: 'digit' | 'keyboard' | 'pointer'`, so we see whether the number keys earn
  their place. A new `favorites_menu_opened { trigger: 'command' | 'switcher_row' }` shows whether the switcher row is
  how people find it. (A menu-bar accelerator and a palette pick both arrive as the command, so `command` doesn't split
  further.)

## Open questions for David

1. **Duplicate needs a new default key.**
   - (a) ⇧F5 **(recommended)**: Total Commander's copy-within-the-same-folder key, the sibling of the ⇧F6 rename Cmdr
     already ships, and free in the registry today. Finder habits lose ⌘D; the palette, right-click menu, and File menu
     still reach it. (Also corrects `pane/DETAILS.md`'s "Duplicate is an F-key idiom in neither Finder nor Total
     Commander"; re-check TC's key reference while implementing.)
   - (b) No default key.
   - (c) Keep ⌘D for Duplicate and put favorites on ⌃D (TC/DC's Windows key). Listed for completeness; it goes against
     the brief.
2. **The error screen's Technical details holds ⌘D above every binding**, so ⌘D on an error screen would never open
   favorites, and an error screen is exactly where someone wants to jump elsewhere.
   - (a) Move it to ⌘I **(recommended)**: reads as "info", and Get info's ⌘I lives in the File list scope, which never
     renders beside the error screen (sibling scopes), so nothing conflicts.
   - (b) Keep ⌘D there: favorites stay keyboard-unreachable on an error screen.
   - (c) Drop the key: the disclosure stays a button.
3. **`0` when the current folder is already a favorite.**
   - (a) A disabled row that says so **(recommended)**: `0` keeps its place, and pressing it explains itself.
   - (b) Hide the row: shorter menu, but `0` silently does nothing.
   - (c) Run the add anyway: the store's re-add moves the favorite to the END, which from this menu looks like a random
     reorder.

## Menu primitive shape (M1's design, to confirm while building)

- **Entries are data**: `MenuEntry = item | heading | separator` in `lib/ui/menu-types.ts` (kept in a `.ts` for the
  reason its header states). An item has `value`, `label`, `icon?`, `disabled?`, `tooltip?`, `accelerator?` (a single
  key rendered in a leading number column and activating the item), and `reorderGroup?` (consecutive items in one group
  reorder among themselves).
- **A controller owns the behavior**: `createMenuController(deps)` in `lib/ui/menu-controller.svelte.ts`, the codebase's
  factory idiom (like `favorites-controller`, `enter-menu`). It holds the highlight (skipping headings, separators, and
  disabled rows), the keyboard vs pointer mode with the 5 px rule, `handleKey(e): boolean` (wrap-around arrows as in the
  switcher, Home/End, Enter/Space, Escape, accelerators, ⌥↑/⌥↓ within a reorder group, and nothing at all while an item
  is being edited), scroll-into-view, and pointer-drag reorder. The pure reorder math moves from `favorites-reorder.ts`
  to `lib/ui/menu-reorder.ts` and emits `onReorder({ group, orderedValues })`; persistence and the optimistic override
  stay the caller's.
- **The component renders**: portaled to `document.body`, glass tokens with the reduced-transparency fallback,
  fit-to-viewport, `role="menu"`, rows as `div role="menuitem"` (a switcher row hosts buttons, so a row can't be a
  `<button>`), headings labelling their group, `role="separator"` lines. A `row` snippet renders custom row content, and
  an `onContextMenu(value, event)` prop hands right-clicks to the caller.
- **Focus and keys**: on open the menu container takes focus (`tabindex="-1"`, `aria-activedescendant` on the
  highlighted row) and the controller routes keys through a document capture listener that lives only while open, the
  model `enter-menu.svelte.ts` already proved deterministic. It restores focus on close. ❗ Verify with VoiceOver once
  and evidence-anchor the result in `lib/ui/DETAILS.md` § Menu. If it holds, M4 retires the switcher's
  `handleVolumeChooserKeyDown` pass-through chain.
- **Submenus arrive in M4** with their only consumer (the switcher's "Connect directly"), carrying the rules from
  `navigation/DETAILS.md` § Dropdown and submenu UI patterns: single cursor, fixed positioning, a ~5 px overlap, row
  hover opens it, highlight only on direct interaction, ArrowRight/ArrowLeft.

## Milestones

Every milestone translates its own new copy into all ten languages (English left in a locale or an unreferenced key
fails the build), updates the colocated `CLAUDE.md` / `DETAILS.md` it touches, runs checks per the `AGENTS.md` cadence,
and commits. David reviews visuals himself, so no screenshot loops.

### M1. One house `Menu`

1. Grow `lib/ui/Menu.svelte` per § Menu primitive shape, minus submenus.
2. Move the archive Enter popup (`pane/enter-menu.svelte.ts`, `enter-menu.ts`) onto the controller. Its behavior stays,
   except arrows now wrap like the switcher's.
3. Tests: controller unit tests (highlight skipping, wrap, accelerators with `e.code` and Shift, editing suspends keys,
   ⌥↑/⌥↓ at both edges, pointer mode exit), a component test, and the tier-3 a11y test the `a11y-coverage` check wants.
4. The contract's other two parts: Debug > Components (`routes/dev/components/sections/MenuSection.svelte`) shows
   headings, separators, accelerators, disabled rows, a reorder group, and a custom row; `docs/design-system.md` §
   Component patterns gets the entry.
5. Rewrite `lib/ui/DETAILS.md` § Menu to match the real component, which also fixes its drift.

About 900–1,300 lines with tests.

### M2. Free ⌘D

Answers open questions 1 and 2. Lands before M3, whose ⌘D default would fail `registry-conflicts.test.ts` otherwise.

1. Duplicate: `commands/sources/file-list.ts`, the accelerators in `menu/menu_bar.rs` and `menu/menu_structure.rs`, the
   `menu_bar_test.rs` expectations, and every doc that says ⌘D for it: `pane/DETAILS.md` § The Duplicate command (plus
   the F-key idiom decision), `pane/duplicate-command.ts`'s header,
   `apps/desktop/src/lib/file-operations/transfer/DETAILS.md`, and `apps/desktop/src-tauri/src/mcp/DETAILS.md`.
2. Technical details: `ErrorPane.svelte`'s listener and its hint (check whether the hint is a `ShortcutChip` or copy),
   the `file-list.ts` entry, and the comments and docs that explain the ⌘D override: `command-registry.ts`,
   `command-ids.ts`, `scope-hierarchy.ts`, `routes/(main)/command-handlers/types.ts` and its `DETAILS.md`,
   `pane/DETAILS.md`.
3. Custom shortcuts persist as deltas, so nobody's own binding moves. Someone who already bound ⌘D to something else
   gets a conflict with `favorites.open` in M3; confirm Settings reports it and the dispatch winner rule picks sanely
   (`shortcut-dispatch.test.ts`).
4. The release notes say where Duplicate went.

About 150–300 lines.

### M3. The favorites menu

1. **The command**: `favorites.open` in `COMMAND_IDS`, a registry entry (scope `Main window`, ⌘D, in the palette with
   keywords `bookmark`, `hotlist`, `favorite`), and a handler that toggles the menu on the focused pane. It also goes in
   the Go menu beside "Add to favorites", with all four places from `lib/commands/CLAUDE.md` § Gotchas: `command_map.rs`
   both directions, `menu_bar.rs`, and `menuCommands`. Plus the `Main window/Favorites menu` scope and its digit
   entries.
2. **The menu**: `navigation/FavoritesMenu.svelte` plus `navigation/favorites-menu.svelte.ts`, which absorbs
   `favorites-controller.svelte.ts` (optimistic order, reconciliation, rename, remove, persist) and takes drag and
   keyboard mechanics from the primitive. `open-favorite.ts` as above. The rename input keeps all four keystroke guards.
3. **The host**: `VolumeBreadcrumb` holds one `openMenu: 'volumes' | 'favorites' | null`, so the two can't both be open.
   `toggleFavoritesMenu()` joins the FilePane / DualPaneExplorer API, and `isVolumeChooserOpen()` generalizes to
   `isHeaderMenuOpen()` for its three "is any menu open" readers (`key-dispatch.ts`, `+page.svelte`,
   `DualPaneExplorer`). Growth in the two capped files stays at export lines.
4. **The switcher**: delete the Favorites group (including `volume-grouping.ts`'s always-render branch), the favorites
   template branch, the `fav` wiring, `handleVolumeContextAction`'s favorites arm, and the favorite CSS. Add the "See N
   favorites ⌘D" row on top.
5. **Right-click**: a `show_favorite_context_menu` command with `Rename` and `Remove from favorites`, and `is_favorite`
   leaves `show_volume_row_context_menu` (the switcher has no favorite rows anymore). Picks keep riding
   `volume-context-action`'s `rename-favorite` / `remove-favorite`, handled by the favorites menu while it's open.
6. **The add gate** from § Proposed defaults, verification first.
7. **Analytics** per § Proposed defaults, with `src-tauri/src/analytics/DETAILS.md` updated.
8. **Tests**: the favorites cases in `navigation/VolumeBreadcrumb.svelte.test.ts` (keyboard reorder, rename guard) and
   `pane/volume-breadcrumb.test.ts` § Favorites section move to `FavoritesMenu` tests. New: digits, the `0` row's three
   states, favorites past nine, the swap keys in both directions, the switcher row. An E2E spec: ⌘D, press 2, the pane
   lands. ❗ Synthetic key events bypass AppKit's menu, so check once by hand (or over MCP) whether a Go-menu
   accelerator reaches the webview as a keydown or arrives as `execute-command`, and make the in-switcher swap work on
   whichever path is real, `isExplorerOverlayOpen` gate included.
9. **Screenshots for translators**: `test/e2e-playwright/i18n-capture-surfaces-main.ts` captures the switcher's
   favorites empty state today; it moves to the favorites menu (with and without favorites) plus the new switcher row.
10. **Docs**: `navigation/CLAUDE.md` and `DETAILS.md` (§ Editable favorites becomes § Favorites menu),
    `src-tauri/src/favorites/CLAUDE.md` and `DETAILS.md` (they say the store backs the switcher's section),
    `docs/architecture.md` if it names the switcher's favorites.

About 900–1,200 lines with tests.

### M4. The volume switcher on the house `Menu`

1. Add submenus to the primitive (§ Menu primitive shape).
2. Render the switcher through `Menu`: sections with headings, a row snippet carrying the icon, filesystem label, status
   glyphs, badges, and the eject or disconnect button (in a slot that doesn't activate the row), the disk-space line
   folded into its row, and the volume-list timeout warning as a footer.
3. Split `VolumeBreadcrumb.svelte` into the chip (`VolumeBreadcrumb.svelte`) and `VolumeChooserMenu.svelte`. The
   keyboard-mode, submenu, and key handlers in `volume-breadcrumb-handlers.svelte.ts` go (the primitive owns them);
   `getConnectionTooltip` and `shouldShowCheckmark` move to their own small modules.
4. If M1's focus decision held, switcher keys route through the primitive and the `handleVolumeChooserKeyDown` chain
   through FilePane and `key-dispatch.ts` goes.
5. The existing switcher tests are the behavior contract: keep them green with as few edits as possible.
6. `navigation/DETAILS.md` § Dropdown and submenu UI patterns moves to `lib/ui/DETAILS.md` § Menu, since the primitive
   now enforces it. The `file-length` allowlist shrink-wraps itself on the next local run; commit the rewrite.

A large diff, ideally net-negative in lines.

### M5. The last two hand-rolled menus (separable)

`DriveIndexBadge.svelte`'s popover menu (portaling also ends the clipping its docs warn about) and
`ViewerContextMenu.svelte` (the viewer window; plain DOM, so no capability change). About 200–400 lines. Cut it if time
runs short; nothing else depends on it.

**Total**: about four to five agent-days. M3 is the user-facing ship point.

## For David's visual and copy review

- The number column (plain tertiary digits, or keycap-styled).
- One highlight style for every menu: the switcher's `--color-accent-subtle` wash, or the Enter popup's solid accent
  fill. The primitive picks one.
- The "See N favorites ⌘D" row.
- Draft copy: "Favorites" (existing heading key), "Add current folder to favorites", "This folder is already a
  favorite", "See {count} favorites" (with singular and zero forms), "Remove from favorites", "Show favorites" (palette
  and Go menu), and the "Favorites menu" scope title in Settings > Keyboard shortcuts.

## Not in scope

- A favorites settings screen.
- Favorites on non-local volumes: the store's v1 limit stays, and this effort only makes the gate real.
- MCP: `select_volume` already reaches a favorite by name and the `favorites` tool edits the list, so no "open the menu"
  tool.
- Letters or type-to-filter inside the favorites menu.
