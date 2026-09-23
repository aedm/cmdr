# Cloud AI consent: one toggle before anything reaches a cloud AI service

**Problem.** Only Ask Cmdr asks before sending data to the user's AI service. Folder name suggestions, natural-language
search, select-by-description, and MCP `ai_search` send names and prompts as soon as a Cloud provider is set up, with no
consent step (the website `/trust` page says so, and carries a DevTodo about it). Ask Cmdr's own opt-in covers only Ask
Cmdr, and it applies on Local too, where nothing leaves the Mac.

**Outcome.** One versioned, backend-enforced "Allow cloud AI" toggle in Settings > AI > Provider, shown only on Cloud,
off by default, gating every cloud LLM call. Its disclosure lists what every feature sends. Ask Cmdr's toggle becomes a
plain feature on/off. Local AI needs no consent. Decisions 1–10 from David are fixed; this plan implements them.

Read before starting: `apps/desktop/src-tauri/src/ai/CLAUDE.md` + `DETAILS.md`, `agent/CLAUDE.md` + `DETAILS.md`
(invariants 7 and 8), `agent/wake/CLAUDE.md`, `agent/store/CLAUDE.md`, `apps/desktop/src/lib/ask-cmdr/DETAILS.md` §
"Consent gate, cost, and settings", `lib/onboarding/DETAILS.md` § "What 'off' turns off", `lib/settings/DETAILS.md`,
`lib/intl/messages/CLAUDE.md`, `docs/guides/i18n.md`, `docs/guides/i18n-translation.md`, `docs/style-guide.md`.

## Decisions this plan makes (flag if wrong)

- **D1. The chokepoint is `ai::manager::resolve_backend`, and it takes an `&AppHandle<R>`.** Every LLM call in the app
  resolves its backend there (census below). Adding the app handle to its signature forces every caller through the
  consent read at compile time, and making `AiBackend::remote` `pub(in crate::ai)` means no code outside `ai/` can build
  a cloud backend around it. One chokepoint suffices; two places need an extra, non-LLM gate (`check_ai_connection` and
  the Ask Cmdr send path's feature toggle), described below.
- **D2. Consent lives in `main.db`'s `meta` table under new keys** (`cloud_ai_consent_version`, `cloud_ai_consent_at`),
  with its logic in a new `apps/desktop/src-tauri/src/ai/cloud_consent.rs` (moved and generalized from
  `agent/consent.rs`). Why `main.db`: it's durable, migrated, transactional, `sqlite3`-inspectable, and the fail-closed
  read plus the "a refused write stays a no" machinery (`RevokePending`) already exist and are tested around it. Why the
  logic moves to `ai/`: cloud consent is an `ai/` concept now, and `resolve_backend` needs it. `ai` already depends on
  `agent` (`ai/state.rs` → `agent::chat::budget`, `ai/manager.rs` → `agent::wake::refresh_readiness`), so the new
  `ai → agent::store` edge sits inside the existing tangle; confirm with `pnpm check desktop-rust-module-cycles` (❌
  never raise its allowlist without David). Rejected: a JSON file in the AI dir (needs a third copy of
  `atomic_write_json` and loses the tested store); `ai-state.json` (non-atomic `fs::write`, errors swallowed).
- **D3. `CLOUD_AI_CONSENT_VERSION = 1`.** Bump it whenever the `ai.cloudConsent.*` copy changes materially. Agent
  invariant 8 ("widening the egress line is a copy change AND a version bump") retargets to this constant.
- **D4. The legacy Ask Cmdr record stays in `main.db` untouched** (`ask_cmdr_consent_version` / `_at`) and grants
  nothing. Its only reader is the one-time Ask Cmdr on/off mapping below. Nothing is deleted.
- **D5. Ask Cmdr's on/off is a registry setting, `askCmdr.enabled`** (boolean, registry default `false`, a visible row
  with `mcpSettable: false`, see D9). The backend reads it fresh from `settings.json`, and an absent key reads `false`
  (fail quiet). Mapping:
  - **(a) had accepted Ask Cmdr consent** (any version, and no held revoke): `true`. They asked for Ask Cmdr; on Local
    it keeps working with no interruption, on Cloud it waits for the cloud toggle.
  - **(b) existing install that never accepted** (or has a held "no"): `false`. They saw an opt-in and didn't take it;
    turning Ask Cmdr on for them (including the proactive loop, which ships on) would turn a "not yet" into a yes.
  - **(c) new install**: the onboarding pick decides. Cloud or Local writes `true`, "no AI" writes `false`. On Cloud the
    cloud toggle still gates it, so this grants no data flow. A re-run of onboarding writes `true` only when the key was
    never set explicitly, so it can't re-arm a switch the user turned off; "no AI" always writes `false`.
- **D6. The mapping runs as a main-window startup step**, idempotent, guarded by "`askCmdr.enabled` not yet explicitly
  set" and "`onboarding.completed` is true" (a fresh install that hasn't finished onboarding is case (c), and onboarding
  sets the key). It asks the backend `ask_cmdr_legacy_opt_in()` → `Recorded` / `NotRecorded` / `StoreUnavailable`;
  `StoreUnavailable` writes nothing and retries next launch. Not a settings schema migration: those run before the agent
  store is guaranteed open, and can't retry once `_schemaVersion` is stamped.
- **D7. Missing cloud consent on Cloud is its own wake state, `NeedsCloudConsent`**: nothing new is admitted to the
  inbox, what's stored is kept (like `Off`), and the status corner stays silent. Ask Cmdr off (`AskCmdrOff`, the renamed
  `NeedsConsent`) keeps today's purge semantics, since the backlog was gathered for Ask Cmdr. Precedence: `AskCmdrOff` →
  `Off` → `NeedsCloudConsent` → `NeedsFullDiskAccess` → `NeedsApiKey` → `Ready`.
- **D8. Turning the toggle off stops cloud AI immediately**: the record clears (or the "no" is held), then every
  in-flight cloud call is cancelled: running Ask Cmdr turns (rail and wake, `agent/chat/cancel.rs`) and folder
  suggestion streams (`ai/stream_registry.rs`). Both registries gain a `cancel_all()`. Translate calls are one-shot
  requests of a few seconds; they finish, and the next one refuses. `ai.provider` is untouched.
