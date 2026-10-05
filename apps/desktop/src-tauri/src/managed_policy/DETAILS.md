# Managed policy: details

The canonical key catalog for Cmdr's MDM support. `/trust` and the sample profile mirror this file; the plan behind it
is `docs/specs/mdm-managed-preferences-plan.md`.

## The preference domain

- Domain: `com.veszelovszki.cmdr` (`config::BUNDLE_ID`), passed explicitly, never `kCFPreferencesCurrentApplication`, so
  dev builds and tests read the same domain as release.
- Profiles write `/Library/Managed Preferences/com.veszelovszki.cmdr.plist` (device scope) or
  `/Library/Managed Preferences/<user>/com.veszelovszki.cmdr.plist` (user scope). `CFPreferencesCopyAppValue` merges
  both with the user's own layer; `CFPreferencesAppValueIsForced` tells the managed layer apart. `CfPrefsSource` asks
  `IsForced` first and only then copies the value.
- Each read pass starts with `CFPreferencesAppSynchronize`, or a profile installed while Cmdr runs stays invisible.
- Linux and other platforms: `NoManagedPrefs`, "no restriction". A future Linux source (for example
  `/etc/cmdr/policy.json`) slots in behind `ManagedPrefsSource`.

## Key catalog

Every key only turns something off or narrows it. Absent or forced to its permissive value means "no restriction".

- **`DisableUsageStats`** (bool): no heartbeat, no events; spool and unreported uptime deleted. Locks
  `analytics.enabled` to off.
- **`DisableCrashAndErrorReports`** (bool): no crash report, no "Attach logs", no automatic or manual error report, no
  amend; a pending crash file is discarded at launch. "Save to disk" stays. Locks `updates.crashReports` and
  `updates.errorReports` to off. In-app feedback is NOT covered: it's text the person typed and sent on purpose (decided
  2026-10-05).
- **`DisableAutomaticUpdateChecks`** (bool): no background check; a manual check still works. Locks `updates.autoCheck`
  to off.
- **`DisableUpdates`** (bool): no check of any kind, no download, no install. Overrides the other two update keys. Locks
  `updates.autoCheck` to off.
- **`MaxUpdateVersion`** (string, or an integer read as a major): "never update past this version". `"0.52"` allows up
  to the last 0.52.x (core `< 0.53.0`), `"0.52.3"` up to and including 0.52.3, `"1"` anything below 2.0.0. Grammar: one
  to three dot-separated non-negative integers, optional leading `v`, surrounding whitespace trimmed. A `<real>` is
  refused (`0.5` and `0.50` are the same real). Unparseable or empty: behaves as `DisableUpdates`. Compared on the
  release core only, so `0.53.0-rc.1` doesn't pass `"0.52"`. ❗ The updater only knows the NEWEST release
  (`latest.json`), so once a release past the ceiling ships, a held Mac gets no more patches: it's "hold here until IT
  moves the ceiling", not a channel. Combines with `DisableAutomaticUpdateChecks`.
- **`DisableAI`** (bool): no AI at all, local model included: no LLM call, no model download, no `llama-server`, no Ask
  Cmdr, MCP `ai_search` refuses. The media index's on-device CLIP model is NOT covered: it's search indexing, not a
  generative feature (decided 2026-10-05). Locks `ai.provider` to `off` and `askCmdr.enabled` to off.
- **`DisableCloudAI`** (bool): the local model stays allowed; nothing goes to a cloud provider, loopback endpoints
  (Ollama, LM Studio) included, since `localhost` can be a tunnel to anywhere. `ai.provider` can't be `cloud`; a stored
  `cloud` reads as `off` (❌ never `local`: that would start a multi-GB download).
- **`AllowedCloudAIHosts`** (array of strings): cloud AI may only reach these hosts. Entries: a host
  (`api.openai.com`), `host:port`, a `*.` suffix pattern (`*.openai.azure.com`: any subdomain, not the apex), or a
  pasted URL (only its host and an explicit port count). Matching: IDNA to punycode, case-insensitive, one trailing dot
  ignored, compared as `url::Host` values (so `::1` matches `[::1]`); an entry with a port matches only that port, one
  without matches any. Malformed entries (`*`, `*.`, a pattern on an IP, a `*` elsewhere, an unparseable port, a path
  without a scheme) are dropped with a warning. An empty list, or a non-array value, allows no host, which equals
  `DisableCloudAI`. Recipe for "local Ollama only": leave `DisableCloudAI` off and list `localhost` and `127.0.0.1`.

Not keys (stay out until an admin asks; decided 2026-10-05): `DisableMCPServer`, a Jamf JSON schema, and "force on"
keys such as a pre-configured corporate AI gateway.

Egress no key turns off: license validation, the S3 price list (`Egress::S3PriceList`, always allowed), the CLIP model
download, and the user-initiated feedback and beta signup.

## Value parsing

- **Bools** accept `<true/>` / `<false/>`, an integer (`0` false, anything else true), and the strings `true`, `false`,
  `yes`, `no`, `1`, `0` in any case, trimmed. `defaults write … Key true` without `-bool` stores a string, hence the
  coercion. Anything else reads as `true` (restrictive) and warns.
- A forced value Core Foundation can't convert reads as `plist::Value::Data`, the wrong type for every key, so each key
  takes its restrictive reading.
