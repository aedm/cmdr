# Favorites menu (⌃D)

**The quest**: GitHub #91. Opening a favorite takes a click on the volume switcher, and there's no shortcut. In Total
Commander and Double Commander, ⌃D opens the favorites list (TC calls it the "Directory hotlist"). A second commenter
asked for number keys: the menu shows `1 Foo`, `2 Bar/Baz`, `3 Qux`, and pressing 3 lands in Qux.

**What we build**: ⌃D opens a favorites menu for the focused pane, at the spot the volume switcher opens (top-left of
the pane). Digits go straight to a favorite, and `0` adds the current folder. The switcher loses its Favorites section
and gets one row instead, "See 12 favorites ⌃D", which keeps favorites one click away and teaches the key. First though,
the switcher's hand-rolled dropdown becomes the house `Menu` primitive, so every menu in the app shares one keyboard and
pointer model.

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
  is menu wiring only, in M3.
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
  - The volume switcher's dropdown, the rich one this effort lifts out: grouped sections with headings and separators,
    keyboard vs pointer mode (a 5 px move exits keyboard mode), wrap-around arrows, Home/End, a submenu with the
    single-cursor rule and fixed positioning, fit-to-viewport, scroll-into-view, drag and keyboard reorder, inline
    rename. It lives in `VolumeBreadcrumb.svelte` (1,828 lines against a 1,845 allowlist entry) plus
    `volume-breadcrumb-handlers.svelte.ts`. Its UI rules are written up in `navigation/DETAILS.md` § Dropdown and
    submenu UI patterns.
  - `DriveIndexBadge.svelte`'s popover menu (which its own docs note gets clipped by the dropdown's `overflow-y: auto`).
  - `routes/viewer/ViewerContextMenu.svelte`.
- **Keys while the switcher is open**: `pane/key-dispatch.ts` routes to an open chooser BEFORE type-to-jump (so digits
  reach a menu rather than the jump buffer) and swallows everything else, and `+page.svelte::isExplorerOverlayOpen`
  suppresses central dispatch while `isVolumeChooserOpen()`. `FilePane.svelte` and `DualPaneExplorer.svelte` carry the
  pass-through exports and are both at their size cap.
- **Analytics**: `favorite_opened { surface: 'breadcrumb' | 'command' }` from `favorites-analytics.ts`, fired by the
  switcher and by `pane/volume-selection.ts` (palette and MCP). Contract: `src-tauri/src/analytics/DETAILS.md`.

## Decisions taken

1. **"Favorites"** everywhere: UI, command ids, code, events. No "bookmarks" or "hotlist" except as palette keywords.
2. **⌃D opens it** on the focused pane (David, 2026-09-16). It's what TC and DC bind, and it's free: ⌃Tab and ⌃⇧Tab are
   the only Control defaults in the registry (verified 2026-09-16). So **Duplicate keeps ⌘D**, and the error screen's ⌘D
   Technical details is untouched. ❗ macOS reads ⌃D as forward-delete inside a text field; the central typing guard
   already bails there, so nothing else is needed.
3. **`0` is a disabled row when the current folder is already a favorite** (David, 2026-09-16), saying so, rather than
   re-adding (which the store would answer by moving that favorite to the end of the list).
4. **An in-app menu on the house `Menu` primitive.** No native popup (DC's), no settings cockpit (TC's and DC's). The
   list itself is where favorites get managed: drag or ⌥↑/⌥↓ to reorder, right-click to rename or remove.
5. **One house `Menu`: grow `lib/ui/Menu.svelte`**, rather than adding a second menu component beside it. The name is
   already the house menu's and has one small consumer; two menu primitives is the drift we're ending.
6. **The right-click menu on a favorite stays native**, like every right-click in Cmdr, with `Rename` and
   `Remove from favorites`. Only the list is in-app.
7. **The switcher's Favorites section goes**, replaced by one selectable row at the top: "See 12 favorites" plus a live
   `ShortcutChip` for ⌃D. Enter or a click swaps the switcher for the favorites menu, in place.
8. **Order** (David, 2026-09-16): the primitive (M1) and the switcher port (M2) go first and are a pure refactor with no
   user-visible change, then **David QAs**. M3 (the feature) and M4 wait for that.

## Proposed defaults for M3 (agent-proposed; say if any is wrong)

- **Anchor**: exactly where the switcher opens, under the pane's volume chip, fitted to the viewport. One menu at a
  time: opening either menu on either pane closes any other.
- **Digits**: `1`–`9` open the Nth favorite at once, `0` runs the add row. Favorites past nine are listed with an empty
  number column and reached by arrows or pointer. Match on `e.code` (`Digit0`–`Digit9`, `Numpad0`–`Numpad9`) with no
  ⌘/⌃/⌥ and Shift allowed, so AZERTY layouts (digits need Shift) work. It's a class-of-key matcher, which
  `cmdr/no-raw-key-match` accepts.
- **The keys that swap menus work inside menus.** Central dispatch is suppressed while a header menu is open, so the
  menu host matches with `eventMatchesCommand` (which follows a rebind): ⌃D inside the switcher swaps to favorites (the
  row just taught that key, so it must work right there), ⌃D inside favorites closes it, and the pane's chooser key (⌥F1
  / ⌥F2) inside favorites swaps back.
