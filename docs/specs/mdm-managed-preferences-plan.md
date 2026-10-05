# MDM managed preferences: IT turns off telemetry, updates, and AI for every user

Tracks the "MDM deployment" section of [#118](https://github.com/vdavid/cmdr/issues/118) (its first three checkboxes,
plus the `/trust` line). Why: an IT team rolling Cmdr out through Jamf, Kandji, or Intune needs to switch off usage
stats, crash reports, updates, and cloud AI for everyone, and see that the app honors it. "On by default" is fine once
IT can turn it off. Out of scope: the `.pkg` installer and the PPPC (Full Disk Access) profile, which have their own
checkboxes.

## Loud rules (read before every milestone)

1. **The backend enforces; the frontend only displays.** Every send, check, and AI call consults the policy in Rust at
   the point where data would leave the Mac. A bypassed UI, an MCP client, or a stale `settings.json` must change
   nothing. IPC stays a pass-through.
2. **Policy only restricts.** Every key can only turn something off or narrow it. No key can force telemetry, updates,
   or cloud AI ON. Absent, `false`, or empty-where-meaningless means "no restriction".
3. **Only FORCED values count.** A value is policy only when `CFPreferencesAppValueIsForced` says so (a configuration
   profile put it in `/Library/Managed Preferences`). A user's own `defaults write com.veszelovszki.cmdr …` must never
   show up as "Managed by your organization".
4. **A key we can't read restricts as much as that key can.** Wrong type, unparseable version, garbage host list: apply
   the most restrictive reading of that one key and `warn!` once per change. An admin who meant "off" must never get
   "on" because of a typo.
5. **The policy overlays, it never rewrites.** Nothing writes a forced value into `settings.json` or the consent record.
   Remove the profile and every user's own choices come back untouched.
6. **Fresh read at egress, cached read everywhere else.** The cache drives the UI and local bookkeeping; every path that
   sends bytes off the Mac (heartbeat, crash and error report upload, update check and download, every LLM request)
   refreshes the policy first. A profile pushed while Cmdr runs must stop the next send, not the next launch.
7. **Typed, never string-matched.** New refusals are enum variants (`BackendResolution`, `AiTranslateErrorKind`, MCP
   `data.reason`, update and report outcomes). The `error-string-match` checks apply.
8. **TDD, real red first.** The parse rules, precedence, ceiling compare, host matching, and every gate get a failing
   test before the code.
9. **i18n: English plus `@key` descriptions only.** A translator agent does the other languages later
   (`.claude/rules/translations-by-a-translator-agent.md`). Copy below is a draft for David's review.
10. **Docs single-sourced.** The key catalog lives in exactly one canonical doc: `managed_policy/DETAILS.md`. The
    `/trust` page and the sample profile are the public mirror, guarded by a test (M8).

## The preference domain and keys

Domain: **`com.veszelovszki.cmdr`**, the bundle identifier (`tauri.conf.json` → `identifier`, `config::BUNDLE_ID`).
Every MDM defaults to "custom settings for bundle id", so this is what admins expect. Managed values land in
`/Library/Managed Preferences/com.veszelovszki.cmdr.plist` (device scope) or
`/Library/Managed Preferences/<user>/com.veszelovszki.cmdr.plist` (user scope); `CFPreferencesCopyAppValue` merges both
and `CFPreferencesAppValueIsForced` tells them apart from the user's own layer. Pass the domain explicitly
(`config::BUNDLE_ID`), never `kCFPreferencesCurrentApplication`, so dev builds and tests read the same domain as release
(Edge hit exactly this with per-channel domains:
<https://learn.microsoft.com/en-us/deployedge/configure-microsoft-edge-on-mac>). Cmdr isn't sandboxed, and the hardened
runtime needs no entitlement for CFPreferences reads.

Precedent: Chromium's macOS policy loader does exactly this (`CFPreferencesAppSynchronize`, then per key
`CopyAppValue` + `AppValueIsForced`, scope from whether the device-level plist holds the key), watches
`/Library/Managed Preferences/<user>/<bundle>.plist` with a file watcher, and ALSO reloads periodically because that
path is "undocumented and therefore fragile"
(<https://chromium.googlesource.com/chromium/src/+/main/components/policy/core/common/policy_loader_mac.mm>, read
2026-10-05). Apple's own contract is one sentence: "use this function to determine whether or not to disable UI
elements" (<https://developer.apple.com/documentation/corefoundation/cfpreferencesappvalueisforced(_:_:)>).

### Value parsing (applies to every key)

- **Bools** accept `<true/>`/`<false/>`, an integer (`0` is false, anything else true), and the strings `true`, `false`,
  `yes`, `no`, `1`, `0` (case-insensitive). Admins hand-write plists, and `defaults write … Key true` without `-bool`
  stores a STRING. Anything else (a dict, `"maybe"`) is rule 4: restrictive (`true`) plus a warn. Without the string
  coercion, rule 4 would turn an admin's `<string>false</string>` into "disabled", which is safe but baffling.
- **Strings** are trimmed; empty is "absent" only where the key says so.
- An unknown key in the domain is ignored with one `debug!` (a profile written for a newer Cmdr must not break an older
  one).

### Telemetry