- Unknown keys are ignored with one `debug!` (sources that can list their keys only), so a profile written for a newer
  Cmdr doesn't break an older one.
- Warnings go to the log only when they differ from the previous read: a bad profile warns once, not on every refresh.

## Precedence

- AI: `DisableAI` > `DisableCloudAI` > `AllowedCloudAIHosts`, folded into `ManagedPolicy::ai()` (`Allowed` /
  `LocalOnly` / `Off`) and `ai_destination()`. Only Cmdr's own `llama-server` (`AiDestination::LocalServer`) counts as
  on-device.
- Updates: `DisableUpdates` wins; `MaxUpdateVersion` and `DisableAutomaticUpdateChecks` combine, which is why
  `UpdatePolicy::Enabled` carries both.
- Per gate: allowed = policy allows AND the user's own setting allows.

## Data flow

- `ManagedPolicy` (fields as parsed) → typed answers → `ManagedPolicyView` (one payload for `get_managed_policy` and the
  `managed-policy-changed` event, carrying the lock list and what the M6 "what your organization manages" summary
  needs: update ceiling, AI mode, allowed hosts).
- `locked_settings()` maps policy to settings-registry ids: `Fixed { value }` or `DisallowedValues { values, fallback }`.
  `LockedValue` is a typed `bool | string` because `serde_json::Value` can't cross IPC. The fallback rides along so
  neither the frontend nor `overlay()` decides the safe value.
- `overlay()` applies the locks to a raw `settings.json` map in memory. A missing or non-object file becomes an object
  holding just the pinned values when anything is locked, since its readers fall back to defaults (`analytics.enabled`
  defaults to on).

## Refresh

- **Lazy first read**: `current()` initializes through a `OnceLock`, so no caller observes an unloaded policy. `init()`
  in `setup()` (before `crash_reporter::init`) forces that read at a known point; it's the one CF read on the main
  thread.
- **Activation**: `NSApplicationDidBecomeActiveNotification` on the DEFAULT center, read on a blocking thread.
- **Folder watch**: FSEvents on `/Library/Managed Preferences`, recursive, skipped when the folder doesn't exist (the
  first profile creates it; activation catches that one). Activation alone misses an MDM push while Cmdr stays
  frontmost through a long Ask Cmdr turn or a model download. Chromium watches the same path and also reloads
  periodically, calling it "undocumented and therefore fragile".
- **Every egress**: `for_egress().await`, coalesced to one read per second, behind a lock so racing callers make one
  trip.
- A change calls `apply_change` (cache.rs): today it logs and emits `managed-policy-changed`; the AI milestone adds the
  immediate stops there (cancel in-flight cloud work, stop `llama-server`, cancel a model download).

## Where the gates live

- **Api-server egress**: `server_request::send(Egress, request)` asks `allows` on a fresh read and refuses with
  `ServerRequestError::BlockedByPolicy` before anything leaves. Every api-server sender goes through it, the heartbeat
  included. Commands that build a bundle first ask the same decision through `server_request::check_policy`.
- **Usage stats**: `analytics::send_permission` and the heartbeat's config shape read `settings.json` through
  `overlay`, so a managed off is an ordinary opt-out and the heartbeat reports effective values plus a coarse
  `managedByOrganization` bool.
- **Crash reports**: `check_pending_crash_report` discards the pending file unoffered under `DisableCrashAndErrorReports`.
- **Updates**: `ManagedPolicy::update_to(version)` is the one version decision (`UpdateRefusal::Disabled` /
  `AboveCeiling`). The updater asks `updates()` before a check (no request under `DisableUpdates`, none for a
  background trigger under `DisableAutomaticUpdateChecks`), then `update_to` for the offered release, again before the
  download and the install, and once more for the version the extracted archive names. `updater/DETAILS.md` § Managed
  policy.

## Testing

- Tests elsewhere put a policy in force with `testing::override_for_test(testing::forcing(&[KEY]))`: a guard that makes
  `current()` and `for_egress()` answer that policy on the test's own thread (a `#[tokio::test]` runs its tasks there),
  so parallel tests never see each other's policy.
- Unit tests run over `FakeSource`. `source.rs` has a scratch-domain CF test (`com.getcmdr.policytest.<tag>`) proving a
  value in the user's own layer isn't reported, and a `PlistFileSource` round trip. `view.rs` tests
  `get_managed_policy` end to end through `CMDR_MANAGED_PREFS_FILE`.
- Dev and E2E: `CMDR_MANAGED_PREFS_FILE=/path/to/policy.plist pnpm dev` (debug and `playwright-e2e` builds only). The
  file is a plain plist dictionary of keys; every key in it counts as forced. Edit it and re-activate Cmdr to refresh.
- Real managed preferences need `sudo`, so an agent can't run them:
  `sudo defaults write "/Library/Managed Preferences/com.veszelovszki.cmdr" DisableUsageStats -bool true`, then
  re-activate Cmdr (or rely on the folder watch). If `IsForced` doesn't see it, make the file `root:wheel` `0644` like a
  profile-written one, then `sudo killall cfprefsd`. Undo with
  `sudo defaults delete "/Library/Managed Preferences/com.veszelovszki.cmdr"`. Unverified until David runs it (plan
  M9); record which steps were needed here, dated.