- **Opening a favorite** does what the switcher's favorite branch does today (`resolvePathVolume`, then a volume change
  onto the containing volume with the favorite's path). That branch is duplicated in
  `VolumeBreadcrumb.handleVolumeSelect` and `volume-selection.ts`; it becomes one `navigation/open-favorite.ts` used by
  the menu and the palette/MCP path.
- **The `0` row** is disabled, with its reason as a tooltip, when the current folder is already a favorite (decision 3),
  or when the pane is somewhere a favorite can't point. Favorites are local paths in v1 and `favorites/DETAILS.md` says
  the add surfaces gate that, but `favorites.add` passes `getFocusedPanePath()` unchecked today. ❗ Verify first what a
  favorite on an SMB, MTP, SFTP, archive, or search-results pane does now (some may already work through
  `resolvePathVolume`), then gate the `0` row AND `favorites.add` on one capability predicate, ❌ never a path-string
  test.
- **Empty state**: the heading, the existing "(Your favorites will show here)" placeholder, a separator, and the `0`
  row.
- **Tooltips** reuse `favorite-tooltip.ts` (the path first, so a renamed favorite still says where it points).
- **Help window completeness**: a `Main window/Favorites menu` scope with `fixedKey` entries for `1`–`9` and `0`
  (dispatch-exempt, handled by the menu), the way `volume.select` documents the chooser's Enter.
