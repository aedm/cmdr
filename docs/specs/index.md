# Specs index

Spec docs and task lists for Cmdr developments, indexed so each stays discoverable. See `README.md` for what this folder
is and when it gets wiped, and `DETAILS.md` § "Wiping a shipped spec" for how. Shipped specs get wiped once their
durable intent is captured in colocated `CLAUDE.md`/`DETAILS.md` (and git history); what remains here is unfinished work
plus deferred work under `later/`.

Each spec below states the problem it solves and what finishing it costs. ❌ None of them narrates what already shipped:
that lives beside the code, and git holds the history.

Open work is moving to GitHub issues (the "Cmdr backlog" project), which is where priorities get set. Every numbered
item in a `*-follow-ups.md` file is shaped to become one issue; once an item is filed, its section here shrinks to the
issue link, and a file whose items are all filed goes away.

## Active

- [ ] `open-decisions.md` - **One product call gates a transfer change: should one unrecoverable file end the whole
      operation?** Today file 200 of 700 failing past retries abandons the other 500; carrying on needs a "finished with
      N missing" terminal report, a list of what was skipped, and partial-success journal semantics.
- [ ] `eject-and-drive-safety-follow-ups.md` - **Eject and drive safety shipped (v0.46.0); five things are left.**
      Manual QA on real sticks, SD cards, and pulled cables (every automated test uses synthetic images by rule); the
      copy for a refusal whose holders are all unclassified (needs David); and three deferred designs with revisit
      triggers: per-disk DA sessions, ejecting through DiskArbitration directly, and stopping the index on a Linux
      external unmount.
- [ ] `data-safety-hunt-follow-ups.md` - **What the transfer-engine data-safety hunt left open after its 15 findings
      were fixed.** Nine re-verified gaps: two high (a cross-FS move loses bytes written after a file's copy; a
      top-level folder symlink on a volume merges through the link), three medium (SMB single-shot upload replaces a
      name taken mid-upload; the volume folder-over-file Overwrite deletes the file first; the local folder-over-file
      prompt reads as file-vs-file), four low. Plus a second hunt over the subsystems the first never reached.
- [ ] `rollback-follow-ups.md` - **Three small history-dialog gaps left after the rollback recheck shipped.** A finished
      rollback stays badged "Rolling back" until the dialog is reopened, a reversal row isn't marked as an undo of the
      operation it reversed (`rolls_back_op_id` is stored but unread), and an operation and its reversal report item
      counts that differ by one (reproduce first).
- [ ] `elevated-file-operations.md` - **A user couldn't move root-owned files out of a folder their macOS user can't
      change, and had to finish with `sudo` (ERR-4TEMD).** Draft, not started: an out-of-process native alert
      (`[Cancel] [Skip] [Allow]`), a Cmdr-specific admin right for 24 hours (revoked on lock, sleep, and quit), and a
      tiny on-demand `SMAppService` root helper doing only the refused steps, so progress, cancel, and rollback stay in
      the engine. macOS 13+. M1 is a spike answering seven open questions.
- [ ] `error-report-triage-plan.md` - **Auto-sent error reports arrive one by one, and nothing says whether one is
      fixed, known, or new.** Plan, not started, decisions taken: stable per-error signatures, a D1 registry (new,
      regressed, open, fixed, ignored), `Fixes-error:` commit trailers marked fixed by release CI, flood caps, Discord
      only for new or regressed, an M1-box agent that diagnoses only those, and one digest email at 06:00 Stockholm
      time. Six milestones across Cmdr and infra, three open questions for David.
- [ ] `servers-hub-review-follow-ups.md` - **What the servers hub's review left open.** One decision for David (a bare
      server-absolute path typed into the transfer box is joined onto the root instead of refused), a doubled SMB host
      on a non-445 port, a missing test for disconnect telling the panes, a host-key-changed pane with no button to see
      the key, an unreachable server named by hostname, and small code and copy nits. 13 self-contained items.
- [ ] `webdav-backend-follow-ups.md` - **A self-signed NAS certificate is a dead end.** Most home NAS boxes present one,
      and the connect stops at `certificate_untrusted` with no way to trust it. Also open: Nextcloud chunked upload for
      big files, a by-hand pass on a Synology and a php-fpm Nextcloud, two small correctness edges (a proxy rewriting
      hrefs, a file where an ancestor folder should be), and full Digest auth on demand.
