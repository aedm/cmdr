# Navigation

Back/forward history, path resolution, paged keyboard shortcuts, and the two menus on the pane's volume chip.

## Module map

- Paths and history: `navigation-history.ts` (immutable stack), `real-folder-history.ts` (the newest non-snapshot entry
  in one), `path-navigation.ts`, `path-resolution.ts`, `keyboard-shortcuts.ts`.
- `VolumeBreadcrumb.svelte` is the CHIP, hosting both menus: `VolumeChooserMenu.svelte` (the switcher) and
  `FavoritesMenu.svelte` + `favorites-menu.svelte.ts` (⌃D). Plus a helper per concern (grouping, disk space, connection
  state, eject, labels, badges) and the dots both placements share (`ConnectionDot`, `UsbSpeedDot`, `DetachButton`).
- `server-row-actions.ts` holds a SERVER row's menu, shared with the hub and the palette.

## Must-knows

- **History pushes on listing success AND failure.** Drop the `listing-error` branch and a TCC-restricted folder stays
  out of history, so `Cmd+[` jumps back two.
- **Callers holding per-entry resources need `push()`**: only it returns `droppedEntries` to release dropped refs.

- **`resolveValidPath` stops at a scheme path's floor and RETURNS it**, never `~`, `/`, or `null`: a remote path can
  answer no probe, so a plain walk lands the pane on the boot disk. It stays in `path-resolution.ts`, which exists only
  to break a cycle.
- **❗ Pass `volumeId` wherever the walk should stay on the pane's volume**: without it every probe asks the boot disk,
  which says "gone" for a phone's folders. Who passes it: `DETAILS.md` § `path-resolution.ts`.
- **A volume-switch correction has two gates**: ONE global `correctionGen` (❌ not one per pane), plus its pane's token
  and position, so it never moves a pane off a navigation that followed.
- **`containingVolumeId` comes from `resolvePathVolume(currentPath)`, ❌ not the `volumeId` prop** (a favorite's is
  virtual), so the checkmark tracks the real one.
- **Read `connectionState` through `connection-state.ts`'s predicates, ❌ never `!= null`**: four backends carry one,
  plus a `saved` row, so "has a value" answers nothing. `showsDisconnect` is "a volume is REGISTERED under this id", so
  both sign-in states are IN; only `saved` has no subject.
- **❗ A PHONE's detach control is decided by `deviceReadiness`, ❌ never by `isEjectable` or `connectionState`.** A
  device row carries `isEjectable: true` unconditionally and no `connectionState` at all, so `isVolumeEjectable` reads
  readiness for one and the two session predicates for every other row. Without it a greyed `unavailable` phone nobody
  can open still offered a live Disconnect.
- **A SERVER row says Disconnect, never Eject**, and is claimed by VOLUME ID (`isServerPlaceRow`), ❌ never by
  `category === 'network'`: a mounted SMB share is one, and `disconnectPlace` can't speak its OS mount.
- **`wordEjectRefusal(e)` words every eject refusal** from `errors.eject.*`; ❌ never `String(e)` or `diskutil` stderr.
- **The Network group's rows are the LISTING's**, filtered by `belongsInSwitcher`, plus the hub this dir synthesizes.
  ❗ No `listSavedServers()` fetch in `volume-grouping.ts`; the row already carries `pinned`.
- **Favorites live in their OWN menu (⌃D), ❌ never in the switcher.** `volume-grouping.ts` groups the `favorite`
  category NOWHERE; the switcher's one "See N favorites" row swaps the menus in place. Mutate ONLY through the
  `$lib/tauri-commands/favorites.ts` wrappers, stripping the `fav-` prefix.
- **❗ The chip holds ONE `openMenu`**, so the two can't both be up: each reports through `onOpenChange`, and
  `isHeaderMenuOpen()` is the single answer the panes suppress their keys on. ❌ No second source of truth.
- **The favorite-rename `<input>` must not leak keystrokes to the panes**: four guards hold that line, and removing any
  one reopens it.
- **❗ BOTH menus are the house `Menu`** (`$lib/ui/DETAILS.md` § Menu), which owns keys, the cursor, pointer mode, the
  submenu, drag reorder, the digit accelerators, placement, and focus. ❌ Never add a key handler, a highlight index,
  or a `getBoundingClientRect` back here; a menu's `onKey` claims only the keys that SWAP the two.

Architecture, flows, and decisions: `DETAILS.md`. Read it before any non-trivial work here.
