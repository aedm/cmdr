/**
 * E2E for the alpha "Ask Cmdr" chat rail: it toggles open from BOTH the View menu item and
 * the ⌘⌥A keyboard shortcut (with the ALPHA badge), focuses the composer on open and
 * returns focus to the panes on Escape, and streams a reply end-to-end.
 *
 * The send path runs against the deterministic scripted fake LLM (the app launches with
 * `CMDR_E2E_ASK_CMDR_FAKE=1`; see the e2e-playwright check + commands/agent.rs), so the
 * reply text is fixed and no provider is needed. The menu path uses `dispatchMenuCommand`
 * (native menu → execute-command); the shortcut path dispatches a real keydown through
 * `handleGlobalKeyDown` → `lookupCommand('⌘⌥A')` → the `askCmdr.toggle` handler.
 */

import { waitBudget } from './wait-budget.js'
import { test, expect } from './fixtures.js'
import {
  closeScopedWindow,
  dispatchMenuCommand,
  ensureAppReady,
  openSettingsWindowViaProd,
  CTRL_OR_META,
} from './helpers.js'
import type { TauriPage } from '@srsholmes/tauri-playwright'

/** The rail is open once its root element is in the DOM. */
function railOpen(page: TauriPage): Promise<boolean> {
  return page.evaluate<boolean>(`document.querySelector('.ask-cmdr-rail') !== null`)
}

/** The ALPHA stability badge is present in the open rail's header. */
function alphaBadgeShown(page: TauriPage): Promise<boolean> {
  return page.evaluate<boolean>(
    `(document.querySelector('.ask-cmdr-rail .feature-status-badge')?.textContent || '').trim().toLowerCase() === 'alpha'`,
  )
}

/** True when focus is on the rail's composer textarea. */
function composerFocused(page: TauriPage): Promise<boolean> {
  return page.evaluate<boolean>(`document.activeElement === document.querySelector('.ask-cmdr-rail textarea')`)
}

/** True when focus is back inside the dual-pane explorer. */
function paneFocused(page: TauriPage): Promise<boolean> {
  return page.evaluate<boolean>(
    `document.querySelector('.dual-pane-explorer')?.contains(document.activeElement) === true`,
  )
}

/** Dispatches the default ⌘⌥A keydown (Ctrl+Alt+A on Linux). */
function pressToggleShortcut(page: TauriPage): Promise<void> {
  return page.evaluate(`document.dispatchEvent(new KeyboardEvent('keydown', {
        key: 'a', code: 'KeyA',
        ctrlKey: ${String(CTRL_OR_META === 'Control')},
        metaKey: ${String(CTRL_OR_META === 'Meta')},
        altKey: true, bubbles: true
    }))`)
}

/** Opens the rail via the View-menu toggle, re-dispatching inside the poll: the
 * cross-source double-fire guard (dispatch-dedup.ts, 300ms) can drop a menu fire that
 * lands right after a prior test's toggle. Idempotent once open. */
async function openRailViaMenu(page: TauriPage): Promise<void> {
  await expect
    .poll(
      async () => {
        if (await railOpen(page)) return true
        await dispatchMenuCommand(page, 'askCmdr.toggle')
        return railOpen(page)
      },
      { timeout: waitBudget(5000) },
    )
    .toBe(true)
}

/** Closes the rail via its header close button, if open, so state doesn't leak between tests. */
async function closeRailIfOpen(page: TauriPage): Promise<void> {
  if (!(await railOpen(page))) return
  await page.evaluate(`document.querySelector('.ask-cmdr-rail .header-actions button:last-child')?.click()`)
  await expect.poll(() => railOpen(page), { timeout: waitBudget(3000) }).toBe(false)
}

/** The rail's "Ask Cmdr is off" gate is showing (the rail is open, the chat isn't). */
function offGateShown(page: TauriPage): Promise<boolean> {
  return page.evaluate<boolean>(`document.querySelector('.ask-cmdr-rail .ask-cmdr-gate[data-gate="off"]') !== null`)
}

/** The composer is present (the rail is past its gates). */
function composerPresent(page: TauriPage): Promise<boolean> {
  return page.evaluate<boolean>(`document.querySelector('.ask-cmdr-rail textarea') !== null`)
}

/** Get the rail ready for a chat test: turn Ask Cmdr on from the rail's gate if it's showing
 * (the switch persists in `settings.json` for the run, so it's a no-op once on), then wait for
 * the composer to render. The gate resolves asynchronously on open, so the composer isn't
 * present on the first tick even when already on — always wait. The E2E fake keeps
 * `ai.provider` off, so the cloud gate never shows here. */
async function ensureChatReady(page: TauriPage): Promise<void> {
  await expect
    .poll(
      async () => {
        if (await composerPresent(page)) return true
        if (await offGateShown(page)) {
          await page.evaluate(
            `document.querySelector('.ask-cmdr-rail .ask-cmdr-gate[data-gate="off"] button')?.click()`,
          )
        }
        return composerPresent(page)
      },
      { timeout: waitBudget(5000) },
    )
    .toBe(true)
}