- **Analytics** (categorical props only): `favorite_opened.surface` becomes `favorites_menu` | `command` (the switcher
  can't open a favorite anymore), plus `via: 'digit' | 'keyboard' | 'pointer'`, so we see whether the number keys earn
  their place. A new `favorites_menu_opened { trigger: 'command' | 'switcher_row' }` shows whether the switcher row is
  how people find it. (A menu-bar accelerator and a palette pick both arrive as the command, so `command` doesn't split
  further.)

## The `Menu` API

This is the deliverable of M1 and the contract M2 ports onto. It covers everything the switcher does today plus reorder
for sections that ask for it. The test of the shape: a consumer should be able to build a menu from data plus a couple
of snippets, and never touch a key handler, a highlight index, or a `getBoundingClientRect`.

**Design rules**

- **Data in, callbacks out.** The caller hands over sections and gets `onSelect` / `onReorder` / `onContextMenu` back.
  The primitive holds no copy of the caller's data and no persistence.
- **The primitive owns interaction**: open and close, anchoring, highlight, keyboard, pointer, keyboard-vs-mouse mode,
  scrolling, submenus, drag reorder, focus.
- **Snippets decorate, they don't re-implement.** The default row (checkmark, icon, label) is there; `label`,
  `trailing`, and `below` snippets add to it. No consumer should have to rebuild a row to add a badge.
- **A control inside a row does its own thing.** A click on a `<button>` / `<a>` / `<input>` inside a row never
  activates the row, so the eject button needs no `stopPropagation` at the call site.
- **One level of submenu** and no type-ahead, virtualization, or checkbox/radio items in v1: nothing needs them, and
  each is addable later without moving the seams.

**Files** (all in `apps/desktop/src/lib/ui/`)

- `Menu.svelte`: the surface. Portal, glass, positioning, rows, submenu, footer.
- `menu-controller.svelte.ts`: `createMenu(deps)`, the controller, in the codebase's factory idiom. ❗ NOT
  `menu.svelte.ts`: case-insensitive macOS resolves `./menu.svelte` to the `Menu.svelte` COMPONENT while CI resolves it
  to the controller, so one import would mean two different modules (measured 2026-09-16 on vite 8).
- `menu-types.ts`: the types below (it stays a `.ts` for the reason its header already gives).
- `menu-navigation.ts`: pure key-to-action and next-highlight math.
- `menu-reorder.ts`: the pure drag math, moved from `navigation/favorites-reorder.ts` (`moveItem`,
  `clampedReorderTarget`, `pointerInsertionSlot`, `pointerReorderTarget`) with its tests.

**Types**

```ts
/** A lucide glyph, or an image the caller already has a URL for (a volume or folder icon). */
export type MenuIcon = { lucide: IconName } | { src: string }

export interface MenuItem<T = unknown> {
  value: string // stable identity, emitted on select
  label: string
  icon?: MenuIcon
  checked?: boolean // renders the leading checkmark
  disabled?: boolean // greyed, unfocusable, never activates
  tooltip?: string
  submenu?: MenuItem<T>[]
  data?: T // the caller's payload, handed back on select and to every snippet
}

export interface MenuSection<T = unknown> {
  id: string
  heading?: string
  items: MenuItem<T>[]
  /** Rows reorder within this section by drag and ⌥↑/⌥↓; the caller persists in `onReorder`. */
  reorderable?: boolean
  /** Shown (disabled, unfocusable) when the section is empty, so the section still reads as a real state. */
  emptyLabel?: string
}
```

**Controller**

```ts
const menu = createMenu<VolumeInfo>({
  getSections: () => sections, // read live, so the menu tracks the caller's state
  onSelect: (item) => { … },
  onReorder: ({ sectionId, orderedValues, from, to }) => { … }, // optional
  onContextMenu: (item, event) => { … },                        // optional, right-click on a row
  onKey: (event) => false,       // optional first look, for the caller's own combos
  isEditing: () => renaming !== null, // optional: an inline editor owns the keys, and drag is off
  onOpenChange: (open) => { … },      // optional
  restoreFocus: () => paneEl?.focus(), // optional, called on close
})

menu.openUnder(chipEl) // anchored, viewport-fitted, max-height set
menu.openAt({ x, y }) // point-anchored (context menus, the Enter popup)
menu.toggleUnder(chipEl)
menu.close()
menu.isOpen // reactive
menu.highlightedValue // reactive
menu.highlight(value) // set the cursor (open-at-current-item)
menu.handleKey(event) // for a host that routes keys itself
menu.destroy() // from the host's teardown
```

**Component**

```svelte
<Menu {menu} ariaLabel="Volumes" minWidth={220}>
    {#snippet label(ctx)}…{/snippet}
    {#snippet trailing(ctx)}…{/snippet}
    {#snippet below(ctx)}…{/snippet}
    {#snippet footer()}…{/snippet}
</Menu>
```

It renders nothing while closed, so the consumer writes no `{#if}`. Each snippet takes one
`MenuRowContext<T> = { item, section, index, highlighted, dragging }` argument. `label` replaces the row's text (the
inline rename field), `trailing` fills the right end of the row (badges, dots, the eject button), `below` adds a
sub-line under the row (the disk-space bar), and `footer` sits under the last section (the volume-list timeout warning).

**What the primitive owns**

- **Keyboard**: arrows wrap and skip headings, separators, disabled rows, and empty placeholders; Home/End; Enter and
  Space activate; ArrowRight opens a submenu and ArrowLeft closes it, and while one is open the arrows walk ITS rows
  (its cursor is a value, so a multi-item submenu lights exactly one); ⌥↑/⌥↓ reorder inside a reorderable section and
  carry the highlight with the moved row. While `isEditing()` is true it handles nothing, so the editor keeps every
  keystroke.
- **Escape closes the open submenu if there is one, otherwise the menu**, down one path and with no second document
  listener, settling the switcher's two disagreeing Escape paths.
- **Every key while open.** `onKey` gets the first look, then the menu's own handling, and anything left over is
  swallowed: an open menu owns the keyboard, which is what keeps the panes behind it inert.
