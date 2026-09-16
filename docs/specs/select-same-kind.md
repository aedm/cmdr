# Select all of the same kind, and a keyboard-openable context menu

**Problem.** Cmdr can select everything, nothing, or the inverse, and it can select by a typed glob. It can't do the one
selection every file manager user reaches for constantly: "the rest of these, the same kind as this one." Total
Commander binds it to `Alt+Num +`. Alongside it, three smaller gaps: the context menu can only be reached with a mouse,
its accelerator labels are hardcoded strings that go stale the moment a user rebinds anything, and the Select menu shows
no shortcut at all for Invert selection, Select files…, and Deselect files… because their keys carry no modifier.

**Ship shape.** `⌥+` selects every entry of the same kind as the row under the cursor. The Select menu grows an item
whose label says what it will do right now ("Select all with extension `*.pdf`"). The context menu's "Toggle selection"
becomes a "Selection" submenu holding everything the Select menu has. `⌃⏎` opens the context menu from the keyboard,
Finder-style. Both new shortcuts are ordinary registry entries a user can rebind.

## Decisions

All of these were settled with David before any code. They are the answers, not options.

1. **Add, never replace.** The command adds its matches to whatever is already selected, like Total Commander. It never
   clears.
2. **Three cases, keyed off the row under the cursor:**
   - a **folder** → select every folder in the listing, whatever their names (a folder called `name.like.this` is still
     just a folder);
   - a **file with an extension** → select every file with that extension, case-insensitively (`.PDF` counts as `.pdf`),
     folders never;
   - a **file with no extension** → select every extension-less file, folders never.
   - the `..` row → no-op, and the menu label falls back to its neutral form.
3. **Extension rule: `getDisplayExtension`, not `getExtension`.** Two conventions exist in the codebase.
   `apps/desktop/src/lib/utils/filename-validation.ts:86` (`getExtension`) returns the last dot segment with its dot and
   has no folder case; `apps/desktop/src/lib/file-explorer/views/full-list-utils.ts:35` (`getDisplayExtension`) is what
   the **Ext column the user is looking at** shows, and what Rust sorts by. They agree on everything that matters here
   (`archive.tar.gz` → `gz`, `.gitignore` → none) and disagree only on a trailing dot (`file.`), where the column says
   "no extension" and `getExtension` says `"."`. Match what the user sees: use `getDisplayExtension`. ❌ Don't
   reimplement either rule.
4. **`⌥+` is NOT a real menu accelerator.** David wants that combo free for typing. Like `⇧8` / `+` / `-` before it, the
   file pane's keydown handler owns it and the menu only _displays_ it.
5. **Canonical spelling `['⌥⇧=', '⌥+']`**, matching Invert selection's existing `['⇧8', '*']` shape: the main-row key
   plus the numpad key. The menu displays `⌥⇧=`, which is honest on every layout. ❌ No display rule rewriting it to
   `⌥+` — David uses a custom mixed English/Hungarian layout and wants the physical truth.
6. **The context menu reads live shortcuts from the registry**, all 15 of them, not just the new submenu. They are
   hardcoded today and already lie after any rebind.
7. **Menu-bar label debounce: 200 ms, debounce (not throttle).** The context menu and the command itself always compute
   live; only the menu-bar label is debounced, because `⌃⏎` means a user can now open the context menu without the mouse
   trip that used to hide staleness.
8. **The dynamic label is rendered in Rust from `menu_t`**, from a typed payload the frontend pushes — the
   `update_pin_tab_menu` pattern. ❌ Never a frontend-composed literal: `menu/CLAUDE.md`'s "every label comes from
   `menu_t`" invariant holds.
9. **Keep "Toggle selection"** as the first item of the new submenu. It is the only place Space is discoverable.
10. **`⌃⏎` pops at the cursor row, and never scrolls.** When the row isn't rendered (the user scrolled away with the
    scrollbar, or the selection is scattered through 10k rows), fall back to the pane's left edge, vertically centred. A
    keypress that silently moves the user's view is worse than a slightly odd menu position.
