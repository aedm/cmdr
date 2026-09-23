# Translator screenshots: the gaps still open

The screenshot harness ships and is re-runnable (`pnpm i18n:shots`). How it works lives with the code:
`apps/desktop/src/lib/intl/messages/DETAILS.md` § Screenshots (mechanism, framing rules, direct vs representative),
`docs/guides/i18n.md` § Screenshots (the overview, and how to read a coverage count),
`apps/desktop/src/lib/dialog-gallery/DETAILS.md` § "Two more callers" (the registry-driven dialog pass), and
`apps/desktop/test/e2e-playwright/DETAILS.md` (window ACLs, overlay rules). Each item below is a catalog family with no
honest image yet, why it resists capture, and what closing it takes.

❌ No absolute numbers live here: they went stale twice while the analysis stayed true. Counts and the per-area ranking
come from `apps/desktop/src/lib/intl/messages/screenshots/coverage-report.md`, which every coupler run rewrites. Read it
before picking an item up. Native keys (`menu.*`, the window title, the already-running alert) are not a gap: the OS draws
them, so no webview capture can reach them, and their `@key` descriptions carry the context instead.

## 1. Screenshot the image-indexing settings panel with indexing on

- **Problem**: `settings.mediaIndex` is the biggest uncoupled cluster by a wide margin. The capture
  `settings-indexing-image-indexing` runs with image indexing OFF, so only the few keys above the master toggle show. The
  panel body never renders: the CLIP model card and its download/delete states, the scope and chosen-folders editor, the
  importance-threshold slider with its buckets and preview lines, per-volume rows, and the progress and reclaim lines.
  `fileExplorer.imageIndex` is the same story from the pane side.
- **Impact**: translators get no picture for the largest block of settings copy.
- **Solution**: a capture pass that turns image indexing on and walks the panel's states.
  `apps/desktop/test/e2e-playwright/image-index-settings.spec.ts` already reaches several of them and is the place to
  start.
- **Size**: M (more staging than any existing settings surface).
- **Blocked on**: nothing.

## 2. Screenshot the sidebar's status, error, and connection states

- **Problem**: in `fileExplorer.navigation`, the pane volume chooser is captured, so group headings are covered, but
  every state the capture never stages is not. Four clusters: per-drive index status (`driveIndex.*`: freshness
  tooltips, the coalesced-changes and unreadable-spots variants, the context menu's enable/rescan/disable/stop/forget
  items, the footer, and every refusal toast), SMB connection (direct-connect tooltips and their in-flight states, the
  saved-password prompt), favorites and reachability (the empty state, rename/remove/reorder failure toasts, the
  unreachable-location toast), and disk space and hardware (the retry ladder on a failed space fetch, USB
  negotiated-speed labels).
- **Impact**: a large family of sidebar copy, mostly error states, that translators see only as text.
- **Solution**: stage each state for the capture: a specific index state on a specific drive, a failing favorites
  write, a failed space fetch, and so on. Start with `driveIndex.*`, the largest of the four. Each cluster can land on
  its own.
- **Size**: M overall (S per cluster).
- **Blocked on**: nothing.

## 3. Give the Ask Cmdr rail's fake LLM a script that calls a tool

- **Problem**: the E2E fake LLM's rail script (`scripted_fake_llm` in `apps/desktop/src-tauri/src/agent/chat/session.rs`)
  is a single `Say` turn, so no tool row ever renders. Uncaptured as a result: the per-tool progress lines
  (`askCmdr.tool.*`) and the rename-undo affordance (`askCmdr.renameUndo.*`), the two largest uncoupled families after
  `settings.mediaIndex`, plus provider-failure copy (`askCmdr.error.*`), the threads panel's management states
  (`askCmdr.sessions.*`), and the proactive agent's additions (`askCmdr.decision.*`, `askCmdr.wakeDigest.*`).
- **Impact**: most of the agent's visible copy has no screenshot for translators.
- **Solution**: copy the wake slot's pattern: it has three scripts selected by `test_mode::wake_fake_script`, one of
  which calls a tool. Add a selectable rail script (a rename plan is the fullest one), keeping the default rail reply's
  "test assistant" phrasing, which `ask-cmdr.spec.ts` matches on. The consent-screen screenshots
  (`askCmdr.consent.*`) are a separate chore owned by the wake-loop follow-ups in `docs/specs/later/ai/`.
- **Size**: M.
- **Blocked on**: nothing.

## 4. Capture the settings pages the capture never visits

- **Problem**: several settings surfaces are missing from `SETTINGS_SECTIONS`
  (`apps/desktop/test/e2e-playwright/i18n-capture-surfaces.ts`):
  - `settings.summary`: the parent summary grids. The capture passes each full section path so it lands on real content,
    so no summary page is ever photographed.
  - `settings.archives`: the `behavior-archives` section exists in `SettingsContent.svelte` but has no capture row.
  - `settings.appearance` and `settings.tint`: the date/time format options, the custom-format editor with its
    placeholder help, and the tint names, all behind a disclosure or a picker on an otherwise-captured page.
- **Impact**: cheap coverage left on the table.
- **Solution**: one extra surface per top-level parent for the summaries, one row for archives, and an opened
  disclosure or picker state for the appearance and tint strings.
- **Size**: S.
- **Blocked on**: nothing.

## 5. Capture the long tail of trouble states

- **Problem**: the report's long tail, in recurring shapes: `suggestedOps` (the agent's suggested-operations panel, not
  captured at all), the transfer dialog's scan-unresponsive and target-will-be-created variants, the error-reporter
  dialog, licensing's error and section copy, file-operation validation messages, the AI, Advanced, and Behavior
  settings pages, the FDA onboarding step, the downloads shortcut row, and pane states (`fileExplorer.pane`,
  `readOnly`, `clipboard`, `errorPane`, `unreachable`) that only render when a pane is in trouble. A few `queue.*` row
  states (the awaiting-answer tooltip, resume and its aria label, the toolbar's selected count) need more than a
  two-operation queue reaches.
- **Impact**: each is small; together they're a real share of the uncoupled keys.
- **Solution**: pick from the top of the report's ranking; `suggestedOps` first, since it's a whole panel at zero. Skip
  the `queue.*` leftovers unless a surface comes cheap.
- **Size**: M in total, S per surface.
- **Blocked on**: nothing.

## 6. Human-read the representative screenshot notes before a locale ships

- **Problem**: the `@key.screenshotNote` text on every representative coupling is translator-facing first-draft copy,
  generated from the curated table in `apps/desktop/scripts/representative-screenshots.ts`. No human has read it.
- **Impact**: translators may be misled by a note that describes the stand-in image wrong.
- **Solution**: a human read of the table before a locale is marketed as reviewed (principle 4: humans review
  human-facing copy).
- **Size**: S.
- **Blocked on**: David's time; it pairs with the app-wide deferred human review of translations.
