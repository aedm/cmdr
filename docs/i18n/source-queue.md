# Source queue

Translators log English-side problems here while translating: ambiguous or inconsistent English, a weak `@key`
description, a missing screenshot, or a rule every language should follow. One bullet per item, at most two lines: the
key(s), what's wrong, the suggested fix. The lead fixes the English, the description, or the shared docs (promoting a
rule into `translation-principles.md` or `translator-instructions.md`), then deletes the entry; git keeps the history.

## Open

- `fileExplorer.network.browser.removeHostConfirm`, `.hostRemoved`, `.hostRemoveFailed`: descriptions (and key names)
  still say "remove" while the English says "Forget". Say forget, and that the host is a saved server.
- `fileExplorer.network.share.useGuest`: "Use guest" is terse and the description doesn't say whether it reconnects as
  guest or re-lists the shares. Consider "Switch to guest" or "Browse as guest".
- `menu.network.unpin`, `servers.pinHint.body`: the descriptions still call it a "very short label" / quote "Unpin", but
  the English is now "Unpin from switcher". Update both; "switcher" alone doesn't hit the `volume-switcher` concept
  either.
- `servers.paneState.notConnected`: "{name} isn't connected" forces agreement with `{name}` in gendered languages, and
  the description says "yet" where the English doesn't. Prefer "Not connected: {name}"-style framing.
- `servers.hub.shareAccount`, `.guestAccount`: "as {username}" has no bare equivalent in many languages; say a label
  form ("Account: sven") or an added verb is fine, and add a screenshot.
- `fileExplorer.navigation.forgetConfirmButton`: one button serves the server, share, and saved-password alerts; locales
  that clear a password with another verb (zh `清除`) get a button that doesn't match the title. Consider a separate
  key.
- `servers.hub.editPickHint`: "Select a server" pulls locales toward their file-marking verb; the English means moving
  the cursor to a row.
- Cross-language rule proposal: when English prose names a button without quotes (`servers.sheet.addAnywayHelp`,
  `servers.pinHint.body`), a locale may quote it; or quote it in English too.
- `list` concept: matches the verb "lists" (Cmdr lists …); add a verb-sense `notMatch` so locales stop needing
  exceptions.
- `go-back` concept: matches "forward" while its headword is "Go back"; split or rename.
- `fileExplorer.network.share.signIn{Title,Message}`, `fileExplorer.networkMount.signIn{Title,Message}`: the screenshot
  is the servers list, not the calm sign-in screen these strings sit on. Capture that pane (with and without the sheet).
- `fileOperations.transferProgress.stage*` (compress and archive-upload phases, and their `*Step` variants): coupled to
  `transfer-dialog.png`, whose note lists only scanning/paused/queued/finishing. Capture a compress-to-remote run
  showing the "Step 1 of 2" line, and name the compress phases in the note.
- `step` concept: its definition says onboarding, setup guide, or install; widen it to any numbered stage of a
  multi-step operation (the two-step compress-then-upload labels now use it).
- `zip` has no concept, yet it recurs in prose (`errors.write.archiveEntryName*`, `settings.archives.*`). Register it so
  each locale's prose form is ruled (de neuter `Zip`, sv `zip-fil`, hu `zip archívum`, zh-Hant `zip`).
- `errors.write.archiveEntryName*` messages: "some tools", "zip tools", and the verb "share" tripped `ai-tool` and
  `network-share`; `notMatch` entries now cover these, but a sense-aware matcher would stop the next one.
- `servers.hub.nearbyGroup`: no screenshot, and the description doesn't say whether a count-last shape is fine (hu, zh,
  and zh-Hant lead with "found nearby"). Recapture the hub with the group header showing, and say the order is free.
- `errors.write.destinationNotAFolder.message` leaves `{path}` bare while its one-line twin
  `errors.volume.notADirectory` quotes it (“{path}”). If the dialog styles the path itself, say so in the description;
  otherwise quote it in both.
- `pnpm i18n:brief --keys a b c` (space-separated) silently briefs only `a` (header says "1 key"). Reject stray
  positional args, or accept both separators.
- Tooling: `sync-locale-keys.ts --restamp` skips overlays (`es-419`), so a new overlay fork's `sourceHash` (the hash of
  the `es` value it overrides) had to be computed by hand with `sourceHash()` for
  `fileExplorer.pane.openLocalNetworkSettings`. Let `--restamp` (or a `--fork`) stamp overlay keys too.
- `{localNetwork}` keys (`fileExplorer.pane.directConnectionBlockedByThisMacToast`, `.openLocalNetworkSettings`,
  `servers.refusal.localNetworkHint`): no screenshot of the toast or the Add server hint. Capture both so translators
  can judge the button's width and whether the pane name reads better quoted (zh and zh-Hant quote it, the rest don't).