- **`DisableUsageStats`** (bool). `true`: no heartbeat, no events, spool and unreported uptime deleted (exactly what a
  user's own opt-out does today). Locks `analytics.enabled` to off.
- **`DisableCrashAndErrorReports`** (bool). `true`: no crash report (automatic or from the dialog), no "Attach logs"
  after a crash, no automatic error report (Flow B), no manual "Send error report" (Flow A), no amend. A pending crash
  file from the last session is discarded at launch without an offer. "Save to disk" in the error-report dialog stays
  (nothing leaves the Mac). Locks `updates.crashReports` and `updates.errorReports` to off.

One key for both report kinds because both ship diagnostics to us and an admin turning off one would always turn off the
other. Feedback is NOT covered (see Decisions).

### Updates

- **`DisableAutomaticUpdateChecks`** (bool). `true`: no background check; "Check for updates" by hand still works. Locks
  `updates.autoCheck` to off.
- **`DisableUpdates`** (bool). `true`: no check of any kind (so no request to `api.getcmdr.com/update-check`), no
  download, no install. IT ships new versions itself. Implies the previous key.
- **`MaxUpdateVersion`** (string). "Never update past this version." `"0.52"` allows anything up to the last 0.52.x
  (ceiling `< 0.53.0`); `"0.52.3"` allows up to and including 0.52.3; `"1"` allows anything below 2.0.0. A newer release
  isn't offered; the UI says one exists and that the organization holds Cmdr back. Grammar: one to three dot-separated
  non-negative integers, optional leading `v`, nothing else. An integer plist value is accepted as a major (`1` →
  `"1"`); a `<real>` is rejected (`0.5` and `0.50` are the same real, so the admin's intent is lost). Unparseable or
  empty → behaves as `DisableUpdates` (rule 4).
  - ❗ **Compare on the release core only** (`major.minor.patch`, prerelease and build metadata stripped) against the
    exclusive bound. In semver `0.53.0-rc.1 < 0.53.0`, so a plain `Version` compare against `< 0.53.0` would let a 0.53
    prerelease through a `"0.52"` ceiling. `"0.52.3"` means core `<= 0.52.3`.

Why "ceiling" and not "pin to exactly X": the updater only knows `latest.json`, which names the newest release. It can't
fetch an older one, so "pin" can only mean "don't go past". A ceiling also lets IT allow patch releases within a minor
(the security fixes) while holding the next minor for testing, which is what Chrome's `TargetVersionPrefix` is used for.

### AI

- **`DisableAI`** (bool). `true`: no AI at all. No LLM call, no local model download, no `llama-server`, no Ask Cmdr, no
  "try local AI" offer, MCP `ai_search` refuses. Locks `ai.provider` to `off` and `askCmdr.enabled` to off.
- **`DisableCloudAI`** (bool). `true`: the local model is still allowed; nothing goes to a cloud provider. The `cloud`
  provider option is disabled; the "Allow cloud AI" switch is locked off. A user whose stored provider is `cloud`
  behaves as `off` (❌ never silently switch them to `local`: that starts a multi-GB download).
- **`AllowedCloudAIHosts`** (array of strings). When present, cloud AI may only reach these hosts. Each entry is a
  hostname (`api.openai.com`, matched case-insensitively and exactly) or a `*.` suffix pattern (`*.openai.azure.com`,
  matches any subdomain, not the bare domain). An empty array means no host is allowed, which equals `DisableCloudAI`.
  Loopback endpoints (Ollama, LM Studio) are `cloud` providers in Cmdr, so they need `localhost` / `127.0.0.1` listed
  like any other host. Malformed entries are dropped with a warn; a non-array value reads as an empty list (rule 4).
  Normalization, identical on both sides (entry and request URL):
  - An entry may be a bare host, `host:port`, or a pasted URL (`https://api.openai.com/v1`): admins paste URLs, and
    dropping those would silently block the provider they meant to allow. Only host and an explicit port are used.
  - Hosts go through `url::Host::parse` (IDNA to punycode, lowercase, IPv4/IPv6 parsed), and one trailing dot is
    stripped (`api.openai.com.` is the same host). Compare `url::Host` values, never `host_str()` strings: `host_str()`
    keeps IPv6 brackets (`[::1]`, see `host_is_loopback` in `ai/connection_check.rs`) and an entry `::1` must match it.
  - An entry WITH a port matches only that port (`port_or_known_default()` on the URL side); an entry without one
    matches any port.
  - `*`, `*.`, and a pattern on a bare IP are malformed (dropped). `*.com` is legal: the admin's call.

Why hosts and not provider ids: the host is where the data actually goes, and it's what a security review asks about.
Provider ids are a frontend preset table (`cloud-providers.ts`), and the `custom` and `azure-openai` presets take any
URL, so an id allowlist would either leak (allow `custom`) or forbid a company's own LLM gateway. A host list covers a
corporate gateway (`llm.corp.example.com`) and Azure tenants (`*.openai.azure.com`) with the same rule.

### Precedence

`DisableAI` > `DisableCloudAI` > `AllowedCloudAIHosts`. `DisableUpdates` overrides the other two update keys, but
`MaxUpdateVersion` and `DisableAutomaticUpdateChecks` are ORTHOGONAL and combine (a ceiling with manual checks only is
the common IT setup), so the typed update policy must hold both at once (see M1). Then, per gate: allowed = policy
allows AND the user's own setting allows. Policy never turns a user's "off" into "on".

### Linux and other platforms

The source is macOS-only for now. Elsewhere the policy reads as "no restriction" (a `NoManagedPrefs` source), and the
docs say so. A future Linux source (for example `/etc/cmdr/policy.json`) slots in behind the same trait. The test-build
`PlistFileSource` override (§ Architecture) works on EVERY platform, so the Linux Docker E2E lane can run the policy
specs too (the `plist` crate parses XML plists anywhere).

## Fresh grep: where each gate lives today

Verified at `5c8f7cec6`; line numbers drift, names don't.

- CFPreferences precedent: `dock/prefs.rs` (`is_forced` ~:80, `to_plist` / `from_plist` ~:86–114, the scratch-domain
  test pattern `com.getcmdr.docktest.<tag>`), `glass_tint.rs` (activation observer ~:113, sync off the main thread
  ~:123), `reveal/registration.rs`. `objc2-core-foundation` already has the `CFPreferences` and `CFPropertyList`
  features (`src-tauri/Cargo.toml` ~:408). No new dependency.
- Usage stats: `analytics/mod.rs` `send_permission` (~:68, the one gate for capture AND heartbeat),
  `analytics/events.rs:35` and `analytics/heartbeat.rs:163` (`SendPermission::OptedOut` arms; the heartbeat one deletes
  the spool).
- Crash reports: `commands/crash_reporter.rs` (`check_pending_crash_report` :12, `dismiss_crash_report` :19,
  `send_crash_report` :32); the actual send is `crash_reporter/pending_delivery.rs` (`send_pending_crash_report` :36 →
  `post_crash_report` :157, the ONE function that POSTs `/crash-report`); `crash_reporter::init` (`lib.rs:284`, BEFORE
  `load_settings` at :434) runs `next_launch::process_pending_crash`; frontend decision in
  `src/lib/crash-reporter/pending-crash-report.ts`; panic courier and survival paths in
  `crash_reporter/panic_courier.rs`, `survival.rs`.
- Error reports: `error_reporter/auto_dispatcher.rs` (`set_enabled` ~:103, the Flow B master switch, seeded in
  `lib.rs:469`), `commands/error_reporter.rs` (`send_error_report` :156, `amend_error_report` :205,
  `save_error_report_to_disk` :226 stays allowed, `send_crash_log_report` :282). The actual sends:
  `error_reporter::upload` (`error_reporter/mod.rs` :532, called from all three report paths: auto-dispatcher :289, Flow
  A :166, crash log :298) and `error_reporter/auto_sent.rs` `send_amend` (:173, its own `reqwest` client).
- Shared api-server error type: `server_request.rs` `ServerRequestError` (crash, error, amend, update check), mapped to
  copy in `src/lib/error-messages/server-request.ts`.
- Updates: `updater/mod.rs` (`skip_reason` :89, `check_for_update` :109 → `Result<Option<UpdateInfo>, …>`,
  `download_update` :236, `install_update` :267), `updater/manifest.rs:47` (semver compare). ❗ `download_update` takes
  `url` and `signature` FROM THE FRONTEND and `UpdateState` stores only the tarball path, so today the backend doesn't
  know which version it staged (see M3). On Linux the frontend calls `@tauri-apps/plugin-updater` `check` directly
  (`tauri_builder.rs` `register_updater`), which bypasses all of this; frontend loop and `updates.autoCheck` in
  `src/lib/updates/updater.svelte.ts`; menu "Check for updates" → `runMenuTriggeredCheck()`.
- AI: `ai/manager.rs` (`resolve_backend` :179, `resolve_backend_inner` :299, `compute_ai_status` :114 with its `Offer`
  branch, `configure_ai` :404), `ai/server.rs:49` `start_ai_server`, `ai/install.rs:85` `start_ai_download`,
  `ai/cloud_consent.rs` (`has_current_cloud_consent` :65, already documented as "a future managed preference becomes one
  more argument here"; `CloudAiConsentStatus` :94; `revoke_cloud_ai_consent` :193 shows the cancel-everything sequence),
  `ai/connection_check.rs` (`check_ai_connection`, `validate_ai_base_url`), `ai/translate_error.rs` +
  `src/lib/ai/translate-error-toast.ts` (lockstep enums), `agent/wake/readiness.rs` (`NeedsCloudConsent`). Callers of
  `resolve_backend*`: `ai/suggestions.rs:214`, `ai/manager.rs` `resolve_translate_backend`, `agent/chat/session.rs:113`
  (Ask Cmdr: resolved ONCE per turn, then many LLM calls in the tool loop), `agent/wake/snapshot.rs:104` (background
  wakes). Every remote request leaves through one of three functions in `ai/client.rs`: `exec_chat_stream_request` :136
  (Ask Cmdr via `agent/llm/genai_impl.rs`), `chat_completion` :365, `chat_completion_stream` :478. `AiBackend::remote`
  is `pub(in crate::ai)`, so nothing outside `ai/` builds one. `ai/download.rs` fetches the local model. Provider
  presets: `src/lib/settings/cloud-providers.ts` (Azure's placeholder
  `https://{resource-name}.openai.azure.com/openai/v1` doesn't parse as a URL host).
- MCP: `mcp/executor/search.rs:304` `execute_ai_search` (typed `data.reason: "cloudAiNotAllowed"` at :296),
  `mcp/executor/async_tools.rs:536` `execute_set_setting` (round-trips `mcp-set-setting` to the frontend, no backend
  check today); frontend half in `src/lib/settings/mcp-main-bridge.ts` (`mcp-get-all-settings`, `mcp-set-setting`).
- Settings UI: `src/lib/settings/settings-store.ts` (`getSetting` :322, `setSetting` :378, `resetSetting` :473),
  `components/SettingRow.svelte` (already has `disabled` + `disabledNote` + `disabledNoteId`),
  `components/boolean-setting.svelte.ts` (`useBooleanSetting`), sections `UpdatesSection.svelte`, `AiSection.svelte`,
  `AiCloudSection.svelte`, `AskCmdrSection.svelte`; shared provider setup in `src/lib/ai-provider-setup/`;
  `src/lib/ai/AiCloudConsentToggle.svelte`; onboarding `StepAi.svelte`, `StepBeta.svelte`.
- Website: `apps/website/src/lib/trust.ts:242` (the "No central administration" gap) and :245 (the `.pkg`/PPPC gap,
  which stays), `src/pages/trust.astro:70` ("There's no central (MDM) control yet.").
- Egress NO key covers (IT will ask what's left; `/trust` must say it, see M8): license validation
  (`licensing/validation_client.rs`), the S3 price list (`s3_costs/price_source.rs`), the CLIP model download from
  Hugging Face (`crates/cmdr-index/src/media_index/clip/install.rs`), and the user-initiated feedback
  (`commands/feedback.rs`) and beta signup (`commands/beta_signup.rs`).

## Architecture

New backend module **`src-tauri/src/managed_policy/`** (named after the UI phrase "Managed by your organization"):

- `mod.rs`: `ManagedPolicy` (typed, `Default` = no restriction), `current()` (cached), `refresh()` (fresh read; updates
  the cache; returns whether it changed), `get_managed_policy` command, `ManagedPolicyChanged` event (tauri-specta).
- `keys.rs`: the key-name constants and the pure parse `ManagedPolicy::from_source(&dyn ManagedPrefsSource)`. The ONE
  place a key name is spelled in Rust.
- `source.rs`: `trait ManagedPrefsSource { fn forced_value(&self, key: &str) -> Option<plist::Value>; }` with
  `CfPrefsSource { domain }` (macOS: `CFPreferencesAppSynchronize` once per read pass, then per key
  `CFPreferencesAppValueIsForced` → `CFPreferencesCopyAppValue` → `plist::Value`), `PlistFileSource` (test builds only,
  below), `NoManagedPrefs` (other platforms), and a `FakeSource` for unit tests.
- `locked.rs`: `locked_settings(&ManagedPolicy) -> Vec<LockedSetting>` where `LockedSetting { id, lock }` and `lock` is
  `Fixed(serde_json::Value)` or `DisallowedValues(Vec<serde_json::Value>)` (for `ai.provider` under `DisableCloudAI`:
  `cloud` disallowed). The ONE mapping from policy to registry setting ids; the frontend overlay and MCP `set_setting`
  both read it.
- `ceiling.rs`: `UpdateCeiling` parse and `allows(&semver::Version)`. `hosts.rs`: `HostPattern` parse and
  `allows(base_url)`, using `url::Url::host_str()` (same parser `validate_ai_base_url` uses).

The CF↔`plist::Value` conversion in `dock/prefs.rs` moves to a shared crate-level helper (for example `cf_plist.rs`)
that both `dock` and `managed_policy` use, rather than a second copy (jscpd would flag it anyway).

Refresh triggers:

- **First read is lazy and blocking**: `current()` initializes the cache with a synchronous read on first use (a
  `OnceLock`/`LazyLock`), so there is NO window where a caller sees `Default` (= no restriction) because setup hadn't
  reached the load yet. Fail-open-until-loaded is exactly the bug an ordering comment can't prevent. `setup()` still
  calls it explicitly before `crash_reporter::init` (`lib.rs:284`) so the cost lands at a known point. That one read
  runs on the main thread (one `cfprefsd` XPC round trip, same as `glass_tint`'s initial read); every LATER read goes
  off the main thread.
- **Activation**: `NSApplicationDidBecomeActiveNotification` (the `glass_tint.rs` pattern, `spawn_blocking`).
- **A file watch on `/Library/Managed Preferences`** (recursive, so the per-user subfolder counts; skip when it doesn't
  exist). Activation alone misses the common case: an MDM pushes while Cmdr stays frontmost for hours with a long Ask
  Cmdr turn or a model download running, and nothing would stop it until the next send. Chromium watches the same path
  for the same reason (see § The preference domain). The watcher only triggers `refresh()`; CF stays the source of
  truth.
- **Every egress point** (rule 6). Coalescing to at most one CF read per second is fine (an Ask Cmdr tool loop or a
  suggestion stream fires many requests).

On a change, `refresh()` emits `ManagedPolicyChanged` and calls one explicit `apply_change(app, old, new)` that does the
immediate stops: AI cancels in-flight cloud turns and suggestion streams whose backend the NEW policy would refuse
(cloud off, or its host no longer allowed), stops `llama-server` and cancels an in-progress local model download when
`DisableAI` arrived. No observer registry: one function, a visible call list.

Egress gates sit in the LOWEST send function, not only in the commands. The commands still refuse early with a typed
outcome (good UX, no wasted bundle build), but the guarantee comes from the send function, so a new caller can't forget
it:

- api-server senders: `analytics` heartbeat send, `crash_reporter::post_crash_report`, `error_reporter::upload`,
  `error_reporter::auto_sent::send_amend`, the update-check and tarball fetches. A `ServerRequestError::BlockedByPolicy`
  variant fits all of them (they already share that type), mapped in `server-request.ts`.
- LLM: the three `ai/client.rs` request functions check the fresh policy against the backend's own base URL right before
  `exec_chat*`. This is what makes rule 6 hold for Ask Cmdr, whose backend is resolved once per turn and then reused
  across the whole tool loop, and it closes the `resolve_backend_with_model` re-read of `get_cloud_config()`.
  `resolve_backend` keeps its check too: it produces the typed, user-facing reason; the client check is the backstop.

Test-build override: **`CMDR_MANAGED_PREFS_FILE=<path to a plist>`** replaces the CF source with `PlistFileSource`,
honored ONLY under `cfg(debug_assertions)` or the `playwright-e2e` feature. ❌ Never honor it in a plain release build:
it replaces IT's policy wholesale. ❌ Don't add it to `prod_instance::NON_PROD_ENV_VARS`; it isn't a harness signal.

Frontend: **`src/lib/managed-policy/`** holds a reactive store fed by `get_managed_policy` at `initWindowSettings()` and
refreshed on `ManagedPolicyChanged`. The settings store reads it: `getSetting(id)` returns the `Fixed` value for a
locked id (and maps a disallowed stored value to the setting's safe value, `off` for `ai.provider`), `setSetting` and
`resetSetting` refuse a locked id without writing, and `SettingRow` plus the row primitives render locked ids disabled
with the managed note. Restricted windows (viewer, queue) don't render any of these settings, so they need nothing.

## Draft copy (David reviews; English only)

- Row note, generic: "Your organization manages this setting."
- Updates off: "Your organization manages updates for Cmdr."
- Ceiling: "Cmdr {available} is out, but your organization keeps this Mac on {ceiling} or earlier."
- AI off: "Your organization turned off AI in Cmdr."
- Cloud AI off: "Your organization allows only on-device AI."
- Host not allowed (picker row, connection check, translate toast): "Your organization doesn't allow this AI service.
  Your IT team can tell you which ones you can use."
- Reports off (crash dialog never shows; Help › Send error report dialog): "Your organization turned off sending
  reports. You can still save one to disk and share it yourself."
- Usage stats off (onboarding step): "Your organization turned off usage stats."
- MCP refusal texts can be plain English (agent-facing), with the typed `data.reason`.

## Milestones

Sequential, one agent each. Each runs `pnpm check --fast` while iterating and plain `pnpm check` (Rust milestones:
clippy included) before committing, and updates the `CLAUDE.md` / `DETAILS.md` of every directory it touches.

### M1. Policy core: read, parse, cache, expose

- **Scope**: `managed_policy/` as above, the shared CF↔plist helper (moved out of `dock/prefs.rs`), lazy first read,
  activation refresh, the `/Library/Managed Preferences` watch, `get_managed_policy` command + `ManagedPolicyChanged`
  event + generated bindings, `PlistFileSource` override, `managed_policy/CLAUDE.md` + `DETAILS.md` (the canonical key
  catalog, parse rules, precedence, manual test recipe), a line in `docs/architecture.md`. No gate changes yet.
- **Intentions**: everything downstream asks `managed_policy::current()` / `refresh()` and never touches CFPreferences
  or key names itself. `ManagedPolicy` exposes typed answers (`usage_stats_disabled()`,
  `ai() -> AiPolicy { Allowed, LocalOnly, Off }`, `cloud_host_allowed(url)`, `updates() -> UpdatePolicy`), not raw
  values. `UpdatePolicy` is `Disabled | Enabled { automatic_checks: bool, ceiling: Option<UpdateCeiling> }`: a flat
  `{ Allowed, NoAutomaticChecks, Ceiling, Disabled }` enum can't express "a ceiling AND no background checks", which is
  the most common combination.
- **Landmines**: `CFPreferencesCopyAppValue` merges user, any-user, and managed layers, so ALWAYS gate on `IsForced`
  first (rule 3). Call `CFPreferencesAppSynchronize` before reading, or a profile installed while running stays
  invisible to this process (the `glass_tint.rs` gotcha). CF calls go off the main thread except the one lazy first read
  (§ Architecture). A forced `false` is "no restriction", not "managed with value false": no key locks anything when
  it's not restrictive. Moving `to_plist` must keep `dock`'s tests green.
- **Test plan**: pure tests over `FakeSource` for every key: absent, forced `true`/`false`, the bool coercions
  (`"false"`, `"YES"`, `0`, `2`, a dict), wrong type (rule 4), the ceiling grammar (`"0.52"`, `"0.52.3"`, `"1"`, integer
  `1`, real `0.52`, `"v0.52"`, `""`, `"latest"`, `"0.52.3.1"`) and its prerelease edge (`0.53.0-rc.1` vs `"0.52"` is
  REFUSED), host patterns (exact, `*.` suffix, bare apex vs `*.`, case, trailing dot, entry with port vs URL with and
  without port, pasted-URL entry, IPv6 `::1` vs `[::1]`, userinfo trick `https://api.openai.com@evil.com`, look-alike
  `api.openai.com.evil.com`, IDN, `*` alone), precedence combos (ceiling + no automatic checks together),
  `locked_settings` output, and that `current()` before any explicit load returns the real policy, not `Default`. A CF
  integration test on a scratch domain (`com.getcmdr.policytest.<tag>`, torn down like the dock tests) proving a value
  the USER layer holds is NOT reported. `PlistFileSource` round-trip. Then the manual recipe once on this Mac (see
  "Testing without an MDM"), evidence dated in `DETAILS.md`.
- **DONE**: `get_managed_policy` returns the right view with a real managed plist and with the file override; nothing
  else in the app behaves differently yet; `pnpm check` green.

### M2. Telemetry enforcement

- **Scope**: `analytics::send_permission` gains a `ManagedOff` outcome (handled like `OptedOut`: capture drops, the
  heartbeat deletes spool and unreported uptime; the heartbeat refreshes the policy before sending). Crash reports:
  `check_pending_crash_report` discards the pending file and answers `None` when reports are disabled;
  `send_crash_report`, `send_crash_log_report`, `send_error_report`, `amend_error_report` refuse with a typed
  `ManagedOff` variant before any I/O; Flow B's dispatcher checks the policy at send time (in addition to its
  `set_enabled` atomic) so a forced off wins over a stored on. The backstop: `post_crash_report`,
  `error_reporter::upload`, and `auto_sent::send_amend` each refresh and refuse with
  `ServerRequestError::BlockedByPolicy` themselves (§ Architecture, "lowest send function"), so the guarantee doesn't
  depend on every command remembering. `save_error_report_to_disk` unchanged. Docs: `analytics/`, `crash_reporter/`,
  `error_reporter/`, and `server_request.rs`'s frontend mapping.
- **Intentions**: one predicate per pipeline, read where the send happens. A user's stored `true` in `settings.json` is
  irrelevant while the policy says off.
- **Landmines**: the panic hook and signal handler must not touch the policy (allocation, locks, XPC): capture stays as
  is; only delivery checks. The auto-dispatcher's no-flush-on-shutdown rule stays. ❌ Don't move the crash-file stamp
  (`error_reporter/CLAUDE.md`). A refusal is not an error: log at `info`/`debug`, never via `log_error!` (that IS the
  auto-report path). The crash discard must use the existing claim/delete helpers, not a raw `remove_file` that races a
  newer crash.
- **Test plan**: red-first unit tests for `send_permission` with each policy × consent combo; heartbeat test that a
  managed-off beat sends nothing and clears the spool (existing localhost-Worker test harness); command-level tests that
  each send command returns the typed refusal without calling `upload`; dispatcher test with `set_enabled(true)` and a
  managed off; each low-level sender (`post_crash_report`, `upload`, `send_amend`) against a `wiremock` server
  (`crash_reporter/tests.rs:910` is the template) asserting ZERO requests received under the policy.
- **DONE**: with `DisableUsageStats` / `DisableCrashAndErrorReports` forced, no request reaches `api.getcmdr.com`
  `/heartbeat`, `/crash-report`, or `/error-report` from any path, verified by tests.

### M3. Update enforcement

- **Scope**: `check_for_update` returns a typed outcome that distinguishes "up to date", "update available", "held by
  policy (available X, ceiling Y)", and "updates disabled by policy" (reshape the `Option<UpdateInfo>` return, update
  bindings and the frontend callers mechanically). `DisableUpdates` short-circuits BEFORE the network request.
  `download_update` and `install_update` re-check the policy and the ceiling against the staged version (a staged update
  from before the profile arrived must not install). That needs a reshape: today `download_update(url, signature)`
  trusts a frontend-supplied URL and `UpdateState` keeps only the tarball path, so the backend can't know the staged
  version and a bypassed frontend could stage anything. `check_for_update` stores the offered `UpdateInfo` (version,
  url, signature) in `UpdateState`; `download_update` takes no URL and downloads only what was offered (re-checking the
  ceiling against that version); the staged slot records the version; `install_update` refuses when the CURRENT policy
  disallows it. The frontend background loop treats `DisableAutomaticUpdateChecks` as `updates.autoCheck = false` via
  the overlay; the backend still refuses a background-triggered check if called anyway (the trigger is already passed
  for analytics; pass it to the command). Docs: `updater/` and `src/lib/updates/` C+D.md.
- **Intentions**: the backend decides; the frontend renders the outcome. The `update_check` analytics event gets a phase
  for each new outcome (categorical, no versions in props).
- **Landmines**: `checkForUpdates()` must never early-return on `ready` (`src/lib/updates/CLAUDE.md`); a managed outcome
  is a new terminal phase, not a failure (no error toast, no `log_error!`). Ceiling compare uses the same `semver` parse
  as `manifest.rs`, but on the release core only (§ Updates: a `0.53.0-rc.1` must not pass a `"0.52"` ceiling). The
  Linux plugin path is out of scope (macOS only, per § Linux); say so in the doc.
- **Test plan**: ceiling compare matrix; `check_for_update` with each rule using an injected manifest fetch (no network)
  proving `Disabled` never fetches; install refusal for a staged version above the ceiling (and for a policy that
  arrived between download and install); `download_update` with nothing offered refuses; frontend `updater.*.test.ts`
  for each outcome's state and that the background loop doesn't call the command under `NoAutomaticChecks`.
- **DONE**: each update rule behaves as specified with tests; the menu check under `DisableUpdates` shows the managed
  message instead of a check.

### M4. AI enforcement

- **Scope**: `resolve_backend` reads the policy: `Off` → new `BackendResolution::ManagedOff`; `LocalOnly` with provider
  `cloud` → `ManagedCloudOff`; a disallowed host → `ManagedHostBlocked`. Each maps to a new `AiTranslateErrorKind` (keep
  `translate-error-toast.ts` in lockstep), a quiet empty for suggestions, a `SlotRefusal` for Ask Cmdr, and an MCP
  `ai_search` `data.reason` (`aiManagedOff`, `cloudAiManagedOff`, `aiHostNotAllowed`). `has_current_cloud_consent` gains
  the policy argument (as its doc anticipates) and `CloudAiConsentStatus` a `managed` field. `check_ai_connection`
  refuses a blocked host or managed-off cloud without a request. `configure_ai` stores config as today but never spawns
  `llama-server` under `Off`; `start_ai_server` and `start_ai_download` refuse under `Off`; `compute_ai_status` never
  `Offer`s under `Off`. Wake readiness maps `Off` / cloud-blocked to a silent state. `apply_change` (from M1) cancels
  in-flight cloud work and stops the server, reusing the `revoke_cloud_ai_consent` sequence. The per-request backstop in
  the three `ai/client.rs` request functions (§ Architecture). Under `AllowedCloudAIHosts`, the remote client gets a
  `reqwest::redirect::Policy::custom` that stops at any hop whose host the policy refuses (genai 0.6.5 takes our own
  client via `ClientBuilder::with_reqwest`); without that, a 3xx from an allowed host reaches any host and invariant 3
  is false. A batch command `cloud_ai_hosts_allowed(base_urls) -> Vec<bool>` for the frontend picker. Docs: `ai/` C+D.md
  (§ Cloud AI consent and § Provider routing), `mcp/DETAILS.md`, agent wake docs.
- **Intentions**: `resolve_backend` stays the one chokepoint; no caller adds its own policy check. Check whether Ask
  Cmdr can run on a local provider: if not, `LocalOnly` locks `askCmdr.enabled` off too (update `locked.rs`).
- **Landmines**: the policy check comes BEFORE consent, key, and endpoint checks, so the user sees the managed reason,
  not "add a key". The host check uses the same base URL `AiBackend::remote` will use (including Ask Cmdr's
  `resolve_backend_with_model`, which rebuilds the backend from a second `get_cloud_config()` read; the client-level
  backstop covers it structurally). An Ask Cmdr turn resolves its backend once and then makes many requests, so a policy
  arriving mid-turn is only caught by the client-level check or `apply_change`, never by `resolve_backend`.
  `configure_ai` must not block (existing must-know).
- **Test plan**: `resolve_backend_inner` matrix with the policy as a parameter (pure, no lock); translate-kind mapping
  tests on both sides; MCP `ai_search` tests asserting each typed `data.reason` (the existing `cloudAiNotAllowed` test
  is the template); consent predicate tests; `start_ai_download` refusal; a test that a policy change to `Off` cancels
  registered streams; a client-level test that a backend built while allowed refuses its NEXT request after the policy
  flips (the mid-turn case); a `wiremock` redirect test (allowed host 302s to a disallowed one, the second server
  receives nothing).
- **DONE**: under each AI key no request leaves for a disallowed destination from suggestions, both translate commands,
  Ask Cmdr (rail and wake), MCP `ai_search`, or the connection check, all proven by tests.

### M5. Frontend overlay and the MCP settings paths

- **Scope**: `src/lib/managed-policy/` store; settings-store overlay (`getSetting` / `setSetting` / `resetSetting` as in
  § Architecture); `SettingRow` and every row primitive pick up the lock themselves (disabled, managed note via
  `disabledNote`, no reset pip), so no section has to remember; `onSettingChange` fires for an id whose effective value
  changed when the policy changes, so `settings-applier.ts` re-pushes (for example `ai.*` through
  `pushConfigToBackend()`). MCP: `execute_set_setting` checks `locked_settings` in Rust BEFORE the round trip and
  refuses with `data.reason: "managedByOrganization"`; `mcp-get-all-settings` reports effective values plus a
  `managed: true` marker. Docs: `src/lib/settings/` C+D.md, `components/` C+D.md, `docs/guides/adding-a-new-setting.md`
  (one line: a policy-lockable setting gets its lock in `managed_policy/locked.rs`).
- **Intentions**: a locked row is visible and greyed with its reason, never hidden (the settings OS-backed-row
  precedent). Search still finds it.
- **Landmines**: sparse persistence: the overlay must never write (rule 5), and `isModified` must not report a locked
  row as modified because of the overlay. The settings window, main window, and onboarding all run
  `initWindowSettings()`; the policy fetch goes there, not per section. `onSpecificSettingChange` subscribers must see
  the overlay change exactly once.
- **Test plan**: settings-store tests (locked get/set/reset, disallowed stored value, overlay change emits once, nothing
  persisted); `SettingRow` / primitive a11y tests for the locked state (the note is reachable through
  `aria-describedby`); `mcp-main-bridge.test.ts` for the marker; Rust test for the `set_setting` refusal.
- **DONE**: every setting `locked_settings` names renders locked with the note in Settings and can't be changed from the
  UI or MCP.

### M6. Feature surfaces: updates, privacy, reports, onboarding

- **Scope**: Updates & privacy section (the check button and status line render each M3 outcome, including the ceiling
  sentence); the menu "Check for updates" toast for managed outcomes; the error-report dialog under
  `DisableCrashAndErrorReports` (send hidden, "Save to disk" kept, the managed line); the crash flow shows nothing (the
  backend already answers `None`); `StepBeta.svelte` shows the managed line in place of the usage-stats switch. Copy in
  `messages/en/*.json` with `@key` descriptions.
- **Intentions**: one sentence of reason, in the place the user looks; no dead buttons without a reason.
- **Landmines**: nothing may show during onboarding from the updater (`shouldShowUpdateToast()`); keep it. The crash
  dialog copy rules (`crash-copy.ts`) don't apply because nothing renders.
- **Test plan**: component tests per surface and outcome; a11y tests for new states; one Playwright spec using
  `CMDR_MANAGED_PREFS_FILE` that opens Settings › Updates & privacy and asserts the locked rows and notes (read
  `test/e2e-playwright/CLAUDE.md` first).
- **DONE**: each telemetry and update key is visible and explained everywhere a user would look.

### M7. Feature surfaces: AI

- **Scope**: `AiSection` / `AiCloudSection` (provider control with `cloud` disabled under `LocalOnly`, everything locked
  under `Off`), the provider picker in `src/lib/ai-provider-setup/` (presets whose host `cloud_ai_hosts_allowed` rejects
  render disabled with the reason; a custom or Azure URL is checked when entered), `AiCloudConsentToggle` (locked from
  `CloudAiConsentStatus.managed`), `AskCmdrSection`, `StepAi.svelte` (onboarding skips the AI step under `Off` and
  offers only local or off under `LocalOnly`), and the translate-error toasts for the new kinds.
- **Intentions**: the shared provider-setup steps change once and both onboarding and Settings get it
  (`sections/CLAUDE.md`: those controls aren't the section's).
- **Landmines**: a disallowed-host preset still needs its row visible with the reason, not removed. The Azure preset's
  placeholder URL has no real host until the user types one; check after entry. ❌ No API key crosses IPC for any of
  this (the host check takes a URL, never a key).
- **Test plan**: picker tests with a mocked `cloud_ai_hosts_allowed`; consent-toggle locked test; `StepAi` tests per
  policy; translate-toast tests for the three kinds; one Playwright spec with an `AllowedCloudAIHosts` file override.
- **DONE**: each AI key is visible and explained in Settings, onboarding, and the error toasts.

### M8. Sample profile, `/trust`, and the drift guard

- **Scope**: publish `apps/website/public/mdm/cmdr-managed-preferences.mobileconfig` (a complete, unsigned profile:
  top-level `PayloadType` `Configuration`, `PayloadScope` `System`, one payload whose `PayloadType` is
  `com.veszelovszki.cmdr` carrying EVERY key with a sensible example value, stable UUIDs, an XML comment per key) and
  `apps/website/public/mdm/com.veszelovszki.cmdr.plist` (the bare key dictionary, for Jamf "Application & Custom
  Settings" / Kandji "Custom Profile" / Intune "Preference file" uploads). `/trust`: a "Central management (MDM)"
  section listing every key (type, effect, precedence, the macOS-only note), linking both files, and replacing the "No
  central administration" gap line in `trust.ts` (keep the `.pkg`/PPPC line) and the :70 sentence. A Rust test that
  `include_str!`s both files and asserts their key set equals the key constants in `keys.rs` and that each example value
  parses without warnings. Wire the files into whatever the website's link checks need. `/trust` also says plainly what
  still talks to the network with every key set (the "Egress NO key covers" list in § Fresh grep): a security review
  asks exactly that, and finding license validation or the CLIP download on their own would cost more trust than listing
  them.
- **Intentions**: an admin can download one file, edit values, and upload it. The guard makes a new key impossible to
  forget in the public docs.
- **Landmines**: coordinate with the `website-copy` agent (it edits `/trust` on this branch): stage only your files, and
  leave copy polish to David. A `.mobileconfig` served as `text/plain` opens in the browser; check whether the website
  host needs a `_headers` entry (`application/x-apple-aspen-config`) and add one only if the mechanism exists. Validate
  both files with `plutil -lint`. Don't sign the profile (MDMs re-sign on upload).
- **Test plan**: the drift-guard test (red first: add a key constant with no profile entry and see it fail);
  `plutil -lint`; website build and its checks.
- **DONE**: the files are published under `/mdm/`, `/trust` documents every key, the gap line is gone, the guard is
  green.

### M9. Close-out

- An adversarial conformance review against the invariants below (fresh agent), then a docs audit of every touched
  `CLAUDE.md` / `DETAILS.md`.
- David installs the sample profile once on a real Mac (see below) and walks the three groups. Then tick the three #118
  checkboxes and the `/trust` one.

## Testing without an MDM

1. **Unit and integration tests**: `FakeSource` for logic, `PlistFileSource` for whole-app tests, the scratch-domain CF
   test for the "user layer doesn't count" rule. No test needs root or a profile.
2. **Dev and E2E runs**: `CMDR_MANAGED_PREFS_FILE=/path/to/policy.plist pnpm dev` (debug builds and the `playwright-e2e`
   feature only).
3. **Real managed preferences, fast** (needs `sudo`, so David runs it; unverified until M1 records evidence):
   `sudo defaults write "/Library/Managed Preferences/com.veszelovszki.cmdr" DisableUsageStats -bool true`, then
   reactivate Cmdr (or rely on the folder watch). If `IsForced` doesn't see it, check the file is `root:wheel` `0644`
   like a profile-written one, then `sudo killall cfprefsd` and retry. Record which of these were needed. Undo with
   `sudo defaults delete "/Library/Managed Preferences/com.veszelovszki.cmdr"`. On an MDM-enrolled Mac, `mdmclient` may
   rewrite that folder; use a spare or unenrolled Mac.
4. **The real thing**: open the published `.mobileconfig`, approve it in System Settings › General › Device Management,
   confirm with `sudo profiles show -type configuration` and
   `defaults read "/Library/Managed Preferences/com.veszelovszki.cmdr"`, then remove it in the same pane. This is what
   an MDM delivers, minus the MDM.

## Decisions for David

1. **Feedback.** Should `DisableCrashAndErrorReports` also block in-app feedback (`send_feedback`)? The plan says no:
   feedback is text the person typed and sent on purpose. A separate `DisableFeedback` key is cheap if an admin asks.
2. **Local AI under `DisableAI`.** The plan blocks the local LLM too ("no AI at all"). The media index's on-device CLIP
   model isn't covered (it's search indexing, not a generative feature). Agree, or should `DisableAI` cover CLIP too?
3. **Optional extras, not in this plan**: a `DisableMCPServer` key, a Jamf JSON schema for a form-based editor in Jamf
   Pro, and "force on" keys for pre-configuring a corporate AI gateway (`CloudAIBaseURL` plus pre-approved consent).
   Each is a clean add-on later.
4. **Copy**: the draft lines in § Draft copy.
5. **Loopback under `DisableCloudAI`.** The plan blocks Ollama / LM Studio on `localhost` too, because they're `cloud`
   providers and `localhost` can be an SSH tunnel to a remote box. The alternative (allow loopback as "on-device") is
   friendlier but unprovable. Recommendation: keep blocking, say so on `/trust`, and point admins at
   `AllowedCloudAIHosts: [localhost]`.
6. **Does the heartbeat say "managed"?** With usage stats still on, `analytics/config_shape.rs` reports STORED settings
   (`ai.provider: cloud`) that the policy overrides, which skews the dashboard. Options: (a) report effective values,
   (b) add a coarse `managed: true` flag (useful: how many installs are IT-managed), (c) leave it. Recommendation: (a) +
   (b), no key names or values.
7. **Remaining egress.** License validation, the S3 price list, and the CLIP model download stay on under every key (see
   § Fresh grep). Fine for v1 if `/trust` lists them; a `DisableNonEssentialNetwork`-style key is a later add-on.

## Invariants (the close-out review checks each)

1. No path sends usage stats, crash reports, or error reports while the matching key is forced on.
2. No update request (check, download, install) happens under `DisableUpdates`; nothing above the ceiling installs.
3. No LLM request reaches a cloud host under `DisableAI` / `DisableCloudAI`, or a host outside `AllowedCloudAIHosts`
   (redirect hops included); no local model download or server start under `DisableAI`.
4. Only forced (managed) values count; the user layer of the domain never does.
5. No key can enable anything; a malformed key restricts as much as it can.
6. Nothing writes a forced value into `settings.json` or the consent record.
7. Every egress gate reads the policy fresh; a profile change while running applies at the next send.
8. Key names are spelled once in Rust (`keys.rs`); the sample profile, plist, and `/trust` match it (drift-guard test).
9. Every refusal is typed; no string matching on messages.
10. The frontend never decides policy: it renders `get_managed_policy`, `locked_settings`, and typed outcomes.
11. No caller can observe the "not loaded yet" policy; `current()` is never fail-open.
12. Every egress gate lives in the lowest send function (api-server senders, the `ai/client.rs` request functions), not
    only in the commands above it.

## Review round 1

Fresh-eyes review against the code at `f6ef6d051` (2026-10-05). Changes, ranked by what would have bitten:

1. **Ask Cmdr mid-turn hole.** The backend is resolved once per turn and reused across the tool loop, so rule 6 ("the
   next send") didn't hold for it, nor for `resolve_backend_with_model`'s second config read. Added the per-request
   backstop in the three `ai/client.rs` request functions, and the general rule that gates live in the lowest send
   function (`post_crash_report`, `upload`, `send_amend`, heartbeat, updater fetches), with a shared
   `ServerRequestError::BlockedByPolicy`.
2. **Redirects broke invariant 3.** The plan accepted that an allowed host can redirect anywhere while invariant 3
   promised no request reaches a disallowed host. Now enforced per hop with a custom redirect policy (genai 0.6.5
   `with_reqwest`).
3. **Ceiling let prereleases through.** `0.53.0-rc.1 < 0.53.0` in semver, so a `"0.52"` ceiling would have allowed 0.53
   prereleases. Compare on the release core. Also: integer accepted, real rejected, grammar spelled out.
4. **The staged-version check couldn't be built.** `download_update` takes the URL from the frontend and the backend
   never learns the version. M3 now stores the offered `UpdateInfo` and stops trusting a frontend URL.
5. **`UpdateRule` couldn't express ceiling + no automatic checks**, the most common IT combination. Reshaped to
   `UpdatePolicy`.
6. **Fail-open startup window.** `current()` returned `Default` (no restriction) until setup loaded it, and the crash
   next-launch path runs before `load_settings`. Now lazily initialized. Also resolved the "CF off the main thread" vs
   "read in setup" contradiction.
7. **Activation-only refresh** missed an MDM push while Cmdr stays frontmost (long agent turn, model download). Added a
   `/Library/Managed Preferences` watch, Chromium's approach, with citations.
8. **Value parsing**: string bools (`defaults write` without `-bool` writes strings), host normalization (IPv6 brackets,
   trailing dot, IDNA, pasted URLs, ports), unknown keys ignored.
9. **Fresh grep gaps**: `crash_reporter/pending_delivery.rs`, `error_reporter::upload`, `auto_sent::send_amend`, the
   `ai/client.rs` request functions, the `resolve_backend*` callers, the real `cloud-providers.ts` path, and the egress
   no key covers (now also listed on `/trust` in M8).
10. Smaller: explicit domain constant (not `kCFPreferencesCurrentApplication`), the override works on Linux E2E,
    `apply_change` cancels model downloads and newly-disallowed-host work, test-recipe permission hint, decisions 5–7.