11. **Whose selection the keyboard menu acts on is already solved.** Right-click semantics apply unchanged: cursor
    inside the selection → the menu acts on the selection; cursor outside it → on that one row. The disabled header line
    already says which. Reuse `pane-pointer.ts::handleContextMenu`, don't re-derive it.

## What the recon established

Facts the plan rests on, each verified in the tree at the commit this branch started from.

- **A bare accelerator really would swallow the key app-wide.** `menu_bar.rs:178-188` already says so in its own
  comments, which is why those three items carry `NONE` today. Worse, `menu/accelerators.rs:18-118`
  (`frontend_shortcut_to_accelerator`) has **no modifier floor**: hand it `"*"` and it cheerfully returns `Some("*")`, a
  real single-character accelerator. That gap is part of this work.
- **The attributed-title technique already exists here**: `menu/context_menu_header.rs:234-254` (`style_as_header`) sets
  an `NSAttributedString` title with font + colour attributes. A right-aligned shortcut needs one more attribute
  (`NSParagraphStyleAttributeName` → `NSMutableParagraphStyle` with a right `NSTextTab`) and a `"{label}\t{glyph}"`
  string.
- **Two `objc2-app-kit` features are missing** for that: `NSParagraphStyle` and `objc2-core-foundation` (the latter
  gates `NSTextTab::initWithType_location`). Both are already vendored in the pinned `0.3.2` — a feature-list edit in
  `apps/desktop/src-tauri/Cargo.toml:355-390`, no version bump. Use the deprecated `initWithType_location` rather than
  `initWithTextAlignment_location_options`, which would additionally need the `NSText` feature.
- **The menu bar is reachable directly**, `app.menu()` → `ns_app.mainMenu()` → `find_ns_item`
  (`macos_appkit.rs:443-447`). ❌ No `NSMenuDidBeginTrackingNotification` hack needed — that's only for context menus.
  Model the new pass on `set_macos_menu_icons`, and call it from the same three places (startup install, the main↔viewer
  menu swap, `rebuild_menu_bar`) **plus** after `update_menu_item_accelerator`'s remove/recreate, which hands back a
  fresh `NSMenuItem` with no attributed title. Miss one and the glyph silently vanishes.
- **All 15 hardcoded context-menu accelerators live in one function**, `menu_structure.rs::build_context_menu`. Two of
  them (`SHOW_IN_FILE_MANAGER_ACCELERATOR.current()`, `COPY_PATH_ACCELERATOR.current()`) _look_ live but
  `PerPlatform::current()` only picks macOS-vs-Linux at compile time. Every other popup builder in the file already
  passes `None::<&str>`, and `build_breadcrumb_context_menu` (`menu_structure.rs:514-546`) **already takes a live
  shortcut from the frontend** — that is the pattern to generalize.