/** The Ask Cmdr switch in Settings > AI > Ask Cmdr: its hidden input carries the label. */
const ASK_CMDR_SWITCH = '[aria-label="Turn on Ask Cmdr"]'

/** The switch's state as Ark draws it (`checked` / `unchecked`), or `missing`. */
function askCmdrSwitchStateJs(): string {
  return `(function() {
    var input = document.querySelector('${ASK_CMDR_SWITCH}');
    if (!input) return 'missing';
    var root = input.closest('[data-scope="switch"][data-part="root"]');
    var control = root ? root.querySelector('.switch-control') : null;
    return (control || input).getAttribute('data-state') || 'unknown';
  })()`
}

/** Sets Ask Cmdr's switch through the Settings window, the way a user does, and closes it. */
async function setAskCmdrEnabledInSettings(main: TauriPage, on: boolean): Promise<void> {
  const settings = await openSettingsWindowViaProd(main)
  try {
    await settings.waitForSelector('.settings-sidebar', waitBudget(3000))
    await settings.evaluate(`(function() {
      var items = document.querySelectorAll('.section-item');
      for (var i = 0; i < items.length; i++) {
        if ((items[i].textContent || '').trim() === 'Ask Cmdr') { items[i].click(); return; }
      }
    })()`)
    await settings.waitForSelector(ASK_CMDR_SWITCH, waitBudget(3000))
    const wanted = on ? 'checked' : 'unchecked'
    if ((await settings.evaluate<string>(askCmdrSwitchStateJs())) !== wanted) {
      await settings.evaluate(`document.querySelector('${ASK_CMDR_SWITCH}')?.click()`)
    }
    await expect
      .poll(() => settings.evaluate<string>(askCmdrSwitchStateJs()), { timeout: waitBudget(3000) })
      .toBe(wanted)
  } finally {
    await closeScopedWindow(main, settings, 'settings')
  }
}

/** Count of completed fake assistant replies currently in the thread. */
function assistantReplyCount(page: TauriPage): Promise<number> {
  return page.evaluate<number>(
    `[...document.querySelectorAll('.ask-cmdr-rail .msg')].filter(m => (m.textContent||'').includes('test assistant')).length`,
  )
}

/** Types `text` into the composer and presses Enter, then waits for ONE NEW fake reply to
 * land. Count-based (not "any reply present"), so it doesn't pass instantly on a reply
 * already shown from a bootstrapped thread. */
async function sendComposerMessage(page: TauriPage, text: string): Promise<void> {
  const before = await assistantReplyCount(page)
  await page.evaluate(`(() => {
    const ta = document.querySelector('.ask-cmdr-rail textarea');
    ta.focus();
    ta.value = ${JSON.stringify(text)};
    ta.dispatchEvent(new Event('input', { bubbles: true }));
    ta.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  })()`)
  await expect.poll(() => assistantReplyCount(page), { timeout: waitBudget(8000) }).toBeGreaterThan(before)
}

/** How many thread rows the open sessions panel shows. */
function sessionRowCount(page: TauriPage): Promise<number> {
  return page.evaluate<number>(`document.querySelectorAll('.ask-cmdr-rail .sessions .row').length`)
}

