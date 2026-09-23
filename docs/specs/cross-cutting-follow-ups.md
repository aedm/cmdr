# Cross-cutting follow-ups

Unrelated findings that surfaced while the Android-over-ADB work shipped, each verified against the code (2026-09-23).
They touch the transfer dialog, the drive index, the test suites, the agent's suggestions, SMB, and the build. ADB's own
open items are `later/adb-follow-ups.md`. Each item below stands alone.

## 1. The copy/move dialog's Confirm stays pressable under a "doesn't accept files" notice

- **Problem**: in the F5/F6 dialog, when the destination folder takes no writes, a red notice ("This folder doesn't
  accept new files", or the read-only / no-permission variants) shows under the path box, but Confirm stays enabled.
  Pressing it starts the transfer, which refuses at once with the same reason in an error dialog. Confirm is disabled
  only for path errors. The comment in `apps/desktop/src/lib/file-operations/transfer/TransferDialog.svelte` (above
  `REFUSAL_KEY`) records keeping it enabled as deliberate, because the transfer asks again before writing.
- **Impact**: a button that always leads to a refusal is a dead end with an extra dialog. Red already means "blocked" in
  this dialog, since path errors disable Confirm.
- **Solution**: disable Confirm while the refusal shows, with the notice as the reason. Keeping it enabled doesn't help
  even when the check is stale: the backend asks the same question before writing and refuses anyway. MCP auto-confirm
  is unaffected, since it meets the backend's typed refusal either way.
- **Size**: S, about 10 lines. **Needs a David decision**: it reverses a recorded choice. Recommendation: disable.

## 2. The index registry lock is held while a macOS file watcher starts

- **Problem**: the first time a drive's index walk starts (or a search walks unindexed folders, or a scan is stopped),
  Cmdr starts a macOS file watcher while holding the index's global registry lock
  (`crates/cmdr-index/src/indexing/lifecycle/state.rs::INDEX_REGISTRY`; the watch is armed from
  `lifecycle/manager/start.rs`). Starting the watcher waits on `fseventsd`, and the same critical section reads the
  database twice, which the module's own docs forbid.
- **Impact**: everything that asks about any index waits meanwhile: status, badges, MCP, and the check run after each
  navigation. Always on background threads, never the UI thread. In practice it happens about once per drive per
  session and took 24–63 ms in a week of David's logs (2026-09-11); it only reaches seconds on a machine busy with
  builds, which is how the tests exposed it. Enough waiters could in theory stall every backend call.
- **Solution**: decide under the lock, start the watcher outside it, then re-lock and install it only if nothing
  changed meanwhile. The hard part is the races: two walks both starting a watcher, or a teardown landing in between.
  Testing it needs a fake drive watcher, which is also what item 3 needs, so do them together.
- **Size**: M, about 150–250 lines, medium risk. Not urgent.

## 3. The index phase tests are the biggest source of retried Rust test runs

- **Problem**: 39 index phase tests start real macOS watchers and wait up to 30 s, under an 8 s test limit.
- **Impact**: from 2026-09-01 to 2026-09-11 the Rust tests lane had 178 clean runs (68 s on average) and 157 "warning"
  runs where a test failed, passed on retry, and made the run take 164 s. These tests were named in 109 of them, up to
  25 a day. None caused a false red, so the cost is noise plus about 1.5 minutes of retries on roughly half the runs.
  (Four trash tests also failed twice ever; a five-line limit bump covers them.)
- **Solution**: (a) a 20 s limit for these tests, five lines, a band-aid; or (b) a fake drive watcher so the tests stop
  depending on `fseventsd`, shared with item 2. Recommendation: (b).
- **Size**: (a) S; (b) M, 150–300 lines.

## 4. The git watcher restarts the macOS event stream once per watched path

- **Problem**: `crates/cmdr-git/src/watcher.rs` watches four paths per repo through `notify`, and each added path
  restarts the macOS event stream.
- **Impact**: the git chip appears 0.1–0.5 s late the first time a pane enters a repo, and under heavy load it can miss
  its 2 s budget and not show at all. The same test was named in 78 retried Rust-lane runs from 2026-09-06 to
  2026-09-11.
- **Solution**: one stream for all four paths, through the vendored `crates/fsevent-stream`.
- **Size**: M, 150–250 lines.

## 5. App path schemes live in `ids.rs` instead of `remote_paths.rs`

- **Problem**: about 125 lines of `crates/cmdr-fs/src/volume/ids.rs` build and parse app path schemes (`adb://`,
  `sftp://`, `webdav://`: `sftp_app_root`, `webdav_app_root`, `server_of_path`, and friends), a different concept from
  volume ids.
- **Impact**: `ids.rs` is harder to navigate, and the path grammar is split across two modules, so the next scheme's
  author may not find both halves.
- **Solution**: move that block into `crates/cmdr-fs/src/volume/remote_paths.rs`, which already owns "the two spellings
  of a remote path", keeping the re-exports so callers don't change.
- **Size**: S, a mechanical move.

## 6. The E2E doc sits at the `CLAUDE.md` word cap

- **Problem**: `apps/desktop/test/e2e-playwright/CLAUDE.md` is 599 words, one below the `claude-md-length` failure
  threshold and well past the 300–400 target, mostly because of incident stories.
- **Impact**: the next must-know added there fails the check, and every agent touching E2E pays the extra tokens.
- **Solution**: move the incident stories into the sibling `DETAILS.md`, keeping one-line guardrails, to reach about
  400–450 words.
- **Size**: S.

## 7. A copy preview from an unplugged phone offers a Retry that never works

- **Problem**: the copy preview treats a volume it doesn't know as a local disk, walks the literal `adb://…` path, and
  fails with "We couldn't finish measuring the source" plus a Retry that can't succeed. A phone that was listed but
  never dialed already gets the typed "Not connected yet" (`crate::unregistered_volumes::why_unregistered`); an
  UNPLUGGED one is no longer listed, so it falls through to the generic path.
- **Impact**: low. The only way there is a search-results pane over a phone's index after unplugging the phone, then F5
  or F8 (which starts the same preview). Confirming afterwards says "Source volume not found".
- **Solution**: the preview refuses an unknown volume with a typed reason, shows the "Not connected yet"-style copy, and
  hides Retry.
- **Size**: S, about 70 lines plus two tests, low risk. Clear win.

## 8. An approved agent suggestion that refuses to start vanishes silently

- **Problem**: when an approved suggestion's operation refuses to start, the suggestion is already marked approved, so
  it leaves the list with nothing running and no message. True for every operation type. Two cases make it concrete:
  bulk rename's start path answers plain English strings
  (`apps/desktop/src-tauri/src/file_system/write_operations/rename/bulk.rs`, `Result<_, String>`), which the dialog only
  logs; and approving a suggestion about a phone or server that isn't connected answers `SourceVolumeGone`
  (`apps/desktop/src-tauri/src/commands/agent/suggested_ops.rs`), which calls it "gone" and which the frontend doesn't
  handle.
- **Impact**: real but uncommon. You approve a suggestion about a NAS share or phone after it went away, the click does
  nothing, and nothing says why.
- **Solution**: (a) type bulk rename's start errors, about 30 lines; (b) classify the unconnected volume with the
  existing not-connected helper and show the refusal in the suggestions dialog, about 55 lines; (c) give the approval
  back, or show why the operation didn't start, for every type.
- **Size**: M overall. **Needs a David decision** on (c)'s design and on the dialog's first visible refusal copy.

## 9. "Volume not found" carries a sentence where a path belongs

- **Problem**: `apps/desktop/src-tauri/src/file_system/listing/streaming.rs` builds
  `VolumeError::NotFound(format!("Volume not found: {}", volume_id))`, but `NotFound` is defined to carry a PATH. And
  `listing/streaming_test.rs` asserts on the message with `msg.contains("Volume not found")`, which the repo's
  no-string-matching rule forbids.
- **Impact**: almost none for users: it only happens in an unmount race, the pane already recovers, and the English
  shows only under technical details. The test is a latent rule violation.
- **Solution**: (a) carry the path and match the variant in the test, about five lines; (b) optionally, unify the three
  shapes "gone" takes across the code, about 15 match arms plus i18n.
- **Size**: (a) S, a clear win; (b) M, a tradeoff.

## 10. A direct SMB connection can't tell a read-only share before the copy starts

- **Problem**: on a direct (`smb2`) connection, the "can I write here?" probe answers "don't know", so a read-only share
  shows no "doesn't accept files" notice in the transfer dialog. `smb2` decodes the share's access rights at connect and
  then discards them, and it doesn't support the per-folder access query at all.
- **Impact**: low to moderate: read-only media shares and guest logins are common. The copy starts and fails on the
  first write with "You don't have permission to copy files here", which is honest but late, with no data risk. The
  same share may show the notice on the macOS mount and lose it after the upgrade to direct (unverified;
  `test -w /Volumes/<share>` on a read-only share would settle it).
- **Solution**: (a) short term, keep the share-level rights `smb2` already decodes as a free early answer; (b) the
  real fix, add the per-folder access query to `smb2` and have `cmdr-smb` ask it for the nearest existing folder.
- **Size**: (a) S, about 40 lines; (b) M, 170–230 lines across both repos plus an `smb2` release. Tradeoff, leaning
  win.

## 11. The app depends on itself for tests, which links two copies into the test binary

- **Problem**: `apps/desktop/src-tauri/Cargo.toml` has `cmdr = { path = ".", features = ["testing"] }` as a
  dev-dependency, so the unit-test binary links the app twice. objc2 gives each Objective-C class fixed, unmangled
  symbol names, so `CmdrPromiseDelegate`, `QuickLookDelegate`, `CmdrDragSource`, and Tauri's embedded `Info.plist`
  collide: the linker keeps one of each and prints `ld: duplicate symbol` warnings.
- **Impact**: none today, since both copies have identical class layouts. But a test-only field on one of those classes
  would turn this into memory corruption in tests. It also bloats the ~340 MB test binary. The self-dependency's
  original reason (a benchmark) moved to `crates/cmdr-index/benches/`; its comment still defends benches, of which only
  `benches/icon_benchmarks.rs` remains, so check whether that one needs `testing`.
- **Solution**: replace the self-dependency with direct dev-dependencies that turn on `testing` for the workspace crates
  that need it (`cmdr-fs` already gets it that way), and update the comment. Resolver 2 keeps dev features out of the
  shipped build. Verify with clippy on all targets plus the Rust tests.
- **Size**: S, about 25 lines in `Cargo.toml` plus one `cfg` line, low risk. Clear win.
