# Warn triage follow-ups

Frontend warns reach the prod log file and error-report bundles. A triage of the warns that used to be silent fixed its
high- and medium-severity findings; this file holds what it parked on purpose, plus items later sweeps parked here (the
eject close-out added three). Each item stands alone with its own severity (low unless it says otherwise), size (S under
30 lines, M under 150, L more), and the file to start from. Line numbers drift; match by content. Fix each with a red
test first.

## 1. macOS notification permission can't be read

**Problem**: per the triage (not re-verified), the notification plugin hardcodes Granted on desktop and drops delivery
results. `apps/desktop/src/lib/notifications/macos-notification-permission.ts` still asks the plugin, so the
`notifications.permissionDenied` toast is unreachable. In dev builds the plugin posts as Terminal, so dev QA of
notifications isn't representative.

**Impact**: low to medium. Someone who turns Cmdr's notifications off gets no download or low-disk banners, and Cmdr
never says so.

**Solution**: (a) a Cmdr Rust command that reads the real authorization state (first check which Apple API the plugin
posts through), or (b) delete the unreachable toast and its copy. Lean: (a).

**Size**: M to L for (a), S for (b). David decision on (a) versus (b). Relates to item 10.

## 2. Rebinding the "go to latest download" hotkey to a taken combo leaves no hotkey, and shows raw plugin English

**Problem**: in `apps/desktop/src-tauri/src/downloads/global_shortcut.rs`, `register` unregisters the working combo
before registering the new one, and its error arm only logs, so a refused combo leaves no hotkey at all. The refusal
also reaches the user as the plugin's raw English ("Unable to register hotkey: RegisterEventHotKey failed for KeyJ")
inside a translated sentence, logged three times. `RegistrationError` is still
`InvalidBinding | PluginError { message }`, while the `Registrar` trait doc and
`apps/desktop/src-tauri/src/downloads/CLAUDE.md` describe a `Conflict` variant that doesn't exist.

**Impact**: low. A user who picks a taken combo loses the feature silently and reads an untranslated error.

**Solution**: restore the previous combo on refusal. Split `RegistrationError::PluginError` by the plugin's typed
variant into an Unavailable reason ("another app may be using that combo"); keep the Rust warn, drop both frontend
warns; fix the two docs.

**Size**: M, with one new string.

## 3. The MTP device-only connect flow looks dead

**Problem**: the backend lists only connected devices' storages, hotplug auto-connects, and tab restore re-resolves
volume ids, yet `scanDevices` and `getMtpVolumes` in `apps/desktop/src/lib/mtp/mtp-store.svelte.ts` survive with no
callers outside the store and its tests. If kept, the store's connect conflates three undefined reasons and logs outside
failures at error twice.

**Impact**: none for users; dead code and a misleading "onVolumeChange callback not provided!" branch in
`MtpConnectionView.svelte`.

**Solution**: confirm nothing reaches it, then delete it, which also settles that branch. Lean: delete.

**Size**: S.

## 4. Bulk rename review: failures go unexplained, and checks time out on network volumes

**Problem**: several gaps in Ask Cmdr's rename review (`apps/desktop/src/lib/ask-cmdr/ask-cmdr-rename-review.svelte.ts`,
`apps/desktop/src/lib/tauri-commands/ask-cmdr.ts`, `ask-cmdr-trigger.svelte.ts`, `ask-cmdr-sessions.svelte.ts`,
`AskCmdrSessions.svelte`):

- `throwIpcError` turns `BulkRenameError` into `Error("rowRefused")` in the preflight, revise, and apply wrappers,
  losing what tells a bad name from a row-id or store refusal.
- A rename failing with `TimedOut`, `CouldntStart`, or `NeedsAnotherLook` leaves the review open with no word on why.
- A preflight timeout clears the spinner and leaves rows unvalidated with nothing said, and reruns on every toggle.
- Both checks get a fixed 5 s (`BULK_RENAME_PREFLIGHT_TIMEOUT` and `BULK_RENAME_APPLY_TIMEOUT` in
  `apps/desktop/src-tauri/src/commands/agent/bulk_rename.rs`), so a 500-row job on a network volume can time out every
  time.
