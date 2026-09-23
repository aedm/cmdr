# Favorites menu: follow-ups

The favorites menu (⌃D, GitHub #91) shipped: the house `Menu` primitive (`apps/desktop/src/lib/ui/DETAILS.md` § Menu),
the volume switcher and the drive-index badge on it, and the menu itself with digits, the `0` add row, reorder, and the
switcher's "See N favorites" row (`apps/desktop/src/lib/file-explorer/navigation/DETAILS.md` § "The favorites menu
(⌃D)"). What's left is below.

## 1. Port the viewer's right-click menu onto the house `Menu`

- **Problem**: `apps/desktop/src/routes/viewer/ViewerContextMenu.svelte` is the last hand-rolled in-app menu: its own
  focus handling, arrow-key index math, outside-press and Escape handling, and positioning, all of which the house
  `Menu` (`apps/desktop/src/lib/ui/Menu.svelte` + `createMenu`) already owns. `docs/guides/building-ui.md` § Building a
  menu rules it out of native (Copy and Select all act on the viewer's offset-based selection, not a DOM one), which
  leaves the house `Menu`.
- **Impact**: low for users (it works), medium for maintenance: a second menu model drifts on keyboard behavior,
  wrap-around, reduced-transparency styling, and a11y, which is the drift the primitive exists to end.
- **Solution**: build its two rows (Copy when there's a selection, Select all) as a `MenuSection`, open with
  `menu.openAt({ x, y })`, and keep the page's Escape gate on `contextMenuPos`
  (`apps/desktop/src/routes/viewer/DETAILS.md` explains the ordering). The viewer is a separate, capability-restricted
  window, so check that nothing the primitive imports drags in a Tauri module the viewer can't have. Update
  `lib/ui/DETAILS.md` § Menu's consumer list.
- **Size**: S (about 200 lines with tests).
- **Blocked on**: nothing.

## 2. Confirm the house `Menu` reads correctly under VoiceOver

- **Problem**: the `Menu` container takes focus and marks the highlighted row with `aria-activedescendant`, on a
  container portaled to `document.body`. axe is clean (`overlays.a11y.test.ts`), but nobody has listened to it with
  VoiceOver, and `aria-activedescendant` on portaled content is a known weak spot in some screen reader and WebKit
  combinations.
- **Impact**: if it doesn't announce, every in-app menu (the volume switcher, the favorites menu, the archive Enter
  popup, the drive-index badge) is silent to VoiceOver users as the cursor moves.
- **Solution**: open each menu with VoiceOver on, arrow through rows, and note what's announced. If it's silent, move
  real focus onto the rows (roving `tabindex`) instead. Record the result, with the macOS and Cmdr versions and the
  date, in `apps/desktop/src/lib/ui/DETAILS.md` § Menu, replacing its "not yet verified" warning.
- **Size**: S to verify; M if the focus model has to change.
- **Blocked on**: a human with VoiceOver (David), or an agent-drivable screen reader check.