- [ ] `cross-cutting-follow-ups.md` - **Side findings from the ADB effort, none of them ADB's.** The copy dialog's
      Confirm stays pressable under a "no writes here" notice, the index registry lock is held while `fseventsd` starts
      a watcher, the index phase tests cause half the retried Rust runs, an approved agent suggestion that refuses to
      start vanishes silently, a direct SMB share can't warn it's read-only, and the app's self-dev-dependency links two
      copies into the test binary. 11 self-contained items.
- [ ] `mtp-follow-ups.md` - **The `cmdr-mtp` extraction has never met a real phone.** Every suite ran against the
      virtual device; the one ordering the refactor could change (storages register before the event loop starts) isn't
      settleable statically. One item: David's seven-step phone checklist, about half an hour.
- [ ] `git-portal-follow-ups.md` - **The routed `.git` portal hasn't been walked by hand.** Copy-out bytes are
      automated, but editing and deleting real files under `.git/`, deleting a repo on an external disk, and the toggle
      haven't been tried in a running app. One six-step QA item, about half an hour of David's time.
- [ ] `favorites-menu-follow-ups.md` - **The viewer's right-click menu is the last hand-rolled menu, and the house
      `Menu` has never been heard through VoiceOver** (GitHub #91 shipped the rest). Port `ViewerContextMenu.svelte`
      onto `Menu`, and run a VoiceOver pass that confirms or replaces the portaled `aria-activedescendant` focus model
      every in-app menu relies on.
- [ ] `dock-integration-follow-ups.md` - **The Dock tile menu names one command two ways, and can't reach connected
      devices.** It says "Go to folder…" where the menu bar says "Go to path…" for the same `nav.goToPath`, and it lists
      bookmarks and tabs but no phones, drives, or servers (the volume list is async, and the Dock asks synchronously).
      Both wait on a David call.
- [ ] `viewer-row-wrap-follow-ups.md` - **The viewer's IPC still calls a row a line, and three small loose ends.** Rows
      shipped, so the wire's `SeekTarget::Line` / `RangeEnd::Line` / `SearchMatch.line` now mean "row", a trap for the
      next reader. Also: a timed-out fetch keeps reading unwatched, the catch-all "Failed to read file" breaks the copy
      rules, and two selection-announcement strings await David's review.
- [ ] `rename-review-follow-ups.md` - **A big Ask Cmdr rename shows nothing until the turn ends.** A 500-file job
      streams five to 22 batches before its one review opens. Opening the review on the first batch and growing it gives
      feedback and a chance to stop early, at the price of preflighting under a user who's already reading.
- [ ] `i18n-glossaries-as-data.md` - **The translator glossaries are 54,449 lines of prose across 143 locales, so
      nothing can check the facts in them.** Proposed, not started, needs David's go-ahead: store one typed row per term
      per locale and generate the markdown, reading shipped values from the catalogs at render time so value drift
      becomes unrepresentable. Expensive (a per-locale migration); a cheaper middle option (a fixed citation format) is
      written up beside it.

## Later

Deferred future work. Unchecked by default; the folder name is the status.

### AI

- [ ] `later/ai/agent-follow-ups.md` - **What the in-app agent still owes.** Designed and unbuilt: an activity log (the
      least-paid principle), the knowledge layer (folder summaries, the walk, the preflight; the largest block), scoped
      rules, a proactivity dial, the bulk slot, moving the drive index to Caches, prompts as assets, a planner job,
      auto-apply, a navigation-intent log, evals, multi-volume identity, an `inspect_file` denylist, and two someday
      items. The decision log it cites lives in `apps/desktop/src-tauri/src/agent/DETAILS.md`.
- [ ] `later/ai/wake-loop-follow-ups.md` - **What the shipped proactive agent still owes.** Five guessed constants
      (interest thresholds, backoff, idle poll, outcome-ring size) that wait on a week of real wakes, and the rail,
      which doesn't show an approve or reject in an open thread until it reloads.
- [ ] `later/ai/bulk-rename-follow-ups.md` - **Two gaps the reviewed bulk rename left, both waiting on a real report.**
      One invented filename on a remote volume refuses the whole plan (proposal construction must never touch a live
      mount), and nothing checks a reply's coverage claim against a truncated listing; only the prompt rule does.
- [ ] `later/inspect-file-follow-ups.md` - **What Ask Cmdr's `inspect_file` still owes.** Six open items, each with its
      trigger: `find` over archive entry names, files on direct SMB / MTP / SFTP volumes (waits on a viewer `Volume`
      seam), password-protected files, OCR for scans, an unsupported codec reading as `corrupt`, and a gate for GPS.

### Indexing and search

- [ ] `later/search-arena-snapshot.md` - **Opening search still waits ~1 s on every reopen past the 30 s idle window,
      and seconds on a session's first open** (GitHub #114). The fix maps a columnar `index-{volume}.arena` in place,
      kept fresh by a journal; it costs ~271 MB disk per volume and a file format to own. Gated on David deciding the
      wait is worth it.
- [ ] `later/indexing/swap-scan-plan.md` - **A rescan of a completed local index takes ~15 minutes; a fresh parallel
      scan of the same volume takes two.** Build a fresh index into `index-{vid}.building.db` and promote it atomically
      (8.4× measured), keeping the in-place reconcile as the fallback when disk is tight or the flag is off. A durable
      `.swap` marker plus open-time recovery guarantees exactly one complete index across any crash. Not started; M0
      spikes first.
- [ ] `later/indexing/sealed-subtrees-follow-ups.md` - **A directory with a million files still costs rows, RAM, and
      resync time, even with verification guarded.** Measure whether the shipped guard already fixed enough (the gate),
      then "seal" such subtrees (keep the aggregate, drop the per-file tail) with three David decisions, then the
      frontend's approximate/unsearchable state. Items 2–3 may never be needed.
- [ ] `later/indexing/media-index-follow-ups.md` - **The image index still can't find people, caption scenes, or read
      phones.** Faces (detect/embed/cluster, then naming with a durable identity store and conservative re-attach) are
      parked until David wants to be in the loop; LLM captions are optional; MTP on-demand enrichment and an "also
      delete the index" offer on disable are small gaps. The decision log is `media_index/DETAILS.md` § "Key decisions".
- [ ] `later/indexing/drive-index-overall-eta.md` - **The indexing status shows the active step's ETA but no honest
      overall "~Xm left".** Only the scan phase has a persisted per-volume duration prior; save, compute, and replay
      have none that survive a restart. Persist those per volume, then sum the active step's ETA with the pending steps'
      seeds. Not started.
- [ ] `later/db-first-listings-plan.md` - **Serve directory listings from the SQLite index instead of `readdir` +
      `stat`**, so first paint is a query. Blocked first on a measurement: the only latency number predates
      release-build measurement, so it may have no user-visible win. Second blocker: `created` is a sort column the
      index doesn't store.
- [ ] `later/importance-follow-ups.md` - **Folder importance ranks with untuned weights.** The tuning loop is built but
      needs David's home directory; the Spotlight last-used sampler's 500-folder cap has never been measured; and a
      recompute ignores the memory watchdog and shutdown (seconds today, so low priority).
- [ ] `later/idle-cost-follow-ups.md` - **Nobody knows what Cmdr costs at idle today.** Every number dates from one
      noisy 2026-08-03 run, so a fresh quiet-machine baseline comes first. Then three CLIP memory calls (idle unload at
      677 ms cold, `MLComputeUnits` worth ~400 MB, an fp16 text tower), a rescan threshold waiting on a week of data, a
      rescan-denylist question for David, and two small API/check cleanups.

### Backends and transfers

- [ ] `later/adb-follow-ups.md` - **A phone shows up twice, and half the ADB backend is still only proven against a
      mock.** One switcher row per phone (decided: MTP face by default, ADB as a mode, matched by serial) replaces the
      "(ADB)" label. The rest of the real-device pass (authorize prompt, 2 GB transfers, `/data`, a real index walk)
      gates measuring `sendrecv_v2` compression.
- [ ] `later/sftp-follow-ups.md` - **Two SFTP gaps wait on a real user.** Free space and non-UTF-8 filenames share one
      fix (vendoring two protocol crates, a permanent maintenance cost), and `~/.ssh/config` aliases could fill the add
      form. Neither is worth doing on a hypothetical.
- [ ] `later/smb-pinned-shares.md` - **An SMB share can't be pinned, so it leaves the switcher the moment it unmounts.**
      An SFTP or WebDAV place keeps a greyed `saved` row that dials on activation; a share reaches the switcher only
      while mounted. The fix is a share-level writer at mount time spelling the server the way `statfs` does, plus the
      port, plus a `pinned` field; the design work left is which arm a `saved` SMB row dials.
- [ ] `later/transfer-queue-follow-ups.md` - **Five independent transfer-queue extensions**: more than one operation per
      lane (`LANE_BUDGET` is a const 1), reopening a stale handle after a long pause on SMB or SFTP, bounding paused
      operations' blocking threads, queue reordering, and queue persistence across restarts.
- [ ] `later/archive-follow-ups.md` - **Three independent archive gaps**: adding a file to a big zip rewrites the whole
      archive (design settled in `docs/notes/m-append-spike.md`, and the SMB server-side half is no longer blocked on
      `smb2`), a file inside an archive can't open in an external app, and editing a zip on an MTP device round-trips
      the whole archive (stretch).

### App and platform

- [ ] `later/warn-triage-follow-ups.md` - **33 small, self-contained defects the frontend warn triage and later sweeps
      parked.** Mostly low severity: unexplained failures, dead catches, and false error logs, plus eight product
      decisions. Two deserve earlier attention: `safe_overwrite_dir`'s failure arm still judges a free name by a lossy
      `exists()` (§ 31, data safety), and a favorites gate that fails its own tests on macOS (§ 33, invisible to CI).
- [ ] `later/default-file-manager-follow-ups.md` - **What "default file manager" still owes now that reveal-in-Cmdr
      ships**: folder opens (`open .`, Spotlight) landing in Cmdr through the `public.folder` handler, an onboarding
      offer for reveal, and a per-app check of which apps' "Show in Finder" actually redirects. Two need a David
      decision first.
- [ ] `later/tags-follow-ups.md` - **Two small Finder-tag gaps**: the seven tag circles show in the context menu on
      volumes that can't hold a tag (MTP, direct SMB), and a tag assigned from search results doesn't show until the
      next navigation.
- [ ] `later/i18n-screenshot-follow-ups.md` - **Catalog families translators still get no screenshot of, and why each
      resists capture**: the image-indexing panel body, the sidebar's status and error states, Ask Cmdr's tool rows (the
      fake LLM never calls a tool), a few never-visited settings pages, the long tail of trouble states, and an unread
      batch of generated screenshot notes. Live numbers come from the generated `coverage-report.md`, never this doc.
- [ ] `later/data-dir-rename-spec-draft.md` - **Plain data-directory names** (`~/Library/Application Support/cmdr/`, not
      `.../com.veszelovszki.cmdr/`). Not started, cosmetic, low value. The bundle identifier must never change (TCC and
      the updater key on it). Timebox the go/no-go first: can a prod build's `app_data_dir()` be repointed, and can
      every `tauri-plugin-store` writer follow; if not, drop it. The index move into `~/Library/Caches/` is
      `later/ai/agent-follow-ups.md` § 6.

### Linux

- [ ] `later/linux-builds-plan.md` - **A Linux release build (AppImage + .deb, x86_64 + aarch64) and a website that
      offers it.** Not started: CI builds macOS only and the site only knows DMGs. ⚠️ Building isn't supporting: three
      known Linux gaps (a file watcher that never starts, Super-bound menu accelerators, macOS-specific copy) gate the
      website button, not the artifacts.
- [ ] `later/dropbox-sync-status-linux.md` - **Cloud badges on Linux, which today are simply absent**: the IPC command
      has a non-macOS arm returning an empty map, and the whole `file_system::sync_status` module is macOS-gated. Not
      started. Holds the research (Dropbox's `~/.dropbox/command_socket` protocol, the reachable statuses without Smart
      Sync) and what a Linux arm needs: reuse the cache and the one-batch service, plus a cheap ancestor gate.