- The sessions panel's archive and choose-thread calls have no catch, so a `main.db` hiccup logs at error and auto-sends
  a report; `commitRename` swallows with no log.
- `switchToThread` discards the pending rename review before loading the new thread, so a failed load keeps the old
  thread on screen with its review gone.

**Impact**: low to medium, the highest-impact item here. A user whose rename silently does nothing can't tell why.

**Solution**: a `BulkRenameFailure` `TypedFailure` across the three wrappers; keep the typed refusal on the review and
render a notice per variant (`ReviewExpired` keeps its notice); a per-proposal `checkFailed` flag with "Cmdr couldn't
check these files in time" and Check again, the warn behind a `LogOnceGate`; catches on the sessions calls and a log in
`commitRename`; load the new thread first, tear down after. For the budget: (a) 30 s on network volumes, or (b) no hard
cap, with progress and Cancel per the "honest progress, everything cancelable" principle. Lean: (b). New copy goes in
`apps/desktop/src/lib/intl/messages/en/askCmdr.json`.

**Size**: L (about 200 lines, plus copy). David decision on the budget.

## 5. A local AI download failure during onboarding goes unseen

**Problem**: no listener is up yet during onboarding, so a failed local model download only logs (`LocalDownloadEnd` in
`apps/desktop/src/lib/onboarding/StepAi.svelte`). The only clue is the Download button reappearing in Settings > AI >
Local.

**Impact**: low. The user thinks the local model is ready when it isn't.

**Solution**: a toast after the wizard closes, with a button to that settings section. Lean: yes.

**Size**: S to M, one new string.

## 6. Suggested ops: Approve is enabled before a group's rows load

**Problem**: Approve stays enabled while an expanded group's rows are loading or failed to load; the count comes from
the store. `apps/desktop/src/lib/suggested-ops/DETAILS.md` records it as an open product question.

**Impact**: low. A user can approve rows they haven't seen, though the count is right.

**Solution**: (a) keep it, or (b) disable Approve until the first window loads. Lean: (a).

**Size**: S. David decision.

## 7. The appearance settings link does nothing on some Linux desktops

**Problem**: `openAppearanceSettings` in `apps/desktop/src/lib/tauri-commands/storage.ts` only logs when none of
gnome-control-center, systemsettings, or xfce4-appearance-settings exists, so the link and the system swatch do nothing.

**Impact**: low; Linux only.

**Solution**: (a) an info toast naming what to open, (b) hide both behind a capability probe, or (c) leave it. Lean: (c)
until Linux gets attention.

**Size**: S for (a) plus copy, M for (b). David decision.

## 8. MCP refuses consent settings in both directions

**Problem**: `mcpSettable: false` blocks an MCP client from turning usage stats, crash reports, or error reports on AND
off, plus the terms-acceptance fields and the held consent revoke
(`apps/desktop/src/lib/settings/definitions/updates-privacy.ts`, `advanced.ts`). Also left settable on purpose:
`analytics.email` (an address) and `onboarding.fullDiskAccessChoice` (an OS permission answer).

**Impact**: low. An AI client can't help a user opt OUT.

**Solution**: allow "off" over MCP and block only "on". Marking either of the two settable ones is one line.

**Size**: S. David decision.

## 9. File viewer: tail, reload, and copy failures go unexplained

**Problem**: `blocking_viewer_op` and its three commands (`viewer_set_tail_mode`, `viewer_reload`, `viewer_set_encoding`
in `apps/desktop/src-tauri/src/commands/file_viewer.rs`) flatten `ViewerError` into a string. On top of that:

- `set_tail_mode` (`apps/desktop/src-tauri/src/file_viewer/session.rs`) stores the flag, then stats the file; on a
  stalled mount the stat outlasts the op timeout, the frontend flips the switch back off, and the backend stays in tail
  mode.
- Reloading a file that was deleted, replaced, or lost its permission dismisses the toast (`ViewerReloadToastContent`,
  in `finally`) with no message and no retry.
- A successful reload doesn't clear the line cache or refetch the way the tail-on path does, so a small file's appended
  lines may never show (likely; confirm in the app).