- **`show_file_context_menu` is excluded from the specta bindings** (`ipc.rs:236`; it's generic over `<R: Runtime>`), so
  new payload fields cost no `bindings.ts` regen. Cheapest command in the app to extend.
- **`popup_at` exists** on Tauri 2.11.5's `ContextMenu` trait, taking a `Position` that is **relative to the window's
  top-left corner**. Plain `popup()` passes `None` and the OS uses the cursor, which is why the mouse path never sent
  coordinates.
- **The whole-listing snapshot already exists**: `FilePane.getEntriesSnapshot()` → `entries-snapshot.ts:34` →
  `getFileRange(listingId, 0, totalCount, showHiddenFiles)`, which reads Rust's `LISTING_CACHE` directly and is
  independent of the rendered virtual window. ❌ Never match against `full-list-cache.svelte.ts`'s `getEntryAt`: it
  returns `undefined` outside the rendered range, so off-screen matches would silently vanish.
- **Bulk add is already cheap**: `selection-state.svelte.ts:195` (`applyIndices`) mutates the `SvelteSet` in place and
  fires `onChanged` exactly once for any number of rows; the status-bar feed tracks only `selectedIndices.size` and
  throttles at 150 ms. 3,000 rows costs one IPC.
- ⚠️ **`FilePane.applyIndices` is the wrong entry point.** `FilePane.svelte:877-896` moves the cursor to the first
  newly-selected row and scrolls it into view on `'add'`. That is right for the Select files… dialog and wrong here — it
  would yank the cursor off the file the user is standing on. Call the selection state's `applyIndices` directly.
- **Frontend row indices include the synthetic `..` at 0** when `hasParent`; backend indices never do. The snapshot
  array already carries the `..` row, so iterating it yields frontend indices with no offsetting, and `applyIndices`
  skips index 0 itself.
- **`eventMatchesCommand` already has a physical-key fallback**, but only for Shift+digit
  (`shortcut-dispatch.ts:102-107`, `physicalDigitCombo`), and it exists precisely because `⇧8` is `*` on US and `(` on
  Hungarian. `⌥⇧=` is the same problem one key over: macOS reports `event.key === '±'`, so `formatKeyCombo` would store
  and compare layout-dependent garbage. Every existing `⌥` binding dodges this by pairing with `⌘` or a non-printable
  key.
- **The palette's dynamic-name plumbing is one hardcoded `if`**: `command-registry.ts:125-142` branches on
  `rest.id === 'app.licenseKey'`. `Command.name` is already a live getter everywhere, `fuzzy-search.ts` rebuilds its
  haystack from scratch each keystroke, and `CommandPalette.svelte:52` is a `$derived` — so nothing caches a name and
  nothing breaks when one changes.
- **Native menus never read `Command.name`.** They resolve through Rust's own `menu_t` catalog, and `Label::License`
  (`menu_spec.rs:82-105`) is a second, independently maintained dynamic-label mechanism. The two systems stay separate;
  say so in the docs so nobody assumes generalizing one fixed the other.

## Milestones

Nine. M3 and M5 are the two points where David can QA something real.

### M1 — Physical-key matching for Option-modified punctuation

Generalize `physicalDigitCombo` into `physicalKeyCombo`: the `Digit*` case as today, plus the punctuation codes already
listed in `key-capture.ts`'s `codeToKey` when a character-altering modifier is held and the produced `event.key` differs
from the physical character. Route the shortcut-**capture** UI through the same helper, so a rebind persists the
physical form rather than `⌥⇧±`.

❌ Don't touch `resolveGlobalKeyAction`. Keeping the fallback out of the global path is deliberate: it's what stops `⇧8`
firing outside the file pane today, and this change shouldn't quietly widen that.

Tests (red first): `⌥⇧=` on a US layout and on a layout where the same physical key types something else, `⌥` plus the
numpad `+`, and a regression that `⇧8` still resolves. `shortcut-vocabulary.test.ts` stays green.

### M2 — Two generic naming features in the registry; delete the `app.licenseKey` special case

- `CommandSource.nameKey` accepts `MessageKey | (() => MessageKey)`. `app.licenseKey` moves its two-key flip into its
  own entry in `sources/app.ts`, and the `if (rest.id === …)` branch in `resolveCommand` goes away. **No behavior
  changes**: Settings > Shortcuts and the help window keep showing the live licence name exactly as they do today.
- `CommandSource` gains an optional `displayName?: () => string`; `Command.displayName` is a getter falling back to
  `name`. **Only the palette reads it** (the `fuzzy-search.ts` haystack and `CommandPalette.svelte`). Settings >
  Shortcuts, the help window, the conflict toast, and the MCP bridge keep reading `name`, which is why the new command's
  Settings row reads the static "Select all of the same kind".

Update `command-registry.parity.test.ts` (it excludes `app.licenseKey` from `EXPECTED_NAMES` and pins both branches
separately — that carve-out stays, it just tests the generic path now) and document both fields in
`lib/commands/CLAUDE.md` / `DETAILS.md`, including the note that Rust's `Label::License` is a separate mechanism.

### M3 — `selection.selectSameKind` (first QA point)

The command itself, no menus yet. David can QA it entirely from the keyboard and the palette.

- `command-ids.ts` (after `selection.invert`), a `sources/file-list.ts` entry mirroring `selection.invert`
  (`shortcuts: ['⌥⇧=', '⌥+']`, `showInPalette: true`, `BLOCKED_BY_DIALOGS`, a `descriptionKey`), a `displayName`
  resolver for the palette, and a handler in `selection-handlers.ts`.
- A new pure helper `pane/select-same-kind.ts` with a colocated test, in the shape of the existing `has-parent.ts` /
  `first-selected-index.ts` pairs: given the cursor entry and the full snapshot, return the frontend indices to add.
  This is where decisions 2 and 3 live, and where TDD pays: write the folder / extension / no-extension / `..` /
  case-insensitivity / `archive.tar.gz` / `.gitignore` / trailing-dot cases red first.
- `FilePane.selectSameKind()`: `getEntriesSnapshot()` → predicate → `selection.applyIndices(idxs, 'add', hasParent)`. ❌
  Not `FilePane.applyIndices` (see the recon note on the cursor jump).
- Key routing: add the id to `selection-keys.ts`'s `selectionCommands`, and a `case` plus a `selectSameKind` dep in
  `pane-key-router.ts`'s `handleSelectionKeys`. Extend `selection-keys.test.ts` and `pane-key-router.test.ts`.

### M4 — Display-only accelerators in the menu bar

- Enable the two `objc2-app-kit` features, each with a comment saying why.
- `ItemSpec` gains `display_accelerator: Option<&'static str>`, carried as a plain glyph (the platform difference is in
  _rendering_, not in the spec). Add a sibling constructor rather than changing every `const fn item()` call site.
- macOS: a `set_display_accelerators` pass modeled 1:1 on `set_macos_menu_icons`, setting an attributed
  `"{label}\t{glyph}"` title with a right-aligned tab stop and `secondaryLabelColor`, reusing `style_as_header`'s shape.
  Call it from all four sites listed in the recon.
- Linux: compose the suffix in Rust after the `menu_t` lookup, following `menu_items.rs::detach_label`. ❌ Not through
  `menu_t_with`'s interpolation family.
- **Modifier floor in `frontend_shortcut_to_accelerator`**: a combo with no `⌘`/`⌃`/`⌥` yields no real accelerator and
  routes to the display path instead. This is what makes a rebind honest — the glyph a user sees always reflects their
  effective binding, and a bare key can never be registered by accident.
- Apply to Invert selection (`⇧8`), Select files… (`+`), Deselect files… (`-`).
- `menu_bar_test.rs`: the renderer needs a segment for the new field, or the snapshot proves nothing. Expect the macOS
  and Linux blocks for this submenu to **diverge for the first time** — that's correct, not a mistake to paper over.

### M5 — The Select-menu item, with its live label (second QA point)

- `SELECT_SAME_KIND_ID`, both `command_map.rs` arms (`CommandScope::FileScoped` — the generic `_ => true` branch in
  `item_states.rs` already covers its enabled state, no new match arm), the `menu_bar.rs` row between Deselect all and
  Invert selection, `NONE` real accelerator plus the `⌥⇧=` display accelerator, the `menuCommands` entry, and the
  `rust-command-id-drift.test.ts` update. Follow the `selection.invert` pattern (tracked, in `menuCommands`), ❌ not the
  `selection.toggle` excuse pattern.
- Four `menu.json` keys, rendered in Rust: `menu.select.sameKind` (the neutral fallback the bar is built with),
  `menu.select.sameExtension` (`{extension}` placeholder), `menu.select.noExtension`, `menu.select.allFolders`. Phrase
  the placeholder string so translators can restructure around it — several target languages inflect.
- A new IPC carrying a typed payload (`kind` + optional extension) that Rust renders with `menu_t` and `set_text`.
  Frontend pushes it on cursor change, **200 ms debounced, and only when the rendered payload actually changed**, so
  holding an arrow key costs nothing.

### M6 — Context-menu accelerators from the registry

Replace all 15 hardcoded strings in `build_context_menu` with values the frontend sends in the `show_file_context_menu`
payload (a command-id → combo map), converted through `frontend_shortcut_to_accelerator` at build time. The context menu
is rebuilt per popup, so there's no remove/reinsert dance — this is strictly simpler than the menu bar.

A popup accelerator never fires, so bare keys are safe as plain display text here: `Space` keeps working the way it does
today, it just comes from the registry now instead of a literal. ❌ No attributed-title work in the context menu.

### M7 — The "Selection" submenu

"Toggle selection" becomes a "Selection" submenu (new key `menu.context.selection`; `menu.context.toggleSelection`
survives, now inside it), ordered: Toggle selection, Select all, Deselect all, Select all with extension …, Invert
selection, separator, Select files…, Deselect files…. Popup submenu items take accelerators exactly like top-level ones,
so nothing special is needed there. Its dynamic label is computed **live at popup time** and sent in the payload — ❌
never read from M5's debounced menu-bar value.

### M8 — `⌃⏎` opens the context menu at the cursor row

- A new command (`shortcuts: ['⌃Enter']`, palette-visible) with its own label and description keys. `⌃⏎` is free: the
  only `⌃` bindings today are `⌃Tab` / `⌃⇧Tab`, and `global-contextmenu.ts` handles the DOM `contextmenu` event, not
  keydown, so there's no collision.
- Reuse `pane-pointer.ts::handleContextMenu(entry)` unchanged, so decision 11 costs nothing.
- `showFileContextMenu` gains an optional window-relative position; Rust calls `popup_at` when it's present and
  `popup()` when it isn't, so the mouse path is untouched.
- Row rect from `document.getElementById('file-<index>')` (both list views already set that id), anchored like
  `enter-menu.ts::enterMenuAnchor` (`left + 16`, `bottom`). Fallback: the scroll container's rect, left edge, vertically
  centred. ❌ No scrolling.
- ⚠️ **Verify empirically**: `popup_at`'s position is documented as relative to the **window's** top-left, while a DOM
  rect is viewport-relative. Any title-bar or decoration offset, and the device-pixel-ratio question, have to be
  measured in the running app at this milestone, not assumed. Nothing in the codebase converts between these two spaces
  today; this is the one genuinely new piece of plumbing in the effort.

### M9 — Docs and localization

Colocated `C+D.md` files get updated as each milestone lands, not here; this milestone is the final sweep plus the
translator pass over the ~8 new keys in 10 locales, per `docs/guides/i18n-translation.md`. **It runs after David's QA**,
so his copy edits translate once rather than twice.

## Order and parallelism

- **Wave 1** (independent surfaces, in parallel): M1 + M2 (frontend shortcuts and registry) ‖ M4 (Rust menu bar,
  `Cargo.toml`, `accelerators.rs`) ‖ M6 (Rust context menu and its IPC). M4 and M6 share the `menu/` directory but touch
  disjoint files; M4 owns `accelerators.rs` and M6 only calls into it.
- **Wave 2**: M3 (needs M1 and M2) ‖ M8 (needs M6's payload shape).
- **Wave 3**: M5 and M7 together — same surfaces, both need M3 plus wave 1.
- **Wave 4**: M9, after QA.

## Not in scope

- Rewriting `Label::License` / the Rust menu's own dynamic-label mechanism to share the frontend's. They stay two
  systems; the duality gets documented instead.
- Making `⇧8` (or any bare key) fire through the global dispatch path rather than the file pane's handler.
- A display rule that renders `⌥⇧=` as `⌥+` or `⇧8` as `*` (decision 5).
