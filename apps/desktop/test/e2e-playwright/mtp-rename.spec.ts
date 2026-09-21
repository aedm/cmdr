/**
 * E2E tests for renaming on an MTP device: the keyboard flow, a folder entered
 * from its row, and the existing-name conflict.
 *
 * Requires the app to be built with `--features playwright-e2e,virtual-mtp`.
 * Shared setup and vocabulary: `mtp-helpers.ts`.
 */

import fs from 'fs'
import path from 'path'

import { test, expect } from './fixtures.js'
import { waitBudget } from './wait-budget.js'
import { MTP_FIXTURE_ROOT } from '../e2e-shared/mtp-fixtures.js'
import {
  mcpCall,
  getMtpVolumePath,
  mcpOpenMtpStorageRoot,
  mcpNavToPath,
  mcpAwaitItem,
} from '../e2e-shared/mcp-client.js'
import { ensureAppReady, fileExistsInPane, moveCursorToFile } from './helpers.js'
import { INTERNAL_STORAGE, installMtpSpecSetup, setRenameInputValue } from './mtp-helpers.js'

installMtpSpecSetup()

test.describe('MTP rename', () => {
  test('renames file on MTP via keyboard', async ({ tauriPage }) => {
    await ensureAppReady(tauriPage)
    const mtpPath = await getMtpVolumePath(INTERNAL_STORAGE)

    // Navigate left pane to MTP Documents
    await mcpOpenMtpStorageRoot('left', INTERNAL_STORAGE)
    await mcpAwaitItem('left', 'Documents')
    await mcpNavToPath('left', `${mtpPath}/Documents`)
    await mcpAwaitItem('left', 'report.txt')

    // Move cursor to report.txt via DOM (keyboard flow test)
    await moveCursorToFile(tauriPage, 'report.txt')

    // Press F2 to start rename
    await tauriPage.keyboard.press('F2')
    await tauriPage.waitForSelector('.rename-input', 10000)

    // Set the new value directly. `tauriPage.type` dispatches one key event
    // per character over the playwright socket, which adds ~14 s on Linux/Xvfb
    // (vs ~80 ms on macOS) for an 18-char name. The setter+input event hits
    // the same Svelte reactivity path that user typing does.
    await setRenameInputValue(tauriPage, 'renamed-report.txt')
    await tauriPage.press('.rename-input', 'Enter')

    // Wait for rename input to disappear
    await expect
      .poll(async () => !(await tauriPage.isVisible('.rename-input')), { timeout: waitBudget(10000) })
      .toBeTruthy()

    // Verify new name appears, old name gone
    await expect
      .poll(async () => fileExistsInPane(tauriPage, 'renamed-report.txt', 0), { timeout: waitBudget(10000) })
      .toBeTruthy()
    await expect
      .poll(async () => !(await fileExistsInPane(tauriPage, 'report.txt', 0)), { timeout: waitBudget(5000) })
      .toBeTruthy()

    // Verify on backing dir
    expect(fs.existsSync(path.join(MTP_FIXTURE_ROOT, 'internal', 'Documents', 'renamed-report.txt'))).toBe(true)
    expect(fs.existsSync(path.join(MTP_FIXTURE_ROOT, 'internal', 'Documents', 'report.txt'))).toBe(false)
  })

  // The route a user takes: Enter on a folder row, which lands the pane on the
  // row's inner path (`/Documents`) rather than the storage URL every other
  // spec here navigates to. A Cmdr-made change is reported at the URL, so this
  // pane only hears about it if the listing cache keys both spellings as one
  // (`listing/cached_listing.rs`, `ListingPath`). Field reports ERR-QW42X and
  // ERR-46A6B: the pane kept showing deleted files, and users acted on them
  // again. A rename is the op that pins it here: the virtual device, like a real
  // Android phone, sends no event for it, while a delete or move queues one
  // whose whole-device refresh would mask the miss.
  test('a folder entered with Enter shows a rename made in it', async ({ tauriPage }) => {
    await ensureAppReady(tauriPage)

    await mcpOpenMtpStorageRoot('left', INTERNAL_STORAGE)
    await mcpAwaitItem('left', 'Documents')

    expect(await moveCursorToFile(tauriPage, 'Documents')).toBe(true)
    // `open_under_cursor` runs the command the Enter key is bound to
    // (`navOpenUnderCursorCommand`), so the pane takes Enter's route to the row's
    // own path. ❌ Not `pressKey('Enter')`: a synthesized press is dispatched on
    // `document.activeElement`, and here it opened the first folder (`DCIM`)
    // with the cursor confirmed on `Documents`.
    await mcpCall('open_under_cursor', {})
    // The premise this spec exists for: Enter lands on the row's own inner path,
    // not on the storage URL.
    await mcpCall('await', { pane: 'left', condition: 'path', value: '/Documents', timeoutSeconds: 15 })
    await mcpAwaitItem('left', 'report.txt')

    expect(await moveCursorToFile(tauriPage, 'report.txt')).toBe(true)
    await tauriPage.keyboard.press('F2')
    await tauriPage.waitForSelector('.rename-input', 10000)
    await setRenameInputValue(tauriPage, 'entered-renamed.txt')
    await tauriPage.press('.rename-input', 'Enter')
    await expect
      .poll(async () => !(await tauriPage.isVisible('.rename-input')), { timeout: waitBudget(10000) })
      .toBeTruthy()

    // No `refresh` here on purpose: an explicit re-read would hide exactly the
    // miss this pins.
    await expect
      .poll(async () => fileExistsInPane(tauriPage, 'entered-renamed.txt', 0), { timeout: waitBudget(10000) })
      .toBeTruthy()
    await expect
      .poll(async () => !(await fileExistsInPane(tauriPage, 'report.txt', 0)), { timeout: waitBudget(5000) })
      .toBeTruthy()

    expect(fs.existsSync(path.join(MTP_FIXTURE_ROOT, 'internal', 'Documents', 'entered-renamed.txt'))).toBe(true)
  })

  test('rename to existing name is rejected on MTP', async ({ tauriPage }) => {
    await ensureAppReady(tauriPage)
    const mtpPath = await getMtpVolumePath(INTERNAL_STORAGE)

    await mcpOpenMtpStorageRoot('left', INTERNAL_STORAGE)
    await mcpAwaitItem('left', 'Documents')
    await mcpNavToPath('left', `${mtpPath}/Documents`)
    await mcpAwaitItem('left', 'report.txt')

    await moveCursorToFile(tauriPage, 'report.txt')
    await tauriPage.keyboard.press('F2')
    await tauriPage.waitForSelector('.rename-input', 10000)

    await setRenameInputValue(tauriPage, 'notes.txt')
    await tauriPage.press('.rename-input', 'Enter')

    // Conflict dialog should appear since notes.txt already exists
    await tauriPage.waitForSelector('[data-dialog-id="rename-conflict"]', 10000)
    const dialogText = await tauriPage.evaluate(
      `document.querySelector('[data-dialog-id="rename-conflict"]')?.textContent ?? ''`,
    )
    expect(dialogText).toContain('already exists')

    // Cancel the dialog. Both files should remain unchanged.
    //
    // We click the dialog's Cancel button explicitly rather than pressing
    // Escape via the OS keyboard: Escape on the conflict dialog routes through
    // `onclose` -> `onResolve('continue')`, which keeps the rename input alive
    // for re-edit (the "continue editing" path). 'cancel' is the only resolution
    // that closes the whole rename flow, and the Cancel button is the only path
    // that dispatches it. Clicking it sidesteps the focus-state ambiguity that
    // `tauriPage.keyboard.press` introduces after Enter on the rename input.
    await tauriPage.evaluate(
      `(function(){
        var dlg = document.querySelector('[data-dialog-id="rename-conflict"]');
        if (!dlg) throw new Error('rename-conflict dialog not present');
        var btn = Array.from(dlg.querySelectorAll('button')).find(b => b.textContent && b.textContent.trim() === 'Cancel');
        if (!btn) throw new Error('Cancel button not found in rename-conflict dialog');
        btn.click();
      })()`,
    )
    await expect
      .poll(async () => !(await tauriPage.isVisible('.modal-overlay')), { timeout: waitBudget(3000) })
      .toBeTruthy()

    expect(fs.existsSync(path.join(MTP_FIXTURE_ROOT, 'internal', 'Documents', 'report.txt'))).toBe(true)
    expect(fs.existsSync(path.join(MTP_FIXTURE_ROOT, 'internal', 'Documents', 'notes.txt'))).toBe(true)
  })
})
