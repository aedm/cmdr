# Managed policy (frontend): details

The key catalog, parse rules, and enforcement live in `src-tauri/src/managed_policy/DETAILS.md`. This file covers how a
window shows the backend's answer.

## Data flow

- `initializeSettings()` (full windows only) awaits `initManagedPolicy(applyPolicyChange)` before it marks the store
  initialized. `initManagedPolicy` subscribes to `managed-policy-changed` FIRST, then fetches `get_managed_policy`; a
  change that lands while the fetch is out wins over the fetch's answer (it's at least as new).
- A failed fetch leaves the window on `UNMANAGED` with a warning. The UI then shows no lock, and the backend still
  refuses every send, check, and AI call the policy rules out.
- On a change, the view swaps and the settings store's `applyPolicyChange({ previous, next })` notifies each setting
  whose EFFECTIVE value moved, once, through the ordinary `onSettingChange` / `onSpecificSettingChange` listeners. So
  `settings-applier.ts` re-pushes (for example `ai.*` through `pushConfigToBackend()`), and nothing is persisted or
  emitted cross-window: every window gets the backend event itself.

## The settings store under a lock

- `getSetting(id)` returns `lockedValue(lock, stored-or-default)`: a `fixed` lock's value, or a `disallowedValues`
  lock's `fallback` when the stored value is one it rules out.
- `setSetting` refuses (logs at info, writes nothing, notifies nobody) a value `lockAllowsWrite` rules out. The row is
  disabled and MCP refuses in Rust first, so this is the backstop.
- `resetSetting` refuses a `fixed` setting, so the person's stored choice survives for when the profile goes away. A
  narrowed setting resets normally.
- `isModified` compares the STORED value with the default, so a lock never makes a row look modified.
- `isOverriddenByPolicy(id)` says the lock changed what `id` reads. A flow that preselects from a read and writes the
  answer back (onboarding's AI step) must skip the write unless the person picked the value, or the policy's display
  lands in `settings.json` as their choice.
- A cross-window `settings:changed` for a locked id notifies only when the effective value moved.

## The UI

- `SettingRow` and every row primitive read `isSettingLocked(id)` themselves (`settings/components/DETAILS.md` § Managed
  rows): disabled, the managed note in place of any section-passed note or badge, no reset pip, and the control's
  `aria-describedby` pointing at that note.
- `SettingsSection` shows "Your organization manages some of these settings." at its top when any row it holds is
  managed (pinned or narrowed). Rows register with the nearest section through context (`section-rows.svelte.ts`), so no
  section keeps a list of its lockable ids.
- MCP: the `cmdr://settings` YAML marks a managed setting `managed: true` beside its effective `value`, and the bridge
  refuses a ruled-out write with `refusal: 'managedByOrganization'` (the backend's own refusal comes first,
  `src-tauri/src/mcp/DETAILS.md`).

- AI: `AiSection`'s provider radios disable what `lockAllowsWrite` rules out; a narrowed lock adds a visible line under
  the row (`settings-ai-provider-managed`). The consent switch locks from `CloudAiConsentStatus.managed`, the service
  pickers from `cloud_ai_hosts_allowed` (`$lib/ai-provider-setup/DETAILS.md`), onboarding skips step 2 when the lock
  leaves only "no AI" (`$lib/onboarding/DETAILS.md`), and Ask Cmdr's provider hint reads `ai.mode === 'off'` (a feature
  state, not a lock). Every refusal sentence comes from `ai-refusal.ts`.

## Decisions

- **The fetch lives in `initializeSettings`, not `initWindowSettings`.** The main window's layout reaches the store
  through `initReactiveSettings()` directly, and applying the policy before `initialized` is what guarantees no consumer
  (the updater's first `updates.autoCheck` read, the first `pushConfigToBackend()`) ever sees the unlocked value. Every
  full window still gets it through `initWindowSettings()`.
- **Pinned and narrowed render differently.** Disabling `ai.provider` whole under `DisableCloudAI` would also take away
  `local` and `off`, which the policy allows; the provider control rules out the `cloud` option itself.
