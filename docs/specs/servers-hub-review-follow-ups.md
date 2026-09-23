# Servers hub review follow-ups

What the servers hub's pre-merge review left open, re-verified against the code (2026-09-23). The hub, the one sign-in
sheet, and the path grammar are documented in `apps/desktop/src/lib/servers/DETAILS.md`; the backend command family in
`apps/desktop/src-tauri/src/commands/DETAILS.md` § "File inventory" (the `servers.rs` entry). Each item below stands
alone.

## 1. A bare server-absolute path gets joined onto the volume root instead of refused

- **Problem**: `cmdr_fs::volume::root_anchored` (`crates/cmdr-fs/src/volume/mod.rs`) runs before the containment check
  at every app site that turns a user-typed path into an absolute one. So `/srv/data/photos` on a volume rooted at
  `sftp://ada@nas:22/srv/data` becomes `sftp://…/srv/data/srv/data/photos`, which then passes containment as a
  legal-but-wrong server path. The typed refusal in `crates/cmdr-fs/src/volume/remote_paths.rs` is correct and never
  gets the chance to fire.
- **Impact**: the guarantee rests on "the app never spells a remote path bare". The one free-text entry point is the
  transfer dialog's destination box (`apps/desktop/src-tauri/src/file_system/write_operations/routing.rs`), whose
  dialect is a leading-slash volume-relative path, indistinguishable from a server-absolute one. A user who types the
  server path they know copies into a doubled folder.
- **Solution**: (a) accept the premise explicitly and say so beside `root_anchored`; or (b) give the remote volumes
  their own anchoring that refuses a bare path rather than joining it.
- **Size**: (a) S, an afternoon; (b) M, about two days to change the seam. **Needs a David decision**: (b) costs the
  transfer box its current dialect for remote destinations.

## 2. A saved SMB host on a non-445 port shows twice in the servers hub

- **Problem**: `smb_hosts` in `apps/desktop/src-tauri/src/commands/servers.rs` dedupes on `address`, and the two sources
  spell it differently: a share row carries `share.server_name` and never a port, while a manually typed entry carries
  `host` or `host:port`.
- **Impact**: `nas.local` typed on port 4450 never matches its own share row, so the hub lists one machine twice.
- **Solution**: dedupe on the host alone, or on the id `manual_servers::generate_server_id` derives from `(host, port)`.
- **Size**: S, an hour with a test. Clear win.

## 3. Nothing asserts that disconnecting a place tells the panes

- **Problem**: the test headed "Disconnecting a place tells the panes too" in
  `apps/desktop/src-tauri/src/commands/servers_test.rs` (`disconnecting_a_place_that_has_no_session_announces_nothing`)
  asserts only the negative: no session means nothing is announced. It would pass with the `emit_volume_gone` call
  deleted.
- **Impact**: the guarantee a pane depends on (drop the session, emit `VolumeUnmounted` with the volume id, keep the row
  as `saved`) has no test, so a regression leaves a pane holding a volume id nothing answers for.
- **Solution**: a second test that registers a session, disconnects, and asserts both halves.
- **Size**: S, an hour. Clear win.

## 4. `OneShotCredentials::save_credentials` refuses forever, even after `forget()`

- **Problem**: in `apps/desktop/src-tauri/src/network/one_shot_credentials.rs` the refusal is unconditional rather than
  tied to the live offer, and a volume keeps the wrapper for its whole life.
- **Impact**: harmless today, because the only writer never seeds. But a volume dialed once with `remember: false` can
  never persist a secret again, so a future "sign in and remember on this volume" writer would fail on those volumes
  only.
- **Solution**: gate the refusal on `self.offer.lock().is_some()`, so the type says what it means.
- **Size**: S, an hour. Clear win.

## 5. The `adb` path field in Settings commits on blur only

- **Problem**: `apps/desktop/src/lib/settings/sections/AdbSection.svelte` commits the path on blur, with no Enter commit
  and no commit on unmount.
- **Impact**: typing a path and closing the Settings window, or pressing Enter and assuming it took, silently drops the
  edit, and this field decides which binary the device tracker restarts under.
- **Solution**: commit on Enter and on unmount.
- **Size**: S, an hour. **Tradeoff**: `UpdatesSection` is blur-only too, so fixing one field alone makes the house shape
  inconsistent; consider a shared text-setting commit helper for both.

## 6. The "host key changed" pane state has no button to look at the key

- **Problem**: `servers.paneState.hostKeyChangedHint` tells the user to "Disconnect, then open it again to check the
  fingerprint", which is two manual steps.
