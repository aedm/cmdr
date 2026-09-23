# Dock integration: follow-ups

The Dock integration shipped whole: the `usage.json` launch-day ledger (`apps/desktop/src-tauri/src/usage/CLAUDE.md`),
the one-time "add Cmdr to your Dock" nudge and its three PostHog events (`apps/desktop/src-tauri/src/dock/CLAUDE.md`,
`apps/desktop/src/lib/dock/CLAUDE.md`), and the Dock tile's right-click menu
(`apps/desktop/src-tauri/src/dock/menu/CLAUDE.md` + `DETAILS.md`). What's left is below.

## 1. One command, two labels: "Go to folder…" in the Dock, "Go to path…" in the menu bar

- **Problem**: the Dock tile menu's `menu.dock.goToFolder` says "Go to folder…" (Finder's name for its ⇧⌘G item, which
  David's Dock spec asked for), while the menu bar's `menu.go.goToPath` says "Go to path…". Both run the same
  `nav.goToPath` command.
- **Impact**: small. Two names for one action is a consistency wart a careful user notices, and it doubles the
  translation surface in ten locales.
- **Solution**: pick one wording and use it in both places (`apps/desktop/src/lib/intl/messages/en/menu.json`), then run
  the translator fan-out for the changed key (`docs/guides/i18n-translation.md`). If "Go to folder…" wins, check the
  palette entry and the dialog's title for the same wording.
- **Size**: S.
- **Blocked on**: a David decision on the wording.

## 2. A "Connected devices" group in the Dock tile menu

- **Problem**: the Dock menu offers bookmarks and open tabs, but not the phones, drives, and servers currently
  connected, so reaching a device from the Dock still means opening Cmdr and using the volume switcher.
- **Impact**: low to medium. A convenience for people who keep a phone or NAS connected; nobody has asked for it yet.
- **Solution**: the full volume list (`volume_listing::list_with_timeout`) is `async`, and `applicationDockMenu:` is a
  synchronous AppKit callback on the main thread, so the menu can't await it. Keep a Dock-owned cache of a cheap
  projection refreshed from `volume_broadcast::do_emit` and read it with `try_lock`, like the other two sources. The
  full plan is in `apps/desktop/src-tauri/src/dock/menu/DETAILS.md` § "Deliberately out of scope".
- **Size**: M.
- **Blocked on**: a David decision on whether it's wanted at all (it was left out of the shipped menu on purpose).
