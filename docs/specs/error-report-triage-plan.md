# Error report triage

**The quest**: Auto-sent error reports land in `#error-reports` one by one, and nothing says whether a report is a bug
we already fixed, a bug we know about, or something new. The 2026-09-12 pass over 94 Discord-only reports found about 25
real bugs, about 13 noise sites, and one install's flood of 118, and it took a long manual session to get there. This
plan gives every error a stable signature, keeps a registry of which signatures are handled, and has an agent on the M1
box diagnose only what's new or regressed, feeding one email at 06:00 Stockholm time every day.

**What David gets**:

- **One email every morning at 06:00 Stockholm time**, quiet days included: yesterday's numbers, then a card per new or
  regressed signature with a diagnosis, a PISS, and a ready-to-paste fix prompt.
- **Discord pings only for new or regressed signatures**, plus every hand-written report as today.
- **No bookkeeping**: a fix commit names its signature in a trailer, and the release that ships it marks it fixed.
- **Floods stay cheap**: one install hammering the same error stores a handful of bundles and shows up as a count.

## Decisions (David, 2026-09-15)

- **Where the agent runs**: the M1 box (`infra/m1mbp/` in the infra repo). It's always on, and it will host other
  scheduled agents too, so the runner is generic and lives in infra.
- **Discord**: pings only for new and regressed signatures. The email carries the full picture.
- **Agent scope**: diagnosis only. Fixes happen when David pastes a fix prompt into a session.
- **Email time**: 06:00 Stockholm time, every day.

## Loud rules

- ❗ Every piece of copy here (email wording, the privacy policy text, the Discord embed) is a draft for David's review.
- ❌ Never group or classify errors by their rendered message. The signature hashes the code's template, a string
  constant from the source; a report without templates stays unsigned (AGENTS.md hard rule on string-matching).
- ❌ The triage agent never edits code, commits, or pushes, and never sets `fixed` or `ignored`. `fixed` comes only from
  a released trailer, `ignored` only from David.
- ❌ Reports gain exactly the fields in § 1 and nothing else. `error_reporter/CLAUDE.md` forbids widening what we send;
  approving this plan is the consent for these fields.
- ❗ Retention and the privacy page move in the same commit (`apps/api-server/DETAILS.md` § Data retention).
- ❗ TDD for the signature function, the classification, the caps, and the version compare.
- ❌ No `run_in_background`; foreground checks only.

## How it fits together

1. **App**: `log_error!` computes a signature per error, and the auto-dispatcher sends the window's signatures with
   counts in the upload `meta`.
2. **Worker intake**: records hits in D1, classifies each signature (new, regressed, known, or tail), pings Discord for
   new or regressed ones, and caps floods before storing the bundle.
3. **Release CI**: collects `Fixes-error:` trailers between the previous tag and the new one, and marks those signatures
   fixed in the new version.
4. **M3 to M1**: every update of local `main` on David's laptop pushes to the M1's Cmdr clone as `laptop/main`.
5. **M1 at 05:00 Stockholm**: a pre-check asks the Worker for pending signatures. When there are some, `claude -p` runs
   `/error-triage` in a worktree at `laptop/main` and posts one diagnosis per signature.
6. **Worker at 06:00 Stockholm**: builds the digest from D1 and mails it.

## 1. Signatures (desktop)

- **Shape**: `e_` plus the first 10 hex characters of SHA-256 over `target`, a NUL byte, and `template` (for example
  `e_3f9a2c1b0d`). Rust computes it, using the `sha2` dependency already in `src-tauri/Cargo.toml`; the frontend and the
  Worker never do. Pinned test vectors guard the function, since changing it re-mints every signature.
- **Rust sites**: `log_error!` gains arms that bind the format literal (`$fmt:literal`) and pass it to `report_error` as
  the template. `format!` already demands a literal, so every existing call site keeps compiling.
- **Frontend sites**: `batch_fe_logs` calls `log_error!(target, "{}", message)`, so its template is a useless `{}`.
  `FrontendLogEntry` gains `template: Option<String>`, filled in `log-bridge.ts` from LogTape's `record.rawMessage` (a
  `TemplateStringsArray` joins with `{}`). The bridge's dedupe stays keyed as it is today, and a deduplicated entry adds
  its count. An entry with no template stays unsigned.