- **Impact**: the one moment the app most wants the user to look at a fingerprint is the one it makes hardest.
- **Solution**: a "Look at the key" button on that pane state that opens the host-key sheet directly. It must do what it
  says (the servers module's "no inert affordance" rule), so it opens the sheet, never "trusts" anything.
- **Size**: M, about half a day: sheet plumbing plus copy (English here; translations by the translator agent).

## 7. An unreachable server is named by its hostname, not the name the user gave it

- **Problem**: there's no `servers.paneState.unreachable` key, so the pane falls back to `servers.refusal.unreachable`
  ("Cmdr couldn't reach {host}.") plus `fileExplorer.unreachable.title`.
- **Impact**: the user named the server and gets a hostname back.
- **Solution**: a `paneState` key taking `{name}`, the way every other pane state does.
- **Size**: S, about two hours including the translator pass.

## 8. `RemoteRoot::to_app_path` doesn't check containment

- **Problem**: `crates/cmdr-fs/src/volume/remote_paths.rs::to_app_path` prefixes whatever the server answered, so an
  href above the collection becomes an app path outside the volume, which the way back then refuses.
- **Impact**: low; only reachable through a misbehaving server, and it fails closed.
- **Solution**: check containment there too and refuse (or drop the entry) the way `to_remote_path` does.
- **Size**: S, under an hour. Clear win.

## 9. WebDAV keeps its own copy of the remote-path `normalize`

- **Problem**: `crates/cmdr-webdav/src/volume/paths.rs::normalize` duplicates the one in
  `crates/cmdr-fs/src/volume/remote_paths.rs`, kept because `root_remote_path` needs it before a volume exists.
- **Impact**: two copies that can drift on `..` handling.
- **Solution**: call the exported `cmdr_fs::volume::remote_paths::normalize_remote_path` (or export the private one) and
  delete the copy.
- **Size**: S, under an hour. Clear win.

## 10. The connection-tooltips test claims a compile-time guarantee it doesn't have

- **Problem**: `apps/desktop/src/lib/file-explorer/navigation/connection-tooltips.test.ts` claims a new
  `ConnectionState` fails to typecheck, but `STATES` is an array literal annotated `ConnectionState[]`, so a seventh
  state compiles and the test keeps passing.
- **Impact**: a new connection state can ship with no tooltip.
- **Solution**: build the list from a `Record<ConnectionState, true>`, so a missing key is a type error.
- **Size**: S, under an hour. Clear win.

## 11. The ADB hint's phone twin is matched by display name

- **Problem**: `apps/desktop/src/lib/adb/should-show-adb-hint.ts` counts device rows sharing a name, treating two rows
  under one name as the MTP and ADB halves of one phone.
- **Impact**: two Pixel 7s on MTP alone suppress the "turn on USB debugging" hint for both, though neither has USB
  debugging on.
- **Solution**: pair by serial through the volume path. Related: `later/adb-follow-ups.md` § 3 (one row per phone) would
  fold the twins by serial anyway, and may make this hint's twin check moot.
- **Size**: S, under an hour. Clear win.

## 12. A tautological test on `SecretOffer`

- **Problem**: `apps/desktop/src-tauri/src/network/one_shot_credentials_test.rs` constructs
  `SecretOffer { remember: false }` and asserts `!offer.remember`.
- **Impact**: it would pass with the field's meaning inverted everywhere else, so it only costs reading time.
- **Solution**: assert the serde shape the IPC boundary needs, or drop it.
- **Size**: S, minutes. Clear win.

## 13. A copy pass over the servers and ADB strings

- **Problem**: English source strings that break the house style (`docs/style-guide.md`), all in
  `apps/desktop/src/lib/intl/messages/en/`:
  - `servers.json` `servers.pinHint.body`: straight quotes around `{command}`, where `servers.sheet.needsStoredSecret`
    uses typographic quotes for the same job.
  - `settings.json` `settings.adb.pathPlaceholder`: "Look for adb the usual way" reads as a command to the user; the
    description already says it right ("Cmdr looks for adb the usual way").
  - `settings.json` `settings.adb.install.intro`: "then press Re-check" for an on-screen button; the house verb is
    choose or select.
  - `settings.json` `settings.fileOperations.adbEnabled.description`: straight quotes around "adb", and "costs nothing
    when you have no Android tooling installed" should be "if".
  - `settings.json` `settings.summary.servers`: "you have trusted" should be "you've trusted".
  - `settings.json` `settings.behavior.serversPinHintSeen.label`: "Long Network group hint shown" is unparseable; it
    never renders in the UI but does surface in settings search.
  - `fileExplorer.json` `fileExplorer.navigation.pinRefusedToast`: "where {name} shows" should be "where {name} appears"
    (Hungarian already says it that way).
  - `fileExplorer.navigation.disconnectBusyTooltip`, `adb.disconnectBusyTooltip`, and the older
    `fileExplorer.navigation.ejectBusyTooltip`: "Can't disconnect while operations are in progress on this
    server/device" reads as backend register and lacks the terminal period every other new tooltip has.
- **Impact**: small polish, but every one is visible to users except the settings label.
- **Solution**: one pass over the English, with `@key` descriptions updated where the meaning shifts, then the
  translator agent re-translates the changed keys. The three busy tooltips change together or not at all.
- **Size**: S, about an hour plus the translator pass. David reviews all UI copy.