- The unknown-size copy band in `viewer-copy.svelte.ts` drops read failures with no toast and no log.

**Impact**: low. The viewer lies about its state or says nothing when something goes wrong.

**Solution**: return `ViewerError` with the timeout as `TimedOut` and regenerate bindings; in `set_tail_mode`, store and
return, and run the catch-up stat on a detached thread; keep the reload toast up and swap its text by typed kind; an
`onReloaded` callback from the page; one `reportRangeFailure` helper for all three copy bands.

**Size**: L (about 200 lines, plus copy).

## 10. Notification send failures escape as unhandled rejections

**Problem**: `sendNotification` can't throw into the catches around it in
`apps/desktop/src/lib/downloads/event-bridge.svelte.ts` and
`apps/desktop/src/lib/low-disk-space/event-bridge.svelte.ts`: the plugin fires the invoke in a discarded async function,
so an IPC rejection escapes as an unhandled rejection at error. Separately, the downloads toast's "Stop showing these"
(`DownloadToastContent.svelte`) awaits the Settings open unguarded, so a rejection is an unhandled error and the toast
stays up; its low-disk twin already dismisses first and catches.

**Impact**: low. False error reports, and a stuck toast.

**Solution**: one awaited `sendMacosNotification` in `apps/desktop/src/lib/notifications/` over a `lib/ipc` wrapper,
behind a `LogOnceGate`; delete both dead catches. Align "Stop showing these" with the low-disk twin. Relates to item 1.

**Size**: S.

## 11. Two false failure logs

**Problem**: `handleOptInChange` and `handleAlwaysChange` in
`apps/desktop/src/lib/settings/sections/MediaIndexNetworkVolumes.svelte` swallow the rethrow, so both `.catch` warns
never run. And `getWorker` in `apps/desktop/src/lib/font-metrics/worker-client.ts` never feature-detects
`OffscreenCanvas`, so a WebView without it (WebKitGTK on Linux) logs "Measuring worker failed" on every launch, once per
in-flight job.

**Impact**: none for users; log noise that hides real failures.

**Solution**: delete both dead catches (`network-volume-prefs.ts` is already the one log site). Check `Worker` and
`OffscreenCanvas` first, log once at info, keep the warn for real failures.

**Size**: S.

## 12. Media-index preference rollbacks can revert a newer toggle

**Problem**: all four media-index prefs setters in `apps/desktop/src/lib/media-index/network-volume-prefs.ts` restore a
whole-array snapshot on failure, which can revert a toggle that landed meanwhile; and the exclusion rollback undoes the
persisted value while the live veto stays applied.

**Impact**: low. A setting can flip back on its own after an unrelated failure.

**Solution**: one shared persist-then-apply helper.

**Size**: S to M.

## 13. Two warns that should be errors

**Problem**: `apps/desktop/src/lib/text-size.svelte.ts` "Scale-change listener threw" and
`apps/desktop/src/lib/file-explorer/network/smb-reconnect-manager.svelte.ts` "Reconnect success callback threw" log at
warn, although each catches only a synchronous throw from Cmdr's own callbacks.

**Impact**: low. Real bugs in our own code don't reach error reports.