- **D9. MCP can't flip consent.** The record lives in `main.db` behind dedicated commands that no MCP tool reaches
  (`set_setting` writes registry settings only). The two new hidden-or-sensitive settings,
  `ai.cloudConsentRevokePending` and `askCmdr.enabled`, carry `mcpSettable: false` (commit `f358206f6`'s mechanism).
  `askCmdr.enabled` isn't consent, but on Local it starts a proactive loop, so an MCP client shouldn't be able to switch
  it on. E2E specs turn Ask Cmdr on by clicking the rail's button, as they click the consent button today.
- **D10. "Only on the user's click" is held by a call-site census test**: a Vitest test scans `src/` and fails if
  `acceptCloudConsent` is imported anywhere except the two toggle components (`AiCloudConsentToggle.svelte`, used by
  Settings and onboarding). The dev-only `tauri-plugin-mcp-bridge` can run webview JS, but it's compiled out of release
  builds (`docs/security.md` § withGlobalTauri).
- **D11. `check_ai_connection` is gated too.** Its `GET /models` carries no user data, but it does reach the service,
  and "nothing reaches a cloud AI service until the user agrees" is the promise. It returns a typed
  `cloud_consent_missing: true` instead of probing, and the locked section never calls it anyway.
- **D12. MDM later**: `has_current_cloud_consent` is the single predicate every gate calls. A future managed preference
  becomes one more input there (a `Policy` argument, like `RevokePending`) and one more field on the status the toggle
  reads (to render it locked). Nothing here designs that out.

Decisions made during implementation:

- **D13. `check_ai_connection` is gated whatever `ai.provider` says** (narrows D11's "when `ai.provider` is cloud"). It
  only ever probes a cloud endpoint being set up, and onboarding can probe before `configure_ai` has pushed the
  provider, so keying on the backend's provider would let one probe through.
- **D14. The slot's refusal is its own small enum, `session::SlotRefusal { NotConfigured, NoCloudConsent }`**, mapped
  onto `AgentErrorKindView` by `SlotRefusal::view` (a `From` beside the view type would make `stream` depend on
  `session` and close a module cycle through `runtime`). `AgentErrorKind` is the runtime's mid-turn vocabulary and stays
  as it was.
- **D15. The consent commands return typed errors** (`CloudAiConsentWriteError::{StoreUnavailable, StoreRefused}`), not
  the old `Result<(), String>`, and `cloud_ai_consent_status` returns the status directly (a missing or unreadable store
  answers not-accepted). They use their own short `main.db` helpers instead of `commands::agent::with_*`, so `ai/` gains
  no edge into `commands/`. A missing store is `StoreUnavailable`, not the old silent `Ok`.
- **D16. The meta-row helpers live in `agent/store/consent.rs`** (split out of `query.rs`, which would otherwise cross
  the file-length limit). The legacy Ask Cmdr writers (`set_ask_cmdr_consent` / `clear_ask_cmdr_consent`) exist only
  until milestone 2 drops the Ask Cmdr consent commands.
- **D17. MCP `ai_search` refuses with `invalid_params` plus `data.reason: "cloudAiNotAllowed"`**, so a client acts on a
  typed field, not the sentence (the `ToolError.data` contract).
- **D18. The send gate is one pure function, `session::admit_send(ask_cmdr_enabled, resolve)`**, which the command
  calls and the runtime test mirrors: Ask Cmdr off refuses without resolving the slot, then the slot's `SlotRefusal`
  maps to its wire kind. The four Ask Cmdr gate switches read fresh from `settings.json` moved to
  `settings/ai_gates.rs` (`loader.rs` would otherwise cross the file-length limit).
- **D19. Until milestone 3, the frontend's old Ask Cmdr consent wrappers are inert shims** (`tauri-commands/ask-cmdr.ts`):
  status reads the cloud status, revoke revokes cloud consent, and accept records NOTHING, because the Ask Cmdr
  disclosure never described the other AI features and so must not grant cloud AI for them. Between milestones 2 and 3
  the rail refuses every send with `askCmdrOff` (nothing sets `askCmdr.enabled` yet); that's expected.

## Census

### LLM call sites and backend resolution

Every path that reaches an LLM (the `ai/llm_log/mod.rs` `JobKind`s plus MCP):

- `JobKind::FolderSuggestions`: `ai/suggestions.rs::get_folder_suggestions` and `stream_folder_suggestions` →
  `manager::resolve_backend().ready_or_log(..)`. Frontend: `lib/file-operations/mkdir/NewFolderDialog.svelte`.
- `JobKind::TranslateSearch`: `commands/search.rs::translate_search_query` → `resolve_translate_backend(false)` →
  `resolve_backend().into_translate_result()`. Frontend: `lib/search/ai-translate.ts` via `SearchDialog.svelte`.
- MCP `ai_search`: `mcp/executor/search.rs::execute_ai_search` calls `commands::search::translate_search_query`
  directly, so an external MCP client drives the same cloud call.
- `JobKind::TranslateSelection`: `commands/selection.rs::translate_selection_query` → `resolve_translate_backend(true)`.
  Frontend: `lib/selection-dialog/SelectionDialog.svelte` (chip shown only on `cloud`).
- `JobKind::AgentChat`, rail: `commands/agent/chat.rs::ask_cmdr_send_message` → consent check, then
  `agent/chat/session.rs::resolve_agent_llm` → `manager::resolve_backend_with_model` → `resolve_backend`.
- `JobKind::AgentChat`, wake: `agent/wake/writer.rs` → `resolve_agent_llm(AgentSlot::Wake)` → same.
- Wake readiness (no call, but decides whether one happens): `agent/wake/snapshot.rs::provider_gate` →
  `resolve_backend_with_model`.
- Non-LLM contact with the service: `ai/connection_check.rs::check_ai_connection` (`GET /models`, key only).
  `configure_ai` sends nothing for cloud (it spawns a local server only).
- `AiBackend::remote` / `AiBackend::local` are built only in `ai/manager.rs` in production code; test-only callers:
  `agent/llm/live_smoke_test.rs`, `selection/ai/real_llm_eval_test.rs`, and the `ai/client_*_test.rs` files.

**Chokepoint**: `resolve_backend`. `resolve_backend_with_model` already calls it; `resolve_translate_backend` already
calls it. So the check sits in one function, and the census above collapses to "every caller passes an app handle".

### Readers of Ask Cmdr consent today

Backend:

- `agent/consent.rs`: `CONSENT_COPY_VERSION` (4), `RevokePending`, `has_current_consent`.
- `agent/store/query.rs`: `get_consent` / `set_consent` / `clear_consent`, `AskCmdrConsent` wire type; test in
  `agent/store/tests.rs::consent_round_trips`.
- `commands/agent/consent.rs`: `ask_cmdr_consent_status`, `ask_cmdr_accept_consent`, `ask_cmdr_revoke_consent`,
  `ask_cmdr_consent_revoke_pending_changed`; registered in `ipc.rs` (lines ~443–446).
- `commands/agent/chat.rs`: the send gate → `AgentErrorKindView::NoConsent` (`agent/chat/stream.rs`, token `no_consent`;
  `agent/chat/runtime/analytics.rs::refusal_props`; tests in `stream/tests.rs`, `runtime/tests.rs`).
- `agent/wake/snapshot.rs::consented`, `agent/wake/readiness.rs` (`AgentGates.consented`, `WakeReadiness::NeedsConsent`,
  `admits_to_inbox`, `permits_stored_signal`), `agent/wake/indicator.rs`, `agent/wake/followup.rs`, the inbox purge
  (`Inbox::purge_if_consent_withdrawn`), tests in `agent/wake/tests/{readiness,inbox,job}.rs`.
- `settings/loader.rs::load_ask_cmdr_consent_revoke_pending` (+ `loader_tests.rs`).
- Doc comments citing the gate: `mcp/executor/image_facts.rs`, `mcp/executor/photos.rs`, `mcp/tool_registry/table.rs`
  (~484).

Frontend:

- `lib/ask-cmdr/ask-cmdr-consent.svelte.ts` (+ `.svelte.test.ts`), `AskCmdrConsent.svelte` (+ test),
  `AskCmdrRail.svelte`, `ask-cmdr-trigger.svelte.ts` (`openRail` refreshes and gates), `wake-indicator.svelte.ts`
  (`'needsConsent'`), `ask-cmdr-labels.ts` (the `noConsent` error label), `rail.a11y.test.ts`, other rail tests mocking
  consent.
- `lib/settings/sections/AskCmdrSection.svelte` (+ `.rows.ts`, tests), `lib/settings/definitions/advanced.ts`
  (`askCmdr.consentRevokePending`), `lib/settings/mcp-main-bridge.ts` (+ test).
- `lib/onboarding/StepAi.svelte` (`declineConsent` on "no AI"), `StepAi.test.ts`, `OnboardingWizard.test.ts`.
- `routes/(main)/+layout.svelte` (`heldConsentRevoke` startup step), `lib/suggested-ops/SuggestedOpsIndicator.svelte`
  (comment), `lib/tauri-commands/ask-cmdr.ts` + `index.ts`, `lib/ipc/bindings.ts` (generated).

Analytics: `analytics/config_shape.rs` ships every boolean setting, so `askCmdr.enabled` and
`ai.cloudConsentRevokePending` will ship automatically (the legacy `askCmdr.consentRevokePending` already does). The
consent record itself is not a setting and won't ship (see open question 2).

E2E: `test/e2e-playwright/ask-cmdr.spec.ts`, `ask-cmdr-wake.spec.ts` (`ensureConsented`), `app-death.test.ts`,
`marketing-shots.spec.ts`, `i18n-capture-ask-cmdr.ts`, `i18n-capture-main-pass.ts`.

## Backend changes (file by file)

1. **`ai/cloud_consent.rs`** (new; `agent/consent.rs` is deleted):
   - `pub const CLOUD_AI_CONSENT_VERSION: u32 = 1;`
   - `RevokePending` (moved), its `load` reading the new `settings::load_cloud_consent_revoke_pending`.
   - `pub fn has_current_cloud_consent(conn: &Connection, revoke: RevokePending) -> bool`, fails closed exactly like
     today (absent, stale, unreadable, or held "no" → false).
   - `pub(crate) fn cloud_consent_from_app<R: Runtime>(app: &AppHandle<R>) -> bool`: `try_state::<AgentDb>()` →
     `open_read_connection()` → `has_current_cloud_consent(.., RevokePending::load(app))`; any failure logs a warn and
     answers `false`.
   - The four Tauri commands (moved from `commands/agent/consent.rs`, renamed): `cloud_ai_consent_status` →
     `CloudAiConsentStatus { accepted, current_version, accepted_version, accepted_at }`, `accept_cloud_ai_consent`,
     `revoke_cloud_ai_consent` (clears, then `cancel_all` per D8), `cloud_ai_consent_revoke_pending_changed`. Each write
     calls `agent::wake::refresh_readiness` and emits a new typed event `CloudAiConsentChanged` so every window's state
     refreshes. They reuse `commands::agent::{with_read_connection, with_write_connection}` (widen to `pub(crate)` if
     needed). Register all four in the `ipc.rs` manifest. Unit tests from `agent/consent.rs` move here.
   - A module doc stating the invariant: the consent predicate has ONE caller path into the send decision,
     `resolve_backend`, plus the readiness snapshot and the status command.
2. **`agent/store/query.rs`**: generalize the meta helpers over a `ConsentRecord` enum (`CloudAi` →
   `cloud_ai_consent_*`, `AskCmdrLegacy` → today's keys). `get_consent(conn, record)`,
   `set_consent(conn, ConsentRecord::CloudAi, ..)`, `clear_consent(conn, ConsentRecord::CloudAi)`. Make it
   unrepresentable to write the legacy record: `set_consent` / `clear_consent` take a `WritableConsent` type that only
   has the cloud variant, or the legacy read is a separate `get_legacy_ask_cmdr_consent`. Rename the wire type to
   `ConsentRecordView` (or keep `AskCmdrConsent` for the legacy read only). No schema migration: `meta` is key/value.
3. **`ai/manager.rs`**:
   - `BackendResolution::NoCloudConsent`.
   - `resolve_backend<R: Runtime>(app: &AppHandle<R>)` and `resolve_backend_with_model(app, model_override)`: read
     provider as today; only when it's `"cloud"`, call `cloud_consent_from_app(app)`. Pass the bool into
     `resolve_backend_inner(provider, port, api_key, base_url, model, requires_api_key, cloud_consent)`, which returns
     `NoCloudConsent` for cloud BEFORE the key/URL checks (consent precedes setup; the UI locks setup anyway). Local and
     off ignore it. Read the consent outside the `MANAGER` lock.
   - `resolve_translate_backend(app, cloud_only)`.
   - `into_translate_result`: `NoCloudConsent` → `AiTranslateErrorKind::NoCloudConsent`. `ready_or_log`: `None` with a
     debug log.
4. **`ai/translate_error.rs`**: `AiTranslateErrorKind::NoCloudConsent` (doc: "Cloud AI is picked but the user hasn't
   allowed it in Settings > AI").
5. **`ai/client.rs`**: `AiBackend::remote` and `AiBackend::local` → `pub(in crate::ai)`, with a doc line saying why. Add
   `#[cfg(test)] pub(crate) fn remote_for_tests(..)` for `agent/llm/live_smoke_test.rs` and
   `selection/ai/real_llm_eval_test.rs` (and the local twin).
6. **`ai/suggestions.rs`**: both commands take `app: AppHandle` (Tauri injects it; the bindings don't change shape) and
   call `resolve_backend(&app)`. Behavior on refusal stays `Ok(Vec::new())` / a `done` stream.
7. **`commands/search.rs`**, **`commands/selection.rs`**: take `app: AppHandle`; split a generic
   `translate_search_query_with<R: Runtime>(app: &AppHandle<R>, ..)` for the MCP path.
8. **`mcp/executor/search.rs`**: `execute_ai_search(app, params)` (thread the handle from the dispatcher, as other
   executors do) → a `ToolError` carrying a plain sentence ("Cloud AI isn't allowed in Cmdr's settings.") when the kind
   is `NoCloudConsent`, chosen by kind, never by message.
9. **`ai/connection_check.rs`**: `check_ai_connection(app, base_url, provider_id)`; when `ai.provider` is cloud and
   consent is missing, return `AiConnectionCheckResult { cloud_consent_missing: true, connected: false, .. }` without a
   request. Add the field (default `false`).
10. **`agent/chat/session.rs::resolve_agent_llm`**: pass `app`; map `NoCloudConsent` → a new
    `AgentErrorKind::NoCloudConsent` (or a small refusal enum if the runtime kind shouldn't grow), surfacing as
    `AgentErrorKindView::NoCloudConsent` (token `no_cloud_consent`).
11. **`commands/agent/chat.rs::ask_cmdr_send_message`**: replace the consent block with the feature gate:
    `settings::load_ask_cmdr_enabled(&app)` false → `AskCmdrSendRefusal::of(AgentErrorKindView::AskCmdrOff)`. The
    cloud-consent refusal then comes from `resolve_agent_llm`, before a thread exists.
12. **`agent/chat/stream.rs`**: `AgentErrorKindView::NoConsent` → `AskCmdrOff` + `NoCloudConsent`, tokens `ask_cmdr_off`
    / `no_cloud_consent` (a PostHog token change; note it in the commit). Update `runtime/analytics.rs`.
13. **`settings/loader.rs`** (+ `mod.rs` re-exports, `loader_tests.rs`): `load_ask_cmdr_enabled` (only a real JSON
    `true` counts; absent → `false`), `load_cloud_consent_revoke_pending` (key `ai.cloudConsentRevokePending`). Keep
    `load_ask_cmdr_consent_revoke_pending`; its only caller becomes the legacy mapping.
14. **`commands/agent/consent.rs`** → replaced by `commands/agent/legacy_opt_in.rs`: `ask_cmdr_legacy_opt_in(app)` →
    `LegacyAskCmdrOptIn { Recorded, NotRecorded, StoreUnavailable }`. Pure core
    `legacy_opt_in(record: Result<Option<..>>, held: bool)`. Register in `ipc.rs`; drop the four old commands.
15. **`agent/wake/readiness.rs`**: `AgentGates { ask_cmdr_enabled, fda_pending, provider }`,
    `ProviderGate::NeedsCloudConsent` (from `BackendResolution::NoCloudConsent`), `WakeReadiness::AskCmdrOff` (renamed
    `NeedsConsent`) and `WakeReadiness::NeedsCloudConsent`, precedence per D7, `admits_to_inbox` false for both new
    states, `permits_stored_signal` false only for `AskCmdrOff`. Rewrite the module doc for the new gates.
16. **`agent/wake/snapshot.rs`**: `consented()` → `ask_cmdr_enabled()` reading the setting; the initial atomic value
    becomes `AskCmdrOff` (still closed). `provider_gate(app)` passes the handle. `indicator.rs`, `followup.rs`, the
    inbox purge rename (`purge_if_ask_cmdr_off`), and the wake tests follow.
17. **`agent/chat/cancel.rs`**, **`ai/stream_registry.rs`**: `cancel_all()`.
18. **Frontend ↔ wake**: a command `ask_cmdr_enabled_changed(app)` that calls `refresh_readiness`, called by the
    settings applier on `askCmdr.enabled` (the readiness snapshot is cached; `askCmdrWakeSettingsChanged` only re-reads
    the loop's own settings).
19. **Bindings**: `cd apps/desktop && pnpm bindings:regen` after each IPC change; `desktop-bindings-fresh` guards it.

## Frontend changes (file by file)

1. **`lib/ai/cloud-consent.svelte.ts`** (new, ported from `ask-cmdr-consent.svelte.ts`, which is deleted):
   `cloudConsentState { accepted: boolean | null, acceptedAt, needsReconsent }`, `refreshCloudConsent()`,
   `acceptCloudConsent(): Promise<ConsentOutcome>`, `declineCloudConsent()` (revoke, retry once, hold in
   `ai.cloudConsentRevokePending` via `forceSave` + `cloud_ai_consent_revoke_pending_changed`),
   `settleHeldCloudConsentRevoke()`, and a listener for `CloudAiConsentChanged` that refreshes. Plus a derived helper
   `cloudAiBlocked(provider)`: `provider === 'cloud' && cloudConsentState.accepted !== true` (null counts as blocked).
   Keep the "never throws, `notSaved` is never `done`" contract and its doc.
2. **`lib/ai/AiCloudConsentToggle.svelte`** (new): the `Switch` + label + description, the disclosure (a `<details>`,
   open while off), the "on since {date}" line, and the `notSaved` line. The ONLY importer of `acceptCloudConsent`
   besides tests (D10). Shared by Settings and onboarding.
3. **`lib/settings/sections/AiSection.svelte`**: under the provider `SectionCard`, when `provider === 'cloud'`, render
   `AiCloudConsentToggle`, then `AiCloudSection` with `locked={cloudAiBlocked}`. Add `AiSection.rows.ts` with
   `row:ai.cloudConsent` (keywords: consent, privacy, allow, send, data) and register it where the other `.rows.ts`
   files are collected.
4. **`lib/settings/sections/AiCloudSection.svelte`**: a `locked` prop: wrap the content in a container with `inert` and
   a dimmed style, plus the `settings.ai.cloudConsent.lockedHint` line above it. While locked, don't call
   `controller.setProvider` (it can trigger a connection check); call it when the lock lifts.
5. **`lib/settings/definitions/ai.ts`**: `askCmdr.enabled` (boolean, default `false`, `mcpSettable: false`, label and
   description keys below). **`definitions/advanced.ts`**: `ai.cloudConsentRevokePending` (hidden, `mcpSettable: false`,
   modeled on `askCmdr.consentRevokePending`); keep the legacy entry hidden, with its comment saying it's read only by
   the legacy mapping. **`types.ts`**: the new ids. **`settings-applier.ts`**:
   `'askCmdr.enabled': () => void askCmdrEnabledChanged()`.
6. **`lib/settings/settings-store.ts`**: export `isExplicitlySet(id)` (reads the `explicitlySet` ledger; `isModified` is
   value-based and can't tell "never set" from "set to the default").
7. **`routes/(main)/+layout.svelte`**: replace the `heldConsentRevoke` step with `heldCloudConsentRevoke`
   (`settleHeldCloudConsentRevoke`) and add `askCmdrEnabledMapping` (new `lib/ask-cmdr/ask-cmdr-enabled-mapping.ts`,
   D6), after `settings`.
8. **`lib/settings/sections/AskCmdrSection.svelte`**: the enable block becomes a `SettingSwitch` on `askCmdr.enabled`.
   Drop the three-state status, "on since", the paused state, and the disclosure. When `enabled && cloudAiBlocked`, show
   `settings.askCmdr.cloudOffHint` plus a link button that opens Settings > AI > Provider at the consent row. Drop
   `row:askCmdr.consent` from `AskCmdrSection.rows.ts` (the switch is a registry row now).
9. **`lib/ask-cmdr/AskCmdrRail.svelte`** + new `AskCmdrGate.svelte` (replaces `AskCmdrConsent.svelte`): three states in
   order: `askCmdr.enabled` false → the off gate with a "Turn on Ask Cmdr" button (sets the setting); on Cloud and
   blocked → the cloud gate with "Open AI settings" (`openSettingsWindow('main', ['AI', 'Provider'], <anchor>)`);
   otherwise the chat. Keep the `null`-means-render-nothing rule so nothing flashes. Keep the `.consent` class names
   out: E2E selectors move to `.ask-cmdr-gate` (update `desktop-svelte-e2e-stale-selector` inputs).
10. **`lib/ask-cmdr/ask-cmdr-trigger.svelte.ts`**: `openRail` refreshes cloud consent and reads `askCmdr.enabled`;
    bootstraps history only when the chat state is showing. A send refusal `askCmdrOff` / `noCloudConsent` refreshes and
    flips the rail to the gate (`ask-cmdr-labels.ts` keeps labels for both, for the rare race).
11. **`lib/ask-cmdr/wake-indicator.svelte.ts`**: silent for `'askCmdrOff'`, `'off'`, `'needsCloudConsent'`.
12. **`lib/file-operations/mkdir/NewFolderDialog.svelte`**: before streaming, if `cloudAiBlocked(provider)`, set
    `aiAvailable = false` and return. No copy (decision 7: quiet).
13. **`lib/search/SearchDialog.svelte`**, **`lib/selection-dialog/SelectionDialog.svelte`**,
    **`lib/query-ui/EmptyState.svelte`** (via `query-dialog-config.ts`): pass an `aiBlocked` flag; the AI mode's empty
    state shows `queryUi.ai.cloudOff.body` plus an "Open AI settings" button in place of the AI examples. The chip stays
    visible so the feature stays discoverable.
14. **`lib/ai/translate-error-toast.ts`**: `'noCloudConsent'` in `ALL_KINDS` and the switch (warn level), with an "Open
    AI settings" action. If `addToast` can't carry an action, use a small content component in the style of
    `lib/reveal/RevealActivationToastContent.svelte`.
15. **`lib/onboarding/StepAi.svelte`**: when Cloud is picked, render `AiCloudConsentToggle` at the top of the right
    column, and `CloudProviderSetup` below it with `locked` while blocked (same `inert` treatment; add the prop). The
    first Next with Cloud and the toggle off shows `onboarding.ai.cloudConsentOffNote` as a confirm-once footer note,
    the same shape as the missing-key gate; the second Next passes. `persist()`:
    - `'off'`: `ai.provider = 'off'`, `askCmdr.proactive = false`, `askCmdr.enabled = false`, and
      `declineCloudConsent()` (open question 1), logging and moving on if `notSaved`.
    - `'cloud'` / `'local'`: `askCmdr.enabled = true` only if `!isExplicitlySet('askCmdr.enabled')`. ❌ Never grants
      cloud consent: only the toggle's own click does.
16. **`lib/settings/mcp-main-bridge.ts`**: no code change (it reads the mark); the test grows (below).
17. **`lib/tauri-commands/`**: wrappers for the new commands in an `ai` file, drop the Ask Cmdr consent wrappers.

## Copy (drafts for David's review)

All in sentence case, active voice, no em dashes, apostrophes as `''` in ICU catalogs (typographic `’` where the
existing keys use it; match the file). Every key gets an `@key.description` meeting `messages/DETAILS.md` § `@key`
metadata schema.

### Moved keys (text unchanged, translations carried over)

The Ask Cmdr "what's sent" list moves under the cloud disclosure. Rename in `en` AND every locale, moving each value
with its whole `@key` block (the `sourceHash` stays valid because the `en` text doesn't change), and update each
description's first sentence to say it now appears under Settings > AI > Allow cloud AI:

- `askCmdr.consent.item.{messages,names,sizes,contents,envelope,attachments,memory}` →
  `ai.cloudConsent.askCmdr.item.{same}`
- `askCmdr.consent.contentsRule` → `ai.cloudConsent.askCmdr.contentsRule`
- `askCmdr.consent.memory` → `ai.cloudConsent.askCmdr.memory`
- `askCmdr.consent.proactive` → `ai.cloudConsent.askCmdr.proactive`
- `askCmdr.consent.logsNote` → `ai.cloudConsent.logsNote`
- `askCmdr.consent.notSaved` → `ai.cloudConsent.notSaved` (its description changes: it's about the cloud AI choice)
- `settings.askCmdr.status.onSince` → `settings.ai.cloudConsent.onSince`

### Dropped keys

`askCmdr.consent.{title,intro,local,accept,decline}`, `askCmdr.consent.whatsNew.{title,body}`,
`settings.askCmdr.{status.on,status.off,status.needsReview,status.changed,turnOn,turnBackOn,turnOff,disclosure.title}`.
Delete them from `en` and let `sync-locale-keys.ts` drop them everywhere. `askCmdr.consent.local` ("Your chats stay on
your Mac...") could survive as a line under the Ask Cmdr switch; drafted below as a new key instead so its description
can change.

### New keys

The disclosure (`en/ai.json`), in render order:

- `ai.cloudConsent.label`: "Allow cloud AI"
  - `@`: Label of the switch in Settings > AI > Provider (and in the onboarding AI step) that allows Cmdr to send data
    to the cloud AI service the user set up. Off by default; nothing reaches the service until it's on. Short, starts
    with a verb.
- `ai.cloudConsent.description`: "Cmdr sends nothing to a cloud AI service until you turn this on."
  - `@`: One-line description under ai.cloudConsent.label.
- `ai.cloudConsent.disclosureTitle`: "What Cmdr sends"
  - `@`: Title of the fold listing everything Cmdr sends to the cloud AI service once the switch is on. Open by default
    while the switch is off.
- `ai.cloudConsent.intro`: "With this on, Cmdr sends these to the AI service you set up below, using your own account:"
  - `@`: Opening line of the list of what each AI feature sends. Ends with a colon introducing the list.
- `ai.cloudConsent.whereItGoes`: "Where your data goes, and what happens to it there, depends on the service you pick
  and your agreement with it. That includes Ollama, LM Studio, and custom endpoints: the data goes wherever that server
  runs. Cmdr has no AI service of its own and never sees any of it."
  - `@`: Paragraph right after the intro. Keeps three facts: the destination and its handling depend on the chosen
    service and the user's terms with it; Ollama, LM Studio, and custom endpoints count too; Cmdr itself receives
    nothing. Ollama and LM Studio are product names; keep them.
- `ai.cloudConsent.folderSuggestions`: "<b>New folder name suggestions</b>: the current folder's path and up to 100
  names in it"
  - `@`: List item. The bold part names the feature (the suggestions in the New folder dialog), the rest says what's
    sent. Keep the number.
- `ai.cloudConsent.search`: "<b>Search in plain words</b>: the text you type"
  - `@`: List item for natural-language search in the Search dialog: only the typed query is sent.
- `ai.cloudConsent.selection`: "<b>Select by description</b>: what you type, plus up to 240 names from the current
  folder"
  - `@`: List item for the AI mode of the Select dialog: the typed description plus a sample of names. Keep the number.
- `ai.cloudConsent.askCmdr.title`: "<b>Ask Cmdr</b>, when you chat or when it starts a conversation on its own:"
  - `@`: List item introducing the nested list of what the Ask Cmdr assistant sends (the moved
    `ai.cloudConsent.askCmdr.item.*` keys). Ends with a colon.
- (moved `ai.cloudConsent.askCmdr.item.*`, then `contentsRule`, `memory`, `proactive` paragraphs)
- `ai.cloudConsent.askCmdr.chatsStayLocal`: "Your Ask Cmdr chats stay on your Mac, in a local database you can open and
  read."
  - `@`: Paragraph under the Ask Cmdr part of the list: chat history is stored locally.
- (moved `ai.cloudConsent.logsNote`)
- `ai.cloudConsent.turnOffAnyTime`: "Turn this off any time and Cmdr stops sending right away. Your service, key, and
  chats stay as they are."
  - `@`: Last paragraph of the disclosure.
- (moved `ai.cloudConsent.notSaved`)

The `<b>` tags need `<Trans>` rendering (`lib/intl/Trans.svelte`); if a bold lead-in is unwanted, drop the tags and use
"New folder name suggestions: ..." plain.

Settings (`en/settings.json`):

- `settings.ai.cloudConsent.lockedHint`: "Turn on Allow cloud AI above to set up a service."
  - `@`: Hint above the dimmed, locked service setup in Settings > AI > Provider while the switch is off. "Allow cloud
    AI" is the switch label (ai.cloudConsent.label); translate it the same way.
- `settings.askCmdr.enabled.label`: "Turn on Ask Cmdr"
  - `@`: Label of the on/off switch for the Ask Cmdr assistant in Settings > AI > Ask Cmdr. Starts with a verb.
- `settings.askCmdr.enabled.description`: "Chat with Cmdr about your files in a side panel."
  - `@`: Description under settings.askCmdr.enabled.label.
- `settings.askCmdr.cloudOffHint`: "Ask Cmdr uses your cloud AI service, and cloud AI is off. Allow it in AI settings to
  start chatting."
  - `@`: Shown under the Ask Cmdr switch when it's on, the AI mode is Cloud, and the Allow cloud AI switch is off.
- `settings.askCmdr.openAiSettings`: "Open AI settings"
  - `@`: Button that jumps to Settings > AI > Provider.

Ask Cmdr rail (`en/askCmdr.json`):

- `askCmdr.gate.off.title`: "Ask Cmdr is off"
- `askCmdr.gate.off.body`: "Turn it on to chat with Cmdr about your files."
- `askCmdr.gate.off.turnOn`: "Turn on Ask Cmdr"
- `askCmdr.gate.cloudOff.title`: "Cloud AI is off"
- `askCmdr.gate.cloudOff.body`: "Ask Cmdr talks to the cloud AI service you set up. Allow cloud AI in Settings first,
  and you're ready to chat."
- `askCmdr.gate.cloudOff.openSettings`: "Open AI settings"
  - `@` for all six: the side panel's short state when Ask Cmdr is switched off, or when cloud AI isn't allowed yet; the
    button opens the relevant switch.
- `askCmdr.error.askCmdrOff` / `askCmdr.error.noCloudConsent` (replace the `noConsent` label in `ask-cmdr-labels.ts`):
  "Ask Cmdr is off." / "Cloud AI is off. Allow it in Settings > AI."

Search and Select (`en/queryUi.json`, `en/ai.json`):

- `queryUi.ai.cloudOff.body`: "Cloud AI is off. Allow it in Settings > AI to use this."
- `queryUi.ai.openSettings`: "Open AI settings"
- `ai.translateError.noCloudConsent.title`: "Cloud AI is off"
- `ai.translateError.noCloudConsent.body`: "Allow it in Settings > AI, then try again."

Onboarding (`en/onboarding.json`):

- `onboarding.ai.cloudConsentOffNote`: "Cloud AI stays off until you allow it. You can do that later in Settings > AI."
  - `@`: Footer note on the first Next press when Cloud is picked and the Allow cloud AI switch is off. A second press
    continues.

### What happens to the 10 translated locales

Findings (`docs/guides/i18n.md` § Enforcement, `docs/guides/i18n-translation.md` § New feature):

- A new `en` key fails `desktop-i18n-coverage` (ERROR) in every full-translation locale (`de`, `es`, `fr`, `hu`, `nl`,
  `pt`, `sv`, `vi`, `zh`, `zh-Hant`) until it's translated: missing and still-English both count. The `en-GB` / `en-AU`
  overlays only hold keys that differ; each carries two `askCmdr.consent.*` keys today, which must be renamed with the
  rest.
- A changed `en` value leaves every locale's translation `desktop-i18n-stale` (WARN, ERROR at release). A removed key
  still referenced anywhere fails `desktop-message-keys-unused` (ERROR), as does a new key with no call site.
- Glossaries under `docs/i18n/<locale>/` cite `askCmdr.consent.*` keys; `desktop-i18n-doc-citations` (ERROR) fails on
  the rename until each citation is repointed.
- The screenshot couplings name `ask-cmdr-consent.png` (`apps/desktop/scripts/representative-screenshots.ts`,
  `test/e2e-playwright/i18n-capture-ask-cmdr.ts`); a rename breaks `desktop-message-screenshots-fresh` (fails in CI).
  Recipe: `lib/intl/messages/DETAILS.md` § Screenshots. ❌ Don't hand-edit `@key.screenshot` fields.

Required steps, in order:

1. Rename the moved keys in `en` and in every locale dir (a small script over the JSON, moving value plus `@key` block),
   then add and drop keys in `en`.
2. `pnpm intl:keys` (regenerates `keys.gen.ts`).
3. `node apps/desktop/scripts/sync-locale-keys.ts` (adds English skeletons with `sourceHash`, drops removed keys).
4. Translate every new key in all 10 locales per `docs/guides/i18n-translation.md`, one translator agent per locale (or
   batched), each given § "The translator-agent context" plus `docs/i18n/<locale>/style.md` and `glossary.md`. Terms to
   settle once per locale and record in its glossary: "Allow cloud AI", "cloud AI".
5. Repoint glossary citations; update the representative-screenshot rules for the moved family (the disclosure now lives
   in Settings > AI > Provider) and the Ask Cmdr capture script.
6. Human review is not a ship gate (`i18n-translation.md` § Human review). David's `en` copy review may change values
   later; that surfaces as `i18n-stale` warnings, and re-translation follows the same loop.

## Tests (write first, see them fail)

Rust:

1. `ai/cloud_consent.rs`: absent → closed; stale version → closed; current → open; held revoke → closed over a recorded
   consent. (Ported from `agent/consent.rs`.)
2. `ai/manager.rs` tests on `resolve_backend_inner`: cloud with key, URL, and no consent → `NoCloudConsent`; cloud with
   consent → `Ready`; cloud without consent AND without a key → `NoCloudConsent`; local without consent → `Ready` when a
   port is up; off → `Off` either way. `into_translate_result(NoCloudConsent)` → kind `NoCloudConsent`;
   `ready_or_log(NoCloudConsent)` → `None`.
3. `translate_error.rs`: `NoCloudConsent` serializes as `"noCloudConsent"` (`desktop-rust-ipc-enum-camelcase` also pins
   the shape).
4. `agent/store/tests.rs`: the cloud record round-trips; writing and clearing it leaves the legacy Ask Cmdr keys
   untouched; the legacy read still sees them.
5. `commands/agent/legacy_opt_in.rs`: `Ok(Some(v4))` + not held → `Recorded`; `Ok(Some(v1))` → `Recorded`; held →
   `NotRecorded`; `Ok(None)` → `NotRecorded`; `Err` → `StoreUnavailable`.
6. `settings/loader_tests.rs`: `askCmdr.enabled` only a real `true` counts, absent reads `false`;
   `ai.cloudConsentRevokePending` likewise.
7. `agent/wake/tests/readiness.rs`: precedence per D7 (Ask Cmdr off beats everything; `Off` beats `NeedsCloudConsent`;
   `NeedsCloudConsent` beats FDA and key); `admits_to_inbox` false and `permits_stored_signal` true for
   `NeedsCloudConsent`; `permits_stored_signal` false only for `AskCmdrOff`. `snapshot.rs`: every state round-trips
   through the atomic; the initial value is closed.
8. `agent/wake/tests/inbox.rs`: the purge fires on `AskCmdrOff`, not on `NeedsCloudConsent`.
9. `agent/chat/runtime/tests.rs` (the mirror of the send path): Ask Cmdr off refuses before any thread; cloud without
   consent refuses before any thread and before any LLM; local with Ask Cmdr on proceeds. `stream/tests.rs`: the two
   view kinds serialize; `analytics.rs`: their tokens.
10. `ai/connection_check.rs`: the pure split returns `cloud_consent_missing` without building a client.
11. `cancel.rs` / `stream_registry.rs`: `cancel_all` cancels every registered token.

Vitest:

1. `lib/ai/cloud-consent.svelte.test.ts` (port of the Ask Cmdr consent tests): accept `done` only when the status reads
   accepted; decline retries once, then holds; a hold that `settings.json` refuses is `notSaved`; settle lets go after
   the store takes it; an unreadable status fails closed; `cloudAiBlocked` is true for `null`.
2. `lib/ai/cloud-consent-call-sites.test.ts` (D10): only `AiCloudConsentToggle.svelte` imports `acceptCloudConsent`.
3. `AiSection` (new `AiSection.svelte.test.ts`): the toggle renders only on Cloud; the cloud section is `inert` while
   off and live when on; switching off calls decline and leaves `ai.provider` as `'cloud'`; switching Cloud → Local →
   Cloud doesn't touch consent.
4. `mcp-main-bridge.test.ts`: `ai.cloudConsentRevokePending` and `askCmdr.enabled` are marked and refused.
5. `ask-cmdr-enabled-mapping.test.ts`: (a) → `true`; (b) → `false`; held → `false`; unavailable → nothing written;
   already explicitly set → no call; onboarding not completed → no call.
6. `StepAi.test.ts`: Cloud shows the toggle above a locked setup; the confirm-once note; "no AI" writes the three
   settings and declines cloud consent; Cloud and Local never call `acceptCloudConsent`; Cloud/Local write
   `askCmdr.enabled = true` only when unset.
7. `AskCmdrRail` / `AskCmdrGate` tests + `rail.a11y.test.ts`: off gate, cloud gate with its settings button, chat on
   Local with Ask Cmdr on, nothing rendered while `null`.
8. `AskCmdrSection.svelte.test.ts` + `.a11y.test.ts`: plain switch; the hint shows only on Cloud without consent.
9. `NewFolderDialog`: blocked → no `streamFolderSuggestions` call, no suggestion strip.
10. `translate-error-toast.test.ts`: `noCloudConsent` copy and level; `isAiTranslateError` accepts it.
11. `EmptyState.svelte.test.ts` / `ModeChips`: AI mode with `aiBlocked` shows the hint and button, chip still there.
12. `wake-indicator.svelte.test.ts`: silent for `askCmdrOff` and `needsCloudConsent`.

E2E (after the units are green): `ask-cmdr.spec.ts` / `ask-cmdr-wake.spec.ts` replace `ensureConsented` with an
`ensureAskCmdrOn` that clicks the rail's turn-on button (the E2E fake keeps `ai.provider` off, so no cloud consent is
involved); `app-death.test.ts` label; marketing and i18n capture scripts move to the new surfaces.

## Milestones

Each milestone ends green on `pnpm check` (per `AGENTS.md` cadence) and is its own commit (or a few).

1. **Backend consent core**: D1–D3, D11, backend steps 1–9 and 19, Rust tests 1–4 and 10. Frontend compiles against the
   new bindings with the old Ask Cmdr consent still in place (it now grants nothing for cloud; that's the point).
2. **Ask Cmdr split, backend**: steps 10–18, Rust tests 5–9 and 11.
3. **Frontend state, Settings, rail**: frontend steps 1–11 and 16–17, Vitest 1–5, 7, 8, 12.
4. **Feature entry points and onboarding**: frontend steps 12–15, Vitest 6, 9–11, E2E updates.
5. **Copy and i18n**: the key moves, new keys, all 10 locales, citations, screenshot couplings.
6. **Docs and website** (below). Then `pnpm check --include-slow`.

## Docs to update

- `apps/desktop/src-tauri/src/ai/DETAILS.md`: new § "Cloud AI consent" as the canonical home (the chokepoint, the
  version rule, fail-closed, the held "no", D7/D8/D11/D12); `ai/CLAUDE.md` gains one must-know ("every backend comes
  from `resolve_backend(app)`, which enforces cloud consent; `AiBackend::remote` stays `pub(in crate::ai)`") and loses
  nothing else. Update the module map for `cloud_consent.rs`.
- `agent/CLAUDE.md` + `DETAILS.md`: consent bullets become "Ask Cmdr has a plain on/off (`askCmdr.enabled`); cloud
  consent is `ai/`'s, see `ai/DETAILS.md`"; invariant 8 retargets to `CLOUD_AI_CONSENT_VERSION`; module map drops
  `consent.rs`. `agent/wake/CLAUDE.md` + `DETAILS.md` for the new states and precedence. `agent/store/CLAUDE.md` for the
  meta keys. `agent/chat/DETAILS.md` and `agent/tools/CLAUDE.md` + `DETAILS.md` where they cite the consent copy.
- `mcp/DETAILS.md`: `ai_search` is cloud-consent gated; the `mcpSettable` list gains the two settings.
- `apps/desktop/src/lib/ai/CLAUDE.md` + `DETAILS.md` (the new module and toggle), `lib/ask-cmdr/CLAUDE.md` (the "rail
  gates on consent" must-know becomes the two gate states) + `DETAILS.md` § Consent (rewritten, pointing to `lib/ai/`),
  `lib/settings/DETAILS.md` + `sections/DETAILS.md`, `lib/onboarding/CLAUDE.md` + `DETAILS.md` § "What 'off' turns off"
  (now four things), `lib/file-operations/mkdir/DETAILS.md`, `lib/suggested-ops/DETAILS.md`, `analytics/CLAUDE.md`
  (which booleans ship).
- `docs/security.md`: § "Ask Cmdr agent egress" becomes "Cloud AI egress": consent is one toggle in `ai/`, enforced in
  `resolve_backend`, covering every feature; the Ask Cmdr bullets stay as the detailed part.
- `apps/website/src/pages/trust.astro`: replace "The assistant asks for consent before first use" and "These work once a
  cloud provider is set up, without a separate consent step" with one statement that nothing reaches a cloud service
  until the user turns on "Allow cloud AI", which lists what each feature sends; Ollama and LM Studio move from the
  "Local AI" bullet into the cloud one (in the app they're set up under Cloud, and a server can be remote); delete the
  consent-gate DevTodo. Per the page's rule, this must be true of the RELEASED app, so it ships with or after the
  release carrying this change.
- `apps/website/src/pages/privacy-policy.astro`: add one sentence to the AI bullet ("Cloud AI stays off until you allow
  it in Settings, where Cmdr lists what each feature sends."). David reviews; legal text.
- `docs/specs/index.md`: this entry; the spec is wiped per `docs/specs/DETAILS.md` once shipped and reviewed.

## Check lanes

- While iterating: `pnpm check --fast`, plus `pnpm check desktop-rust-clippy` after any Rust change (not in `--fast`).
- Per milestone: `pnpm check`. Named lanes worth running explicitly when their inputs change: `desktop-bindings-fresh`,
  `desktop-rust-module-cycles`, `desktop-rust-error-string-match`, `desktop-rust-ipc-enum-camelcase`,
  `desktop-rust-tests`, `desktop-svelte-tests`, `desktop-svelte-check`, `desktop-svelte-eslint-typecheck-svelte`,
  `desktop-svelte-e2e-stale-selector`, `desktop-message-keys-fresh`, `desktop-message-keys-unused`,
  `desktop-message-key-naming`, `desktop-i18n-coverage`, `desktop-i18n-parity`, `desktop-i18n-icu`,
  `desktop-i18n-stale`, `desktop-i18n-doc-citations`, `desktop-message-screenshots-fresh`, `docs-reachable`,
  `docs-dead-links`, `website-typecheck`, `website-build`.
- At the end: `pnpm check --include-slow` (runs `desktop-svelte-e2e-playwright` for the Ask Cmdr specs).

## Open questions for David

1. **Onboarding "no AI" also turns cloud consent off** (my pick), so a later switch back to Cloud asks again. It's the
   one place that re-asks outside a version bump, justified because "no AI" is the clearest "no" a user gives and today
   it already revokes Ask Cmdr consent. Keep, or leave cloud consent alone there (strict reading of decision 4)?
2. **Ship `cloudAiConsented` in the analytics heartbeat** as a runtime boolean beside `fdaGranted`? It would show how
   many Cloud users turn the toggle on after the update. The config-shape already ships `askCmdr.enabled` by its "all
   booleans" rule; this one needs an explicit line, and maybe a word on the trust page.
