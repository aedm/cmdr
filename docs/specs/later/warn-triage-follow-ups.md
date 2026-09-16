# Warn triage follow-ups

**Problem.** Frontend warns now reach the prod log file and error-report bundles. A triage of the 174 warns that used to
be silent found 21 Cmdr bugs, 17 outside failures handled poorly, and 22 unrelated bugs. The high- and medium-severity
ones are handled on the `drive-safety-and-warns` worktree (operation progress, server sign-in, AI startup and consent,
report senders and the update check, the excluded-folder privacy leak). What the triage left is low severity, parked on
purpose so the fixes didn't turn into yak shaving. Later work has since parked items here that aren't from the triage
and aren't all low severity, so read each item's own marker: the eject close-out sweep added three, one of them a
data-loss path.

**How to use this.** Each item stands alone, carries a severity (low unless noted), a size (S under 30 lines, M under
150, L more), and the file to start from. Paths are under `apps/desktop/src/` unless they start with `src-tauri/` or
`crates/`. Line numbers drift; match by content. Fix with a red test first, like the rest of the triage.

## Decisions to make first

1. **macOS notification permission can't be read** (low to medium). Per the triage (not re-verified), the notification
   plugin hardcodes Granted on desktop and drops delivery results, so someone who turns Cmdr's notifications off gets no
   download or low-disk banners and Cmdr never says so; `notifications.permissionDenied` copy is unreachable. In dev
   builds the plugin posts as Terminal, so dev QA of notifications isn't representative. Options: (a) a Cmdr Rust
   command that reads the real authorization state (M to L; first check which Apple API the plugin posts through), or
   (b) delete the unreachable denial toast and its copy (S). Lean: (a).
2. **Rebinding the "go to latest download" hotkey to a taken combo** (low). The rebind unregisters the working combo
   first, so a refused new one leaves no hotkey at all. Fix: restore the previous combo on refusal (S). Pairs with B6
   item 5 below. Lean: yes.
3. **The MTP device-only connect flow looks dead** (low, no user impact). The backend lists only connected devices'
   storages, hotplug auto-connects, tab restore re-resolves volume ids, and `getMtpVolumes` / `scanDevices` have no
   callers. If kept, `mtp-store` connect conflates three undefined reasons and logs outside failures at error twice.
   Fix: confirm nothing reaches it, then delete it (S), which also settles `MtpConnectionView.svelte`'s "onVolumeChange
   callback not provided!" branch. Lean: delete.
4. **Bulk rename's 5 s check budget** (low to medium). Ask Cmdr's rename review gives its preflight and apply checks 5
   s, so a 500-row job on a network volume can time out every time. Options: (a) 30 s on network volumes (S), or (b) no
   hard cap: progress plus Cancel, per the "honest progress, everything cancelable" principle (M). Lean: (b). Do it with
   B5 below.
5. **A genuine local AI download failure during onboarding goes unseen** (low). No listener is up yet, so only the
   Download button reappearing in Settings > AI > Local tells the person. The failure is already typed
   (`LocalDownloadEnd` in `lib/onboarding/StepAi.svelte`). Proposal: a toast after the wizard closes, with a button to
   that settings section (S to M, one new string). Lean: yes.
6. **Suggested ops Approve before rows load** (low). Approve stays enabled while an expanded group's rows are loading or
   failed to load (the count comes from the store). Options: (a) keep it (noted in `lib/suggested-ops/DETAILS.md`), or
   (b) disable it until the first window loads (S). Lean: (a).
7. **Appearance settings link on Linux desktops Cmdr has no settings app for** (low). `lib/tauri-commands/storage.ts`:
   with none of gnome-control-center, systemsettings, or xfce4-appearance-settings, the link and the system swatch do
   nothing. Options: (a) an info toast naming what to open (S plus copy), (b) hide them behind a capability probe (M),
   (c) leave it. Lean: (c) until Linux gets attention.
8. **MCP refuses consent settings in both directions** (low). `mcpSettable: false` blocks an MCP client from turning
   usage stats, crash reports, or error reports ON and OFF, plus the terms-acceptance fields and the held consent
   revoke. The alternative is allowing "off" over MCP and blocking only "on". Also left settable on purpose:
   `analytics.email` (an address) and `onboarding.fullDiskAccessChoice` (an OS permission answer); either is a one-line
   mark.