**Solution**: promote both to error (keep `text-size`'s `LogOnceGate`). ❌ Don't bulk-promote the ~30 warns that fire
only on a broken Tauri bridge: one incident would fan out into many auto-sent reports.

**Size**: S.

## 14. The viewer's restricted-settings allowlist has three hand-kept mirrors

**Problem**: the allowlist lives in the Rust `setting_id` strings (`apps/desktop/src-tauri/src/commands/settings.rs`),
`RESTRICTED_PERSISTABLE_SETTINGS` (`settings-store.ts`), and `PERSIST_ALLOWLIST` (`restricted-settings-bridge.ts`), with
no parity test. And `apps/desktop/src-tauri/capabilities/viewer.json` grants `core:event:default` (emit included),
although viewer code never emits.

**Impact**: low, security hardening. The bridge's allowlist already contains the damage.

**Solution**: derive the allowlist from one list, pin the Rust ids with a test, and narrow the capability to listen and
unlisten after checking nothing viewer-side emits indirectly. Keep `restricted-settings-bridge.ts`'s refusal at warn:
hostile viewer content can trigger it.

**Size**: S.

## 15. SMB Keychain failures may read as a protocol error

**Problem**: the SMB credential wrappers in `apps/desktop/src/lib/tauri-commands/networking.ts` still go through
`throwIpcError`. From reading `PlacesBrowser.svelte` (not tested), a Keychain refusal after a successful share listing
may read as `protocol_error`.

**Impact**: low. A misleading message on a Keychain problem.

**Solution**: the `KeychainFailure` carrier the SFTP and WebDAV wrappers already use.

**Size**: S.

## 16. A saved place whose server was forgotten reads as "unreachable"

**Problem**: `no_such_server` maps to `unreachable` in the pane (`apps/desktop/src/lib/servers/connect-flow.ts`;
`apps/desktop/src/lib/servers/DETAILS.md` notes it).

**Impact**: low. The user checks their network when the real fix is re-adding the server.

**Solution**: a dedicated "this server isn't saved anymore" sentence, which means a new string in every locale.

**Size**: S.

## 17. Saved-place install race (theoretical)

**Problem**: two installs interleaving between `get` and `register` on different threads could replace the first volume
without `on_superseded` (`register` in `apps/desktop/src-tauri/src/file_system/volume/manager.rs` returns `()`;
`on_superseded` runs from a separate read in `network/connect_wiring.rs`). No await point, so no test can force it.

**Impact**: low. A superseded volume's cleanup could be skipped.

**Solution**: `register` hands back the displaced volume under its lock.

**Size**: S.

## 18. Update download and install failures log at error

**Problem**: `apps/desktop/src/lib/updates/updater.svelte.ts` logs download and install failures at error, although a
download can fail from network trouble, and a frontend error auto-sends a report for opted-in people. Only the check
phase routes through `serverRequestLogLevel`. The doc comment there argues for error level on purpose.

**Impact**: low. Error reports from ordinary network trouble.

**Solution**: route both through `apps/desktop/src/lib/error-messages/server-request.ts`'s level helper, as the update
check does, after weighing the doc comment's argument.

**Size**: S. David decision (the current level is deliberate).

## 19. Volume wrappers swallow every failure into a fallback

**Problem**: `listVolumes`, `getDefaultVolumeId`, `resolvePathVolume`, `resolveLocation`, and `getVolumeSpace` in
`apps/desktop/src/lib/tauri-commands/storage.ts` still have bare `catch {}` fallbacks from when these commands were
macOS-only. Every platform registers them now, so a real failure hides as an empty or default answer. The FDA and
privacy-settings wrappers beside them already lost theirs.

**Impact**: low to medium. Real volume failures are invisible.

**Solution**: remove the fallbacks, checking each caller first (wider blast radius).

**Size**: M.

## 20. "Delete model" can claim success when its prune failed

**Problem**: deleting the CLIP model still does `writer.prune_all_clip().unwrap_or(0)`
(`crates/cmdr-index/src/media_index/scheduler/mod.rs`), although the media writer reports the failure.

**Impact**: low. The UI says the model's data is gone when it isn't.

**Solution**: surface it like the reclaim path's typed `PruneFailure`.

**Size**: S.

## 21. An unreadable `media.db` reads as "already cleared"

**Problem**: on a reclaim, `stored_coverage` (`crates/cmdr-index/src/media_index/scheduler/reclaim.rs`) turns an
unreadable store into "nothing doomed" via `.ok()` and `.unwrap_or_default()`.

**Impact**: low. The user is told there's nothing to delete when Cmdr couldn't look.

**Solution**: a typed unreadable answer that shows the existing `couldNotDelete` toast.

**Size**: S.

## 22. The excluded-folder purge maps the mount root byte-exactly

**Problem**: `os_folder_to_index_prefix` (`crates/cmdr-index/src/media_index/network/fetch.rs`) strips the mount root
with a plain `strip_prefix`, without the case and Unicode folding `path_is_within` applies.

**Impact**: low, disk space only. An exclusion whose mount-root part is spelled in another case or normalization form
can't be purged; reads and the enrichment veto still hide and block those images.

**Solution**: apply the same folding as `path_is_within`.

**Size**: S.

## 23. A leftover plaintext OpenAI key in `settings.json`

**Problem**: the legacy key migration was deleted past its removal date, so anyone who skipped every release that ran it
keeps an unused `ai.openaiApiKey` in `settings.json`.

**Impact**: low. A secret sits in plaintext, unused.

**Solution**: delete the key on launch without reading it.

**Size**: S.

## 24. Onboarding's beta step ticks a checklist row even when its link never opened

**Problem**: `openAndTick` in `apps/desktop/src/lib/onboarding/StepBeta.svelte` starts the tick timer even when
`openExternalUrl` rejects.

**Impact**: low. The checklist claims a step the user never saw.

**Solution**: tick only after the open resolves.

**Size**: S.

## 25. Small stale states in the Ask Cmdr rail and provider setup

**Problem**: not re-verified one by one: the Ask Cmdr cost footer can show the previous thread's cost after a switch;
`ProviderSetupController` shows a stale provider's key error under the newly picked one; the wake indicator seed can
overwrite a newer event.

**Impact**: low. Briefly wrong information on screen.

**Solution**: reset the footer and the key error on switch; make the indicator seed lose to any newer event.

**Size**: S each.

## 26. Recents log noise, and a recent recorded for a refused navigation

**Problem**: not re-verified: Search and Selection recents log identical lines under one category, and go-to-path
records a recent even when navigation was refused.

**Impact**: low. Confusing logs, and a recents list offering a path that didn't work.

**Solution**: distinct log categories; record the recent only after navigation succeeds.

**Size**: S.

## 27. Renaming an indexed drive leaves the index on its old path

**Problem**: the index manager's `volume_root` (`crates/cmdr-index/src/indexing/lifecycle/manager.rs`) is set once at
construction and goes stale after a live rename.

**Impact**: low to medium. Listings of the old path fail and the index stops updating until Cmdr restarts. Nothing is
deleted, because the delete gates see an incomplete listing.

**Solution**: follow the rename (or restart the manager for that volume) wherever `volume_root` and everything built
from it are used.

**Size**: M.

## 28. On external drives, the importance `last_used` sample looks up boot-disk paths

**Problem**: `DirTree::path_at_into` yields index-relative paths, so the Spotlight lookup in
`crates/cmdr-index/src/importance/scheduler/` asks about a Mac path instead of the mount's; external volumes still get
`last_used_available`.

**Impact**: low, no drive-safety impact. The `last_used` signal is noise on external drives.

**Solution**: join the mount root when building the sampled paths, or turn the signal off for volumes where it can't
work.

**Size**: S.

## 29. Two catalogs use a term their own glossary retired

**Problem**: in `zh`, `indexing.staleDialog.body`, `staleDialog.bodyPhone`, and `firstConnect.body` say `目录大小` where
the rest of `indexing.json` says `文件夹大小`, the form the `zh` glossary settles on. In `pt`,
`indexing.rescan.incompletePreviousScan` says `análise`, which that glossary reserves for a transfer's pre-count; a
drive scan is `varredura`.

**Impact**: low, cosmetic. Inconsistent terms for the same thing.

**Solution**: one consistency pass per language, catalog and glossary together, by a translator agent
(`docs/guides/i18n-translation.md`).

**Size**: S.

## 30. The in-flight ledger's test store is a singleton only some tests lock

**Problem**: the ledger's test store (`apps/desktop/src-tauri/src/file_system/write_operations/in_flight_temps.rs`) is
one `STORE` singleton behind a `SINGLE_FILE` mutex that only guard-holding tests take. A guarded cell's `init_and_sweep`
replays records that ~80 non-guarded engine tests wrote into the shared log, and deletes their in-flight temps
mid-write: `overwrite::tests::test_safe_overwrite_different_sizes` fails that way under a contended `cargo test`.
Nextest's per-process isolation hides it completely, so `pnpm check` and CI never see it.

**Impact**: medium, invisible to CI. Flaky local runs, and a fixture that can mask real failures.

**Solution**: per-test isolation of the store instead of a singleton plus a partial mutex, which is a fixture redesign.
❗ Tightening assertions is NOT the fix: six cells were already tightened (in `2c9870162`) and the engine test still
failed, because its temp is deleted out from under it. ❌ It can't be validated by a lane that's already green;
reproduce under plain `cargo test` first.

**Size**: M.

## 31. A failed restore in `safe_overwrite_dir` can misjudge what's at the destination

**Problem**: in `apps/desktop/src-tauri/src/file_system/write_operations/overwrite.rs`, when `safe_overwrite_dir`'s
materialize step fails, the cleanup is `if dest.exists() { remove it }` followed by a plain `fs::rename(aside, dest)`.
`exists()` collapses every stat error (and a dangling symlink) into "nothing there", and the restore bypasses
`rename_no_replace`. The primitive itself (`rename_no_replace`, now over `volume::rename_local_exclusive`) and the
`land_temp` restore are already fixed: only a `NotFound` counts as a free name there.

**Impact**: medium, data safety. Re-derive before fixing: a plain rename of a directory can't replace a non-empty
directory, so the likely worst case is a failed restore that leaves the aside for the launch sweep, but the arm is the
one place in the file still deciding "free" from a lossy check. It's reached on smbfs and FUSE, where `RENAME_EXCL` is
unsupported and stats fail oddly (FAT32 and exFAT support the flag, probed on macOS 27.0).

**Solution**: `match` on the stat's error kind, propagate anything that isn't `NotFound`, and restore through
`rename_no_replace` like the other two sites. ❌ Red test first: a stat that fails with something other than `NotFound`,
which needs a permission wall or an injected seam rather than a real dying mount.

**Size**: S to M.

## 32. An eject's holder scan can run a code-signing query against the drive it's ejecting

**Problem**: `target_devices` in `apps/desktop/src-tauri/src/file_system/volume/eject/holders/facts.rs` builds rule 5's
device list with `filter_map(root_device)`, so a mount root whose `stat` fails is silently dropped. A holder whose
executable lives on that mount then reads as "not on the drive" (`owns_executable` also answers `false` on an empty
list), rule 5's short-circuit is skipped, and `is_platform_binary` runs `SecCodeCopyGuestWithAttributes` against a
binary on the volume being torn down.

**Impact**: low to medium, narrow (needs a `stat` failure on a still-listed mount root). That multi-second read puts
Cmdr itself into the kernel's holder list, which is the exact thing the guard exists to prevent.

**Solution**: keep unreadable roots rather than dropping them, and skip the signing query whenever any target device is
unknown. Add a test that distinguishes "not on the drive" from "couldn't tell"; no existing pin can.

**Size**: S.

## 33. The favorites add-gate tests fail on macOS, and CI can't see it

**Problem**: `commands::favorites::add_gate_tests::an_ordinary_local_folder_can_be_favorited` and
`an_archive_inner_path_cannot_be_favorited` fail `assert!(path_can_be_favorited(…).await)` on an ordinary temp dir,
taking ~5.6 s each, at every load level (measured five times on 2026-09-16). CI runs only on Ubuntu and the gate is
macOS-only. Two candidate causes, neither confirmed: (a) `path_can_be_favorited`
(`apps/desktop/src-tauri/src/commands/favorites.rs`) reads only `.volume` from the path-volume resolution and drops its
`timed_out` flag, so a resolve that couldn't answer inside `VOLUME_TIMEOUT` (2 s) reads as "no volume contains this
path"; (b) the tests' `ensure_root_volume` uses `register_if_absent` on the process-wide `VolumeManager`, so a sibling
test that registered `root` first decides the outcome. The ~5.6 s doesn't match one 2 s timeout, so measure first.

**Impact**: medium. If (a), "Add to favorites" can silently refuse an ordinary folder for a real user.

**Solution**: print `timed_out` and the resolved id in both tests under `cargo nextest`, with `--test-threads 1` and
without, and compare; the fix follows the diagnosis. ❌ Don't relax the assertion.

**Size**: S to diagnose.