test.describe('Ask Cmdr rail', () => {
  test.describe.configure({ timeout: waitBudget(30000) })

  test.beforeEach(async ({ tauriPage }) => {
    await ensureAppReady(tauriPage)
    await closeRailIfOpen(tauriPage as TauriPage)
  })

  test.afterEach(async ({ tauriPage }) => {
    await closeRailIfOpen(tauriPage as TauriPage)
  })

  // The rail opens to its "Ask Cmdr is off" gate while the switch is off, and the gate's
  // button turns it on and unlocks the chat.
  //
  // ⚠️ It turns the switch OFF first (through Settings, the way a user does) rather than
  // assuming a fresh profile: `settings.json` outlives every test in the shard, and
  // `ask-cmdr-wake.spec.ts` sorts ahead of this one (`-` before `.`) and turns Ask Cmdr on to
  // run a wake at all. MCP `set_setting` can't reach `askCmdr.enabled` (`mcpSettable: false`).
  test('gates on Ask Cmdr being off, and turning it on unlocks the chat', async ({ tauriPage }) => {
    const page = tauriPage as TauriPage
    await setAskCmdrEnabledInSettings(page, false)
    await openRailViaMenu(page)
    // The gate is shown; the composer is not reachable yet.
    await expect.poll(() => offGateShown(page), { timeout: waitBudget(3000) }).toBe(true)
    expect(await composerPresent(page)).toBe(false)

    // The gate's button turns Ask Cmdr on and unlocks the composer.
    await page.evaluate(`document.querySelector('.ask-cmdr-rail .ask-cmdr-gate[data-gate="off"] button')?.click()`)
    await expect.poll(() => composerPresent(page), { timeout: waitBudget(3000) }).toBe(true)
    expect(await offGateShown(page)).toBe(false)
  })

  test('opens from the View menu item with the ALPHA badge', async ({ tauriPage }) => {
    const page = tauriPage as TauriPage
    await dispatchMenuCommand(page, 'askCmdr.toggle')
    await expect.poll(() => railOpen(page), { timeout: waitBudget(3000) }).toBe(true)
    expect(await alphaBadgeShown(page)).toBe(true)
  })

  test('opens from the ⌘⌥A keyboard shortcut', async ({ tauriPage }) => {
    const page = tauriPage as TauriPage
    // Re-press inside the poll: the cross-source double-fire guard (dispatch-dedup.ts,
    // 300ms) can drop a keyboard fire right after a menu fire from a prior test. The
    // toggle is idempotent while open, so extra presses after it opens are harmless.
    await expect
      .poll(
        async () => {
          if (await railOpen(page)) return true
          await pressToggleShortcut(page)
          return railOpen(page)
        },
        { timeout: waitBudget(5000) },
      )
      .toBe(true)
  })

  test('focuses the composer on open and returns focus to the panes on Escape', async ({ tauriPage }) => {
    const page = tauriPage as TauriPage
    await openRailViaMenu(page)
    await ensureChatReady(page)
    await expect.poll(() => composerFocused(page), { timeout: waitBudget(3000) }).toBe(true)

    // Escape on the focused composer returns focus to the active pane.
    await page.evaluate(
      `document.querySelector('.ask-cmdr-rail textarea')?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))`,
    )
    await expect.poll(() => paneFocused(page), { timeout: waitBudget(3000) }).toBe(true)
  })

  test('sends a message and streams the fake reply', async ({ tauriPage }) => {
    const page = tauriPage as TauriPage
    await openRailViaMenu(page)
    await ensureChatReady(page)

    // Type into the composer and press Enter (Svelte's bind:value listens for input).
    await page.evaluate(`(() => {
      const ta = document.querySelector('.ask-cmdr-rail textarea');
      ta.focus();
      ta.value = 'hello there';
      ta.dispatchEvent(new Event('input', { bubbles: true }));
      ta.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    })()`)

    // The scripted fake streams "Hi! I'm the test assistant." into the assistant bubble.
    await expect
      .poll(
        () =>
          page.evaluate<boolean>(
            `(document.querySelector('.ask-cmdr-rail')?.textContent || '').includes('test assistant')`,
          ),
        { timeout: waitBudget(8000) },
      )
      .toBe(true)
  })

  test('creates two threads, searches, and switches to the match', async ({ tauriPage }) => {
    const page = tauriPage as TauriPage
    // A per-run nonce keeps the search from matching threads left by earlier runs.
    const nonce = `n${String(Date.now())}`
    const alpha = `${nonce} budget spreadsheet`
    const beta = `${nonce} grocery list`

    await openRailViaMenu(page)
    await ensureChatReady(page)

    // Start from a fresh chat: the rail may have bootstrapped a prior test's thread, so
    // without this the first send would append to it (and its reply would satisfy the
    // send-wait early). New-chat gives thread one its own title.
    await page.evaluate(`document.querySelector('.ask-cmdr-rail [aria-label="New chat"]')?.click()`)
    await sendComposerMessage(page, alpha)
    // Start a fresh thread again, then thread two.
    await page.evaluate(`document.querySelector('.ask-cmdr-rail [aria-label="New chat"]')?.click()`)
    await sendComposerMessage(page, beta)

    // Open the sessions panel; both threads are listed.
    await page.evaluate(`document.querySelector('.ask-cmdr-rail [aria-label="Chats"]')?.click()`)
    await expect.poll(() => sessionRowCount(page), { timeout: waitBudget(3000) }).toBeGreaterThanOrEqual(2)

    // Search for the first thread's distinctive word; only it matches.
    await page.evaluate(`(() => {
      const input = document.querySelector('.ask-cmdr-rail .sessions input.text-field-control');
      input.focus();
      input.value = ${JSON.stringify(`${nonce} budget`)};
      input.dispatchEvent(new Event('input', { bubbles: true }));
    })()`)
    await expect
      .poll(
        () =>
          page.evaluate<string>(
            `[...document.querySelectorAll('.ask-cmdr-rail .sessions .row-title')].map(r => r.textContent).join('|')`,
          ),
        { timeout: waitBudget(5000) },
      )
      .toContain('budget')

    // Exactly one result, and it's the budget thread (not the grocery one).
    const titles = await page.evaluate<string>(
      `[...document.querySelectorAll('.ask-cmdr-rail .sessions .row-title')].map(r => r.textContent).join('|')`,
    )
    expect(titles).toContain('budget')
    expect(titles).not.toContain('grocery')

    // Click the result → it switches the rail to that thread and closes the panel.
    await page.evaluate(`document.querySelector('.ask-cmdr-rail .sessions .row')?.click()`)
    await expect
      .poll(() => page.evaluate<boolean>(`document.querySelector('.ask-cmdr-rail .sessions') === null`), {
        timeout: waitBudget(3000),
      })
      .toBe(true)
    await expect
      .poll(
        () =>
          page.evaluate<boolean>(`(document.querySelector('.ask-cmdr-rail')?.textContent || '').includes('budget')`),
        { timeout: waitBudget(3000) },
      )
      .toBe(true)
  })
})