## Deferred fix batches

### B5: bulk rename review failures

Area: `lib/ask-cmdr/ask-cmdr-rename-review.svelte.ts`, `lib/tauri-commands/ask-cmdr.ts`, the review dialog,
`lib/ask-cmdr/ask-cmdr-trigger.svelte.ts`, `AskCmdrSessions.svelte`. New copy in `messages/en/askCmdr.json`. About 200
lines. The highest-impact item left here.

1. `throwIpcError` turns `BulkRenameError` into `Error("rowRefused")`, losing what tells a bad name from a row-id or
   store refusal. Fix: a `BulkRenameFailure` `TypedFailure` across the preflight, revise, and apply wrappers (S).
2. A rename failing with `TimedOut`, `CouldntStart`, or `NeedsAnotherLook` leaves the review open with no word on why
   nothing was renamed. Fix: keep the typed refusal on the review and render a notice per variant; `ReviewExpired` keeps
   its notice (M, copy).
3. A preflight timeout clears the spinner and leaves rows unvalidated with nothing said, and reruns on every toggle.
   Fix: a per-proposal `checkFailed` flag with "Cmdr couldn't check these files in time" and Check again, the warn
   behind a `LogOnceGate` (M, copy). Settle decision 4 first.
4. The sessions panel's archive and choose-thread calls have no catch, so a `main.db` hiccup logs at error and
   auto-sends a report; `commitRename` swallows with no log (S).
5. `switchToThread` cancels the pending rename review before loading the new thread, so a failed load keeps the old
   thread on screen with its review gone. Fix: load first, tear down after (S).

### B6: viewer and system notifications

Area: `routes/viewer/`, `src-tauri/src/commands/file_viewer.rs`, `src-tauri/src/file_viewer/session.rs`,
`lib/downloads/`, `lib/low-disk-space/`, `lib/notifications/`, `src-tauri/src/downloads/global_shortcut.rs`. About 300
lines; splits into viewer (1–4) and notifications (5–7).

Shared enabler: `blocking_viewer_op` and its three commands (`viewer_set_tail_mode`, `viewer_reload`,
`viewer_set_encoding`) flatten `ViewerError` into a string. Return `ViewerError` with the timeout as `TimedOut`, and
regenerate bindings.

1. `set_tail_mode` stores the flag, then stats the file. On a stalled mount the stat outlasts the op timeout, the
   frontend flips the switch back off, and the backend stays in tail mode. Fix: store and return; run the catch-up stat
   on a detached thread (S).
2. Reload on a file that was deleted, replaced, or lost its permission dismisses the toast (`ViewerReloadToastContent`,
   in `finally`) with no message and no retry. Fix: keep the toast up and swap its text by typed kind (M, copy).
3. A successful reload only calls the command and dismisses; nothing clears the line cache or refetches the way the
   tail-on path does, so a small file's appended lines may never show (likely, confirm in the app). Fix: an `onReloaded`
   callback from the page (S).
4. The unknown-size copy band in `viewer-copy.svelte.ts` drops read failures with no toast and no log. Fix: one
   `reportRangeFailure` helper for all three copy bands (S).
5. `sendNotification` can't throw into the catches in `lib/downloads/event-bridge.svelte.ts` and
   `lib/low-disk-space/event-bridge.svelte.ts`: the plugin fires the invoke in a discarded async function, so an IPC
   rejection escapes as an unhandled rejection at error. Fix: one awaited `sendMacosNotification` in
   `lib/notifications/` over a `lib/ipc` wrapper, behind a `LogOnceGate`; delete both dead catches (S). Relates to
   decision 1.
6. A hotkey combo the OS refuses shows the plugin's raw English ("Unable to register hotkey: RegisterEventHotKey failed
   for KeyJ") inside a translated sentence, logged three times. Fix: split `RegistrationError::PluginError` by the
   plugin's typed variant into an Unavailable reason ("another app may be using that combo"); keep the Rust warn, drop
   both frontend warns; fix the downloads `CLAUDE.md` and Registrar doc that describe a nonexistent Conflict variant (M,
   copy). Do decision 2 with it.
7. Downloads' "Stop showing these" awaits the Settings open unguarded, so a rejection is an unhandled error and the
   toast stays up. Align it with the low-disk twin (S).

### B7: dead catches and false failure logs