- **Survived panics** (the courier's direct `report_error` call): the template is the panic location, `file:line`. It
  changes whenever the file shifts, so these re-mint per release and aliases absorb it (§ 2). They're rare.
- **Dispatcher**: `DebounceState` keeps `signature → { target, template, count }` beside today's first-message fields,
  capped at 20 distinct signatures per window with the rest counted as `signatureOverflow`.
- **Session skip**: a flush whose signatures all went out earlier in this session sends nothing. It's in memory only, so
  it doesn't add the persistence layer `error_reporter/DETAILS.md` § Crash-loop interaction rules out.
- **What the upload `meta` gains** (mirrored in `manifest.json`):
  - `errorSignatures`: `[{ sig, target, template, count }]`.
  - `signatureOverflow`: a count.
  - `diagId`: already inside the zip; the Worker needs it before deciding whether to store the bundle.
  - Templates are constants from Cmdr's public source and carry nothing about the user. `error_reporter/DETAILS.md` §
    What we send lists all three.
- **Known limits**:
  - A wording edit or a module move mints a new signature. It shows up once as a false "new", and the agent links it
    with `aliasOf`.
  - One log site can hide two causes (`"Couldn't copy: {error}"`). When the agent sees that, the fix is splitting the
    log site or putting the typed variant into the template.

## 2. Registry (D1, migration `0017_error_signatures.sql`)

- **`error_signatures`**: `sig` (primary key), `target`, `template`, `status`, `alias_of`, `fixed_in`, `fixed_commits`,
  `first_seen`, `last_seen`, `first_version`, `updated_at`. No identifiers; kept indefinitely.
- **`error_hits`**: `report_id`, `sig` (NULL for an unsigned report), `count`, `kind`, `app_version`, `env`,
  `install_key`, `bundle_stored`, `received_at`. Deleted after 90 days by the retention sweep.
- **`error_triage_notes`**: `id`, `sig`, `run_id`, `created_at`, `diagnosis` (JSON). History stays, so a regression
  shows the diagnosis from last time.
- **`error_triage_runs`**: `id`, `day` (Stockholm date), `started_at`, `finished_at`, `code_ref`, `code_date`,
  `diagnosed`, `carried_over`, `outcome` (`ok`, `nothing-pending`, `partial`, `failed`, or `missing`), `notified_at`.

Statuses:

- **`new`**: first seen, and nobody has looked.
- **`regressed`**: was `fixed`, and a report arrived from `fixed_in` or later.
- **`open`**: diagnosed and waiting for a fix.
- **`fixed`**: a released commit carries its trailer. Hits from versions older than `fixed_in` are **tail**: expected,
  counted apart, and shrinking as installs update.
- **`ignored`**: David's call, with a note. It should stay nearly empty, because error level means a Cmdr bug by policy
  (`error_reporter/CLAUDE.md`), so noise gets downgraded to warn and marked fixed.
- **`alias_of`**: points at the signature this one continues. Its hits roll up into that one wherever counts appear.

Details:

- **`install_key`**: HMAC-SHA-256 of `diagId` under a new Worker secret, `INSTALL_KEY_SECRET`, first 16 hex characters.
  D1 alone never holds a raw `diag_` id, and the key lives exactly as long as the bundle it came with.
- **Environments**: `dev` hits are recorded with `env = 'dev'` and left out of every count and classification.

## 3. Intake (`src/telemetry/`)

1. **Flood cap, before the R2 put**: store the bundle only while its install has stored fewer than three bundles for
   that signature today and fewer than 20 overall. Past that, record the hits with `bundle_stored = 0`, skip the put,
   and answer 200 with `amendKey: null`, which clients already read as "amending isn't available". The counters are KV
   and racy, like the other gates in `error-report-intake.ts`. Reports without `diagId` (older builds) skip this gate;
   the global caps still hold. Hand-written reports are never capped.
2. **Record and classify**, in `postUploadWork`'s own try/catch so a D1 failure never costs the upload. Per signature:
   no row inserts `new`; `fixed` with `appVersion >= fixed_in` becomes `regressed`; `fixed` with an older version is
   tail; anything else is known.
3. **Discord**: post when any signature is new or regressed, or when `kind` is `user`. The embed lists each signature
   with its class and template. Unsigned auto reports don't ping; the email counts them. The existing daily caps stay.
4. **Version compare**: a small tested helper in the Worker. A pre-release suffix sorts below its release.

## 4. Marking fixes

- **The trailer**: a fix commit ends its body with `Fixes-error: e_3f9a2c1b0d`, one line per signature. Downgrading
  noise to warn is a fix too. The convention lives in `docs/tooling/feedback-and-error-digest.md`, the doc every agent
  chasing a report reads, with a one-line pointer in `error_reporter/CLAUDE.md`.
- **Release CI**: a step in `release.yml`, after the publish succeeds, reads
  `git log --format='%(trailers:key=Fixes-error,valueonly)' <previous tag>..<new tag>` and calls
  `POST /triage/fixes { version, fixes: [{ sig, commit }] }` with `TRIAGE_API_TOKEN`, a new GitHub secret. Every listed
  signature that isn't `ignored` becomes `fixed` with `fixed_in` set to this version.
- **Idempotent**: running the step twice writes the same rows. When it fails, the release is already out; the run shows
  a warning and `docs/guides/releasing.md` gets a one-line recovery note (re-run the job).

## 5. Laptop `main` on the M1

The M1 clones from GitHub, and `origin/main` usually trails the laptop's local `main` by days of unpushed commits. An
agent reading `origin/main` would re-diagnose bugs David already fixed.

- **A `reference-transaction` hook** in the laptop's Cmdr clone watches `refs/heads/main`. On each committed update, it
  pushes in the background with `git push --quiet m1 +main:refs/remotes/laptop/main` (`ssh -o ConnectTimeout=5`) and
  stays silent when the box is unreachable. The tailnet lets David's devices reach the M1.
- **A remote-tracking ref** is never checked out, so it can't collide with a worktree any agent on the box is using, and
  every future job there can read `laptop/main` too.
- **Freshness is visible**: the job records the ref's commit and date, and the email says how old the snapshot is.
- **Home**: `.git/hooks` isn't versioned and this is machine setup, so the hook and its install script live in the infra
  repo (`laptop/`). The push goes only to David's own box, and approving this plan is the consent for it.

## 6. The daily job (M1)

### The generic runner (infra repo, `m1mbp/agent-loops/`)

- **One launchd user agent per job**, with `StartCalendarInterval` in local time. Verify the box's timezone is
  `Europe/Stockholm` first; SSH to the M1 timed out on 2026-09-15, so it's unchecked.
- **A shared `run-agent-job <job>` wrapper**: a lock so runs never overlap, a hard timeout, logs under
  `~/Library/Logs/agent-jobs/<job>/` with 30-day pruning, a healthchecks.io check per job (`/start`, success, `/fail`),
  and only that job's secrets exported. `CLAUDE_CODE_OAUTH_TOKEN` goes to the one command, per `m1mbp/CLAUDE.md`.
- **After a reboot**, user agents wait for a console login (FileVault), so the job stays down until David logs in on
  site. The Worker's email is what notices (§ 7).

### Cmdr's `error-triage` job (05:00 Stockholm, 55-minute timeout)

1. Fetch `origin --tags`, then add a detached worktree at `laptop/main` in the job's scratch directory.
2. **Pre-check**: `GET /triage/pending?limit=10`. When it's empty, record a `nothing-pending` run and exit without
   starting Claude.
3. Run `claude -p "/error-triage <run id>"` in the worktree.
4. Remove the worktree.

### The `/error-triage` command (`.claude/commands/error-triage.md`)

Versioned with the code it reads, so the snapshot and the instructions always match.

- **Scope**: up to 10 pending signatures per run, most installs first. The rest stay pending and count as carried over.
- **One read-only subagent per signature**, in parallel. Each one:
  - Downloads up to two bundles from the presigned URLs the pending endpoint returns (different installs when possible),
    then reads the manifest's breadcrumbs and `logs/cmdr.log`.
  - Finds the log site from `target` and `template` at `laptop/main`.
  - Checks for a fix already on `laptop/main` (a trailer or an obvious code change) and whether any tag contains it.
  - For `regressed`, reads the earlier notes and the fixing commit first.
  - Returns the diagnosis JSON below.
- **The lead** posts each diagnosis, which moves the signature to `open`, then finishes the run with its outcome and
  `code_ref`.
- **Never blocks on a question** (the M1 box rule): open questions go into the diagnosis summary.

Diagnosis JSON, validated by the endpoint:

- `verdict`: `bug`, `noise` (the UI already handles it, so it should log at warn), or `needs-data`.
- `confidence`: `high`, `medium`, or `low`.
- `summary`: one sentence.
- `cause`: `path:line` at `code_ref`, plus the mechanism in two to four sentences.
- `piss`: `{ problem, impact, solution, size, clearWin }`.
- `fixPrompt`: a self-contained prompt that opens with "Pls fix `e_…`" and names the trailer to add.
- `aliasOf`: set when this is an existing signature after a wording edit or a move.
- `alreadyFixed`: `{ commit, released }` when a fix exists on `laptop/main`.

### The triage API (Worker)

All behind `TRIAGE_API_TOKEN` (bearer, `constantTimeEqual`), separate from the dashboard's read-only `ADMIN_API_TOKEN`:

- `GET /triage/pending?limit=N`: pending signatures with counts, versions, installs, earlier notes, and up to two
  presigned bundle URLs each (the existing `createPresigner`), so the M1 needs no Cloudflare credential.
- `POST /triage/runs` and `POST /triage/runs/:id/finish`.
- `POST /triage/signatures/:sig/diagnosis`.
- `POST /triage/signatures/:sig/status`: David's `ignored`, reopen, or alias, through any session.
- `POST /triage/fixes`: release CI.

Token holders: the M1's sops store, the GitHub repo secrets, and David's shared store.

## 7. The daily email (Worker)

- **Timing**: a second cron expression, `0 4,5 * * *`, beside `0 */3 * * *`. The digest runs when `event.cron` is the
  new expression and the hour in `Europe/Stockholm` (via `Intl.DateTimeFormat`) is 6, so daylight saving picks the right
  UTC hour. The "every invocation" jobs gain a check on the three-hourly expression, or they'd run twice as often.
  Everything goes through `runCronJob`, so a failure alarms like the rest.
- **Recipient**: `ERROR_TRIAGE_EMAIL`, falling back to `CRASH_NOTIFICATION_EMAIL`.
- **Stamping**: `notified_at` on the day's run row, after the send (the feedback digest pattern). With no run row for
  the day, the job inserts one with outcome `missing` and sends anyway.
- **Subject** carries the numbers, for example: "Cmdr errors 2026-09-14: 2 new, 0 regressed, 14 reports from 4
  installs".
- **Body**, in order:
  1. **Yesterday's numbers** (the Stockholm day), each beside its 7-day average: reports (auto and hand-written),
     installs, new, regressed, open in total, tail hits, and unsigned reports.
  2. **A card per diagnosed signature**: class, template and target, hits and installs, versions, verdict and
     confidence, summary, cause, PISS, the fix prompt in a monospace block, and the report ids.
  3. **Carried over**: signatures still waiting, one line each.
  4. **Top five open signatures** by installs over seven days, one line each.
  5. **Health**: when the agent ran and how old its `laptop/main` snapshot was, or a line saying the triage agent didn't
     report today.
- **Rendering**: `src/email/error-triage.ts`, beside the crash and feedback emails.

## 8. Older builds

- **Unsigned reports** are counted by version only. They're never grouped by message, and auto ones don't ping. Silent
  updates retire them within a release or two.
- **No seeding**: a known bug that fires gets diagnosed once, and that diagnosis becomes its record. The 10-per-run
  limit spreads a busy first week over a few mornings.

## 9. Privacy

- **`apps/website/src/pages/privacy-policy.astro`** (draft copy for David): the error report bullet adds that we keep a
  count per error type, tied to a pseudonymous install key, for the same 90 days, and that error type names come from
  Cmdr's source code. The infrastructure bullet adds that those counts live in D1.
- **`apps/api-server/DETAILS.md` § Data retention**: `error_hits` rows are deleted after 90 days; signatures, notes, and
  runs carry no identifiers and stay.
- **Diagnoses** may quote a redacted log line or two. Bundles never get copied into D1.

## Milestones

1. **Signatures in the app** (§ 1): macro arms, the frontend template field, the per-window map, the session skip, the
   new `meta` fields, pinned vectors, and docs. About 450–650 lines. Users only send signatures after a release; the
   server side works without them.
2. **Registry and intake** (§ 2, § 3, § 9): migration, hit recording, classification, version compare, Discord gating,
   the flood cap, the retention sweep, the privacy page draft, and docs. About 800–1,000 lines.
3. **Triage API and fix marking** (§ 4, § 6 API): routes and token, the `release.yml` step, and the trailer convention.
   About 500–650 lines.
4. **Daily email** (§ 7): the cron expression and gating, the number queries, the renderer, and tests. About 600–800
   lines.
5. **The `/error-triage` command** (§ 6): the command and a dry run on the laptop against real pending data with posting
   disabled. About 150–250 lines.
6. **Infra, in the infra repo** (§ 5, § 6 runner): the laptop hook and its install script, the runner and the
   `error-triage` launchd job, `TRIAGE_API_TOKEN` in the M1's store and GitHub, and a healthchecks.io check. About
   200–300 lines, plus David for the secrets and the playbook run.

Milestones 5 and 6 can run in parallel once 3 lands. The first real email needs 2 through 6, deployed: the Worker deploy
and the remote D1 migration go out when David pushes.

## Open questions

1. **Your own installs in the numbers**: want them split out ("4 installs, 1 of them yours")? That needs your `diag_` id
   from each Mac once, stored as a Worker variable.
2. **Cap sizes**: three stored bundles per install and signature per day, 20 per install per day. Good?
3. **The Discord switch-over**: unsigned auto reports stop pinging the day Milestone 2 deploys, while most installs
   still run builds without signatures. Fine, or keep pinging unsigned ones until a signed release has been out a week?

## Later

- **A registry view** in the analytics dashboard.
- **Tell the reporter**: intake answers with `knownFixedIn`, and the toast says the next update fixes it (copy for
  David).
- **Fatal panics** (`crash_reports`) join the same registry.
- **Fix branches**: once diagnoses prove reliable, the agent prepares unmerged branches.
- **Onboarding opt-in** for auto-send, for more signal from outside installs.