- **Focus**: the menu container takes focus on open (`tabindex="-1"`, `aria-activedescendant` on the highlighted row)
  and calls `restoreFocus` on close. Keys route through a document capture listener that lives only while open, the
  model `enter-menu.svelte.ts` already proved deterministic against focus timing. ❗ Check `aria-activedescendant` on a
  portaled container with VoiceOver once, and evidence-anchor the answer in `lib/ui/DETAILS.md` § Menu.
- **Pointer**: hover moves the highlight, unless keyboard mode is on; a pointer move over 5 px leaves keyboard mode; a
  click activates; a right-click calls `onContextMenu`; a pointer-down outside closes.
- **Reorder**: pointer drag past a threshold, the drop-line cue at the insertion gap, the dragged row's own styling, and
  window-level listeners cleaned up on close and destroy. `onReorder` fires once, on drop.
- **Placement**: fixed position, clamped into the viewport, `max-height` to the space below the anchor with its own
  scroll, the highlighted row scrolled into view, submenus positioned off the row's rect with a small overlap and the
  single-cursor rule (a submenu's cursor replaces the parent's).
- **Surface**: portal to `document.body`, glass tokens with the reduced-transparency fallback, `role="menu"`, rows as
  `div role="menuitem"` (a row hosts buttons, so it can't be a `<button>`), sections as labelled groups.

- **Test hooks**, documented in `lib/ui/DETAILS.md` § Menu as a contract other suites rely on, the way `.ui-popover` and
  the `.select-*` classes already are: `data-*` attributes (❌ not CSS classes, which are styling and get renamed)
  naming the surface, a row by its `value`, the highlighted / checked / disabled states, a section by id, the submenu
  and its highlighted row, and the drag state including the slot the drop-line cue sits in. ❗ Without these, M2's
  characterization pins (which select on switcher markup the port deletes) can only be rewritten by hand, and the proof
  that the port changed nothing weakens to "the new tests pass". **Shipped in M1**: `data-menu` (+
  `data-keyboard-mode`), `data-menu-submenu`, `data-menu-section="<id>"`, `data-menu-empty`, `data-menu-row="<value>"`
  with `data-highlighted` / `data-checked` / `data-disabled` / `data-dragging`, and `data-drop-cue="above|below"`
  carrying `data-drop-slot="<n>"`.

**What the caller still owns**: the data and its order (including an optimistic override while a reorder persists),
persistence, navigation, toasts, and any inline editor's state.

## Milestones

M1 and M2 are a refactor with no user-visible change, and David QAs after M2. Every milestone updates the colocated
`CLAUDE.md` / `DETAILS.md` it touches, runs checks per the `AGENTS.md` cadence, and commits per logical step. M3 also
translates its own new copy into all ten languages (English left in a locale, or an unreferenced key, fails the build).

### M1. The house `Menu`

1. Build it per § The `Menu` API: `Menu.svelte`, `menu-controller.svelte.ts`, `menu-types.ts`, `menu-navigation.ts`,
   `menu-reorder.ts` (the pure reorder math moved out of `navigation/favorites-reorder.ts`, tests included).
2. Move the archive Enter popup (`pane/enter-menu.svelte.ts`, `enter-menu.ts`, `FilePane.svelte`'s render site) onto it.
   Behavior stays, except arrows now wrap like the switcher's. Its document-capture listener becomes the primitive's.
3. Tests: pure unit tests for navigation and reorder math, controller tests (highlight skipping, wrap, ⌥↑/⌥↓ at both
   edges, `isEditing` suspension, pointer-mode exit, `onKey` precedence, swallowing), a component test (rendering,
   snippets, a control inside a row not activating it), and the tier-3 a11y test `a11y-coverage` requires.
4. The rest of the primitive contract: a Debug > Components section
   (`routes/dev/components/sections/MenuSection.svelte`) showing headings, an empty section, disabled rows, a
   reorderable section, a submenu, and the three snippets; plus the `docs/design-system.md` § Component patterns entry.
5. Rewrite `lib/ui/DETAILS.md` § Menu against the real component, which also clears its drift, and give
   `lib/ui/CLAUDE.md` its one-line pointer.

About 1,100–1,500 lines with tests.

### M2. The volume switcher on the house `Menu`

Characterization first: the switcher's behavior gets pinned in tests BEFORE the port, so a behavior-preserving refactor
is provable (`docs/guides/multi-agent-refactors.md`).

1. ✅ **Done** (commit `9bc288359`): 16 pins across `navigation/VolumeBreadcrumb.svelte.test.ts`,
   `pane/volume-breadcrumb.test.ts`, and `navigation/favorites-controller.svelte.test.ts` covering highlight-on-open,
   keyboard-vs-pointer mode, the submenu's four key paths and the single-cursor rule, placement and scroll-into-view,
   the three row controls that mustn't activate their row, right-click targeting, the empty-favorites placeholder being
   skipped by arrows, and the drag cue's gap. Every pin was verified to fail when its behavior is broken. Two
   carry-forwards for the port:
   - **Escape is deliberately unified.** Today the routed handler closes only an open submenu while a DOM-dispatched
     Escape closes the whole dropdown through a second document listener. The primitive has one path: Escape closes the
     submenu if one is open, otherwise the menu. So the pin asserting today's DOM-Escape-closes-everything case gets
     updated with the port, ❌ never quietly deleted.
   - `volume-breadcrumb-handlers.svelte.ts` now measures 94.3% covered, so its `coverage-allowlist.json` entry looks
     unneeded. Left as a warn on purpose (the file is about to lose most of its contents to the primitive anyway); ❗
     removing the entry needs David's consent.
2. ✅ **Done** (steps 2–7, commits `b79a68737` + `2d743c8fd`). `VolumeChooserMenu.svelte` renders the switcher through
   `Menu` (sections from `volume-grouping.ts`, all four snippets, favorites `reorderable`), `VolumeBreadcrumb.svelte` is
   the chip alone (1,828 → 422 lines), `volume-breadcrumb-handlers.svelte.ts` keeps only the chip's inline popup,
   `favorites-controller.svelte.ts` keeps rename / remove / the optimistic order, and the `handleVolumeChooserKeyDown`
   chain is gone (`key-dispatch.ts` still swallows every key while a switcher is open, for the rename input's
   keystrokes). § Dropdown and submenu UI patterns moved into `lib/ui/DETAILS.md` § Menu. New colocated pieces both
   placements share: `ConnectionDot`, `UsbSpeedDot`, `DetachButton`, `detach-volume.ts`, `connect-directly-row.ts`,
   `drive-badges.svelte.ts`, `connection-tooltips.ts`, `volume-checkmark.ts`.

   **What the port deliberately changed** (everything else is byte-for-byte the same behavior):
   - **Escape is unified**, as planned above. No pin had to change for it in the end: the DOM-Escape pin had no submenu
     open, and the routed submenu-Escape pin already expected submenu-first.
   - **Three pins were re-pointed with their assertion adjusted**, since the primitive's markup says the same thing
     differently: rows and the empty placeholder carry `tabindex="-1"` (out of the tab order, cursor owned by
     `aria-activedescendant`) where the old markup had no attribute, and an unopenable device row is `data-disabled`
     rather than `.is-unavailable`. That last one also means the arrows now SKIP such a row instead of landing on it.
   - **The submenu arrow sits at the row's far right**, after the eject button, because the primitive renders it last.
     Before, `.submenu-trigger` sat between the connection dot and the eject button. Two primitive bugs fell out of the
     port and are fixed rather than carried: leaving keyboard mode with the pointer resting over a row lit two rows
     (`429923068`), and a pointer-down on a control BESIDE the anchor counted as outside, so the chip's eject button
     closed the list it was ejecting from (`keepOpenWithin`).

Net effect: the switcher's own code went from 1,828 + 200 lines to 422 (chip) + 755 (list) + ~330 across the eight
shared modules, and every menu behavior it used to hand-roll is now the primitive's.

✅ **David QA'd it on 2026-09-16: "it looks and feels as before."** M3 is cleared to start.

Two decisions are still open, neither blocking M3:

- **Where the submenu arrow sits.** It's at the row's far right now (what macOS does); it used to sit between the
  connection dot and the eject button, since it belongs to that dot. (a) Leave it, free. (b) Render it before
  `trailing`, a one-line no-API change that lands it in a third spot, right after the label and ahead of the filesystem
  tag. (c) Restore the old spot exactly: `hasSubmenu` on `MenuRowContext`, a `submenuArrow: 'end' | 'caller'` prop so
  the primitive suppresses its own, and a small exported arrow component, about 20 lines plus permanent API surface that
  re-opens the placement question for every later consumer. Recommendation: (a).
- **The `volume-breadcrumb-handlers.svelte.ts` coverage-allowlist entry** (now 94.3% covered, so it looks unneeded).
  Removing an allowlist entry needs David's consent, so it stays as a warn until he answers.

### M3. The favorites menu

1. **The primitive's last piece**: `accelerator?: string` on `MenuItem`, rendered in the leading number column and
   matched per § Proposed defaults, with its catalog row and tests.
2. **The command**: `favorites.open` in `COMMAND_IDS`, a registry entry (scope `Main window`, ⌃D, in the palette with
   keywords `bookmark`, `hotlist`, `favorite`), and a handler that toggles the menu on the focused pane. It also goes in
   the Go menu beside "Add to favorites", with all four places from `lib/commands/CLAUDE.md` § Gotchas: `command_map.rs`
   both directions, `menu_bar.rs`, and `menuCommands`. Plus the `Main window/Favorites menu` scope and its digit
   entries.
3. **The menu**: `navigation/FavoritesMenu.svelte` plus `navigation/favorites-menu.svelte.ts`, which absorbs
   `favorites-controller.svelte.ts`. `open-favorite.ts` as above. The rename field keeps all four keystroke guards.
4. **The host**: `VolumeBreadcrumb` holds one `openMenu: 'volumes' | 'favorites' | null`, so the two can't both be open.
   `toggleFavoritesMenu()` joins the FilePane / DualPaneExplorer API, and `isVolumeChooserOpen()` becomes
   `isHeaderMenuOpen()` for its three readers. Growth in the two capped files stays at export lines.
5. **The switcher**: delete the Favorites section (including `volume-grouping.ts`'s always-render branch) and add the
   "See N favorites ⌃D" row on top.
6. **Right-click**: a `show_favorite_context_menu` command with `Rename` and `Remove from favorites`, and `is_favorite`
   leaves `show_volume_row_context_menu`. Picks keep riding `volume-context-action`'s `rename-favorite` /
   `remove-favorite`.
7. **The add gate** and **analytics** per § Proposed defaults, with `src-tauri/src/analytics/DETAILS.md` updated.
8. **Tests**: the favorites cases move from the switcher suites to `FavoritesMenu` tests, plus digits, the `0` row's
   three states, favorites past nine, the swap keys both ways, and the switcher row. An E2E spec: ⌃D, press 2, the pane
   lands.
9. **Screenshots for translators**: `test/e2e-playwright/i18n-capture-surfaces-main.ts` captures the switcher's
   favorites empty state today; it moves to the favorites menu (with and without favorites) plus the new switcher row.
10. **Docs**: `navigation/CLAUDE.md` and `DETAILS.md` (§ Editable favorites becomes § Favorites menu),
    `src-tauri/src/favorites/CLAUDE.md` and `DETAILS.md` (they say the store backs the switcher's section),
    `docs/architecture.md` if it names the switcher's favorites.

About 900–1,200 lines with tests.

### M4. The last two hand-rolled menus (separable)

`DriveIndexBadge.svelte`'s popover menu (portaling also ends the clipping its docs warn about) and
`ViewerContextMenu.svelte` (the viewer window; plain DOM, so no capability change). About 200–400 lines. Cut it if time
runs short; nothing else depends on it.

**Total**: about five agent-days, of which M1 and M2 are roughly half. M3 is the user-facing ship point.

## For David's visual and copy review

- M2 is meant to look identical to today. One deliberate exception to settle: the two menus have different highlight
  styles (the switcher's `--color-accent-subtle` wash, the Enter popup's solid accent fill), and the primitive has to
  pick one. M1 goes with the switcher's wash, since it's the surface with the most rows.
- M3: the number column (plain tertiary digits, or keycap-styled) and the "See N favorites ⌃D" row.
- M3 draft copy: "Favorites" (existing heading key), "Add current folder to favorites", "This folder is already a
  favorite", "See {count} favorites" (with singular and zero forms), "Remove from favorites", "Show favorites" (palette
  and Go menu), and the "Favorites menu" scope title in Settings > Keyboard shortcuts.

## Not in scope

- A favorites settings screen.
- Favorites on non-local volumes: the store's v1 limit stays, and this effort only makes the gate real.
- MCP: `select_volume` already reaches a favorite by name and the `favorites` tool edits the list, so no "open the menu"
  tool.
- Letters or type-to-filter inside the favorites menu.