No user impact, log truth only. About 60 lines, 100 with the rider.

1. `lib/settings/sections/MediaIndexNetworkVolumes.svelte`: `handleOptInChange` and `handleAlwaysChange` swallow the
   rethrow, so both `.catch` warns never run (`network-volume-prefs.ts` is already the one log site). Fix: delete both
   catches (S).
2. `lib/font-metrics/worker-client.ts`: `getWorker` never feature-detects `OffscreenCanvas`, so a WebView without it
   (WebKitGTK on Linux per `font-metrics/DETAILS.md`) logs "Measuring worker failed" on every launch, once per in-flight
   job. Fix: check `Worker` and `OffscreenCanvas` first, log once at info, keep the warn for real failures (S).
3. Rider: all four media-index prefs setters restore a whole-array snapshot on failure, which can revert a toggle that
   landed meanwhile, and the exclusion rollback undoes the persisted value while the live veto stays applied. Fix: one
   shared persist-then-apply helper (S to M).

## Smaller items

- **Error promotions left for later.** Promote `lib/text-size.svelte.ts` "Scale-change listener threw" (keep its
  `LogOnceGate`; the listeners are Cmdr's own five view callbacks) and
  `lib/file-explorer/network/smb-reconnect-manager.svelte.ts` "Reconnect success callback threw" (one sync subscriber).
  Both catch only a synchronous throw from Cmdr's code (S). ❌ Don't bulk-promote the ~30 warns that fire only on a
  broken Tauri bridge: one incident would fan out into many auto-sent reports.
- **Viewer capability hardening** (low, security). The restricted-settings allowlist lives in three hand-kept mirrors
  (Rust `setting_id` strings, `RESTRICTED_PERSISTABLE_SETTINGS`, `PERSIST_ALLOWLIST`) with no parity test, and
  `src-tauri/capabilities/viewer.json` grants `core:event:default` (emit included) although viewer code never emits. The
  bridge's allowlist already contains the damage. Fix: derive the allowlist from one list, pin the Rust ids with a test,
  and narrow the capability to listen and unlisten after checking nothing viewer-side emits indirectly (S). Keep
  `restricted-settings-bridge.ts`'s refusal at warn: hostile viewer content can trigger it.
- **SMB Keychain wrappers** still go through `throwIpcError`. From reading `PlacesBrowser.svelte` (not tested), a
  Keychain refusal after a successful share listing may read as `protocol_error`. Fix: the `KeychainFailure` carrier the
  SFTP and WebDAV wrappers use (S).
- **A dedicated "this server isn't saved anymore" sentence.** A saved place whose server was forgotten reads as
  `unreachable` in the pane (`lib/servers/connect-flow.ts`, `no_such_server`). More honest copy needs a new string in
  every locale (S).
- **Saved-place install race** (theoretical). Two installs interleaving between `get` and `register` on different
  threads could replace the first volume without `on_superseded`. No await point, so no test can force it. Fix:
  `register` hands back the displaced volume under its lock (S).
- **Update download and install failures log at error** (`lib/updates/updater.svelte.ts`), although a download can fail
  from network trouble, and a frontend error auto-sends a report for opted-in people. Fix: route both through
  `lib/error-messages/server-request.ts`'s level helper, as the update check already does (S).
- **Stale "non-macOS" catch-alls in the volume wrappers** (low to medium). `lib/tauri-commands/storage.ts`'s
  `listVolumes`, `getDefaultVolumeId`, `resolvePathVolume`, `resolveLocation`, and `getVolumeSpace` still swallow every
  failure into a fallback, although every platform registers these commands, so a real failure hides as an empty or
  default answer. The FDA and privacy-settings wrappers beside them already lost theirs. Wider blast radius: check each
  caller before removing the fallback (M).
- **"Delete model" can claim success when its prune failed.** `delete_clip_model` still `unwrap_or(0)`s
  `prune_all_clip`, although the media writer now reports the failure. Fix: surface it like the reclaim path's typed
  `PruneFailure` (S).
- **An unreadable `media.db` reads as "already cleared"** on a reclaim: `stored_coverage` treats it as nothing doomed.
  Fix: a typed unreadable answer that shows the existing `couldNotDelete` toast (S).
- **The excluded-folder purge maps the mount root byte-exactly.** `os_folder_to_index_prefix` strips the root without
  the case and Unicode folding `path_is_within` now applies, so an exclusion whose mount-root part is spelled in another
  case or normalization form can't be purged. Reads and the enrichment veto still hide and block those images, so it's
  disk space only (S).
- **A leftover plaintext OpenAI key.** The legacy key migration was deleted past its removal date, so anyone who skipped
  every release that ran it keeps an unused `ai.openaiApiKey` in `settings.json`. Fix if it matters: delete the key on
  launch without reading it (S).
- **Small leaks and stale states** (S each): StepBeta ticks a checklist row even when its link never opened; the Ask
  Cmdr cost footer can show the previous thread's cost after a switch; `ProviderSetupController` shows a stale
  provider's key error under the newly picked one; the wake indicator seed can overwrite a newer event; Search and
  Selection recents log identical lines under one category; go-to-path records a recent even when navigation was
  refused.
- **Renaming an indexed drive leaves the index on its old path** (low to medium). The index manager's `volume_root` goes
  stale after a live rename, so listings of the old path fail and the index stops updating until Cmdr restarts. Nothing
  is deleted, because the delete gates see an incomplete listing. Start from
  `crates/cmdr-index/src/indexing/lifecycle/manager.rs` (the `volume_root` field and everything built from it).
- **On external drives, the importance `last_used` sample looks up boot-disk paths** (low, no drive-safety impact).
  `DirTree::path_at_into` yields index-relative paths, so the Spotlight lookup asks about a Mac path instead of the
  mount's, and that signal is noise there. Start from `crates/cmdr-index/src/importance/scheduler/walk.rs`, where the
  sampled paths are built.
- **Two catalogs use a term their own glossary retired** (low, cosmetic). In `zh`, `indexing.staleDialog.body`,
  `staleDialog.bodyPhone`, and `firstConnect.body` say `目录大小` where the rest of `indexing.json` says `文件夹大小`,
  which is the form the `zh` glossary settles on. In `pt`, `indexing.rescan.incompletePreviousScan` says `análise`,
  which that glossary reserves for a transfer's pre-count, where a drive scan is `varredura`. Both found while
  translating `indexing.needsFreshScan.afterDisconnect` on 2026-09-16, and left alone to keep that commit to one key.
  Fix: one consistency pass per language, catalog and glossary together (S).
- **The in-flight ledger's test store is one singleton behind a mutex only some tests take** (medium, invisible to CI).
  A guarded cell's `init_and_sweep` replays records that non-guarded engine tests wrote into the shared log, and deletes
  their in-flight temps mid-write: `overwrite::tests::test_safe_overwrite_different_sizes` fails that way under a
  contended `cargo test`, with no ledger assertion in it at all. `SINGLE_FILE` serializes guard-holders against each
  other, but around 80 engine tests write to the store without ever taking it. Nextest hides it completely, because
  per-process isolation removes the shared store, so `pnpm check` and CI have never been exposed (134/134, repeatedly).
  The hazard predates the eject work; M11 surfaced it by roughly tripling the write volume, since `track` now fires for
  a temp, an aside, and a staging dir where only `register` fired before. ❗ Tightening assertions is NOT the fix, and
  the six cells already tightened in `2c9870162` must not be read as one: `a_partial_with_no_named_path_space_...`,
  `a_local_record_stays_a_bare_path_on_disk`, both `the_sweep_refuses_a_..._that_isnt_one_of_our_scratch_files`,
  `a_leftover_on_a_removable_drive_...`, and `a_leftover_on_the_mac_stays_local_homed`. Those only stopped each cell
  asserting about the whole ledger; the engine test still failed afterwards, because its temp is deleted out from under
  it. The fix is per-test isolation of the store instead of a singleton plus a partial mutex, which is a fixture
  redesign, and ❌ it can't be validated by a lane that's already green. Start from the ledger's test fixture and
  `SINGLE_FILE` (M).
- **`rename_no_replace`'s fallback clobbers on a stat it couldn't make** (HIGH, data loss; predates the eject work,
  found by its close-out sweep). `src-tauri/src/file_system/write_operations/overwrite.rs`, the arm after the atomic
  attempt: `if fs::symlink_metadata(dest).is_ok() { AlreadyExists } else { fs::rename(temp, dest) }`. Only a `NotFound`
  means the name is free; every OTHER stat error (`EACCES`, `EIO`, a mount on its way out) takes the same branch and
  falls into a **plain `fs::rename`, which overwrites whatever is actually there**. ❗ The fallback runs exactly where
  `RENAME_EXCL` / `renameat2(RENAME_NOREPLACE)` is unsupported — FAT and exFAT sticks, FUSE and SMB mounts — so the
  filesystems that reach it are the ones most likely to answer a stat with something that isn't `NotFound`. It is also
  the primitive the aside RESTORES rest on, which is what makes the eject plan's invariant 9 ("restores go only through
  `rename_no_replace`") true on paper and not in the one case that matters. Two siblings, same shape: the
  `safe_overwrite_dir` failure arm guards its cleanup with `if dest.exists()`, which collapses every stat error (and a
  dangling symlink) to "nothing there" and then restores with a plain `fs::rename`; and the `land_temp` failure arm
  restores the aside with a plain `fs::rename` and never goes through `rename_no_replace` at all, unlike the launch
  sweep's restores. Fix: `match` on the error kind, propagate anything that isn't `NotFound`, and route both sibling
  restores through the same primitive — all three sites as ONE piece of work, since fixing the primitive alone leaves
  the two callers that bypass it. ❌ Red test first (a stat that fails with something other than `NotFound`, which needs
  a permission wall or an injected seam rather than a real dying mount) (M).
- **A holder scan can run a code-signing query against the drive it's ejecting** (low to medium; narrow).
  `src-tauri/src/file_system/volume/eject/holders/facts.rs`: `target_devices` builds rule 5's device list with
  `paths.iter().filter_map(root_device)`, so a mount root whose `stat` FAILS is silently dropped rather than noted. A
  holder whose executable lives on that mount then reads as "not on the drive" from `owns_executable` (which also
  answers `false` outright on an empty list), rule 5's short-circuit is skipped, and `is_platform_binary` runs
  `SecCodeCopyGuestWithAttributes` against a binary on the volume being torn down — the multi-second read that puts Cmdr
  itself into the kernel's holder list, which is the whole reason the guard exists. Invariant 14's guard collapsing on a
  "couldn't tell", so the same family as the mount-table and rebuild-marker collapses the effort fixed. Needs a `stat`
  failure on a still-listed mount root, on the abandonable thread, so it is narrow. Fix: keep the unreadable roots
  rather than dropping them, and skip the signing query whenever any target device is unknown; it wants a test of its
  own, since no existing pin can distinguish "not on the drive" from "couldn't tell" (S).
- **The favorites add-gate's two tests fail on this Mac at every load level, and CI can't see it** (medium, invisible to
  CI). `commands::favorites::add_gate_tests::an_ordinary_local_folder_can_be_favorited` and
  `an_archive_inner_path_cannot_be_favorited` both trip `assert!(path_can_be_favorited(…).await)` on an ordinary
  `tempfile::tempdir()`, taking ~5.6 s each to do it. Measured five times on 2026-09-16 across loads from 7 to 68, same
  outcome every run, so it is ❌ not the starvation the rest of a red `rust-tests` run is; `pnpm check` reports it under
  "Ordinary assertion or panic". CI never sees it because every runner is ubuntu and the gate is macOS-only. Two
  candidate mechanisms, neither confirmed: (a) `path_can_be_favorited` (`src-tauri/src/commands/favorites.rs`) reads
  only `.volume` off `PathVolumeResolution` and DROPS its `timed_out` flag, so a resolve that couldn't answer inside
  `VOLUME_TIMEOUT` (2 s, `commands/volumes.rs`) is indistinguishable from "no volume contains this path" and the gate
  refuses — the same "couldn't tell read as a negative" shape the eject work found four times; (b) the tests'
  `ensure_root_volume` uses `register_if_absent` against the process-wide `VolumeManager`, so a sibling test in the same
  binary that registered `root` first decides what this one resolves against, which is the ledger hazard above in
  another costume. ❗ The ~5.6 s doesn't match one 2 s timeout, so measure before believing either. Start by printing
  `timed_out` and the resolved id in the two tests, under `cargo nextest` with `--test-threads 1` and without, and
  compare (S to diagnose; the fix is whatever it turns out to be). ❌ Don't "fix" it by relaxing the assertion: a gate
  that refuses an ordinary local folder is a real refusal a person would meet as "Add to favorites did nothing".
