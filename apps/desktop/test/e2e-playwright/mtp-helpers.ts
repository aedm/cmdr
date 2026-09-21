/**
 * Shared setup and vocabulary for the virtual-MTP specs (`mtp.spec.ts`,
 * `mtp-rename.spec.ts`, `mtp-transfers.spec.ts`, `mtp-refusals.spec.ts`).
 *
 * They all run on the same sequential MTP shard against one virtual device, so they
 * need the identical per-test reset: recreate the local fixtures, sync the device's
 * object tree, and park both panes on the local volume. `installMtpSpecSetup()`
 * registers that, plus the file-scope timeout that MTP's protocol overhead needs.
 * Each spec calls it itself, so a spec states its own preconditions instead of
 * inheriting whatever ran before it on the shard.
 *
 * Requires the app to be built with `--features playwright-e2e,virtual-mtp`.
 */

import os from 'os'
import fs from 'fs'

import { test, expect } from './fixtures.js'
import { waitBudget } from './wait-budget.js'
import { restoreFixtureTree } from '../e2e-shared/fixture-manifest.js'
import { recreateFixtures } from '../e2e-shared/fixtures.js'
import { recreateMtpFixtures } from '../e2e-shared/mtp-fixtures.js'
import { initMcpClient } from '../e2e-shared/mcp-client.js'
import { getFixtureRoot, isStateClean } from './helpers.js'

import type { TauriPage, BrowserPageAdapter } from '@srsholmes/tauri-playwright'
export type PageLike = TauriPage | BrowserPageAdapter

// Volume names (verified from manual testing against the virtual device)
export const INTERNAL_STORAGE = 'Virtual Pixel 9 - Internal Storage'
export const SD_CARD = 'Virtual Pixel 9 - SD Card'

// Local volume name differs by platform (macOS: "Macintosh HD", Linux: "Root")
export const LOCAL_VOLUME_NAME = os.platform() === 'linux' ? 'Root' : 'Macintosh HD'

/** Returns the size of a file, or -1 if it doesn't exist / can't be statted. */
export function safeFileSize(p: string): number {
  try {
    return fs.statSync(p).size
  } catch {
    return -1
  }
}

/**
 * Returns true when both panes show the local volume. Reads the DOM directly
 * via a single tauri-playwright `evaluate` instead of going through MCP's
 * `cmdr://state` resource: the latter is one HTTP roundtrip per poll
 * iteration, which on a paranoid 100 ms poll loop dominates the wait time
 * even after the panes have already flipped. The breadcrumb's
 * `.volume-breadcrumb .volume-name` element holds the current label.
 */
export async function bothPanesOnLocalVolume(tauriPage: PageLike): Promise<boolean> {
  return tauriPage.evaluate<boolean>(
    `(function(){
      var els = document.querySelectorAll('.volume-breadcrumb .volume-name');
      var name = ${JSON.stringify(LOCAL_VOLUME_NAME)};
      if (els.length < 2) return false;
      for (var i = 0; i < 2; i++) {
        var t = (els[i].textContent || '').trim();
        if (t !== name) return false;
      }
      return true;
    })()`,
  )
}

/**
 * Sets `.rename-input`'s value directly via the native setter so Svelte's
 * reactivity sees a single update. Used instead of `tauriPage.type` because
 * per-character key dispatch over the playwright socket costs ~14 s for an
 * 18-char name on Linux/Xvfb (vs ~80 ms on macOS); the slow path made
 * `MTP rename via keyboard` the worst per-test Linux outlier (5.1×).
 */
export async function setRenameInputValue(page: PageLike, value: string): Promise<void> {
  await page.evaluate(
    `(function() {
      var input = document.querySelector('.rename-input');
      if (!input) return;
      input.focus();
      var desc = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value');
      if (desc && desc.set) desc.set.call(input, ${JSON.stringify(value)});
      else input.value = ${JSON.stringify(value)};
      input.dispatchEvent(new Event('input', { bubbles: true }));
    })()`,
  )
  await expect
    .poll(
      async () => page.evaluate<boolean>(`document.querySelector('.rename-input')?.value === ${JSON.stringify(value)}`),
      { timeout: waitBudget(3000) },
    )
    .toBeTruthy()
}

/**
 * Registers the per-test reset every MTP spec needs, plus the file-scope timeout.
 * Call it once at module scope, above the `test.describe` blocks.
 */
export function installMtpSpecSetup(): void {
  // MTP operations go through the virtual device which adds protocol overhead.
  // 30s default is too tight for multi-step MTP test chains.
  test.setTimeout(waitBudget(120_000))

  test.beforeEach(async ({ tauriPage }) => {
    recreateFixtures(getFixtureRoot()) // Local fixtures for cross-storage tests
    await initMcpClient(tauriPage) // Discover MCP port

    // Pause the watcher, recreate fixtures on disk, then sync the virtual
    // device's object tree via an explicit rescan. The watcher stays PAUSED
    // (we never resume it here) so late FSEvents from the wipe+recreate can't
    // race the test by removing freshly rescanned handles. See
    // `src-tauri/src/mtp/DETAILS.md` § "Virtual device watcher in E2E".
    await tauriPage.evaluate(`window.__TAURI_INTERNALS__.invoke('pause_virtual_mtp_watcher')`)
    recreateMtpFixtures()
    await tauriPage.evaluate(`window.__TAURI_INTERNALS__.invoke('rescan_virtual_mtp')`)

    // Force both panes back to a local volume. Previous tests may have left a pane
    // on MTP, and ensureAppReady's mcp-nav-to-path events get rejected by
    // navigateToPath when the pane is on an MTP volume (it requires select_volume first).
    // Volume name differs by platform: "Macintosh HD" on macOS, "Root" on Linux.
    //
    // Short-circuit: if both panes are already on the local volume AND no modal
    // overlay is lingering, skip the volume-select + Escape sequence. This is the
    // common case for non-first tests in the spec.
    if (!(await isStateClean(tauriPage, LOCAL_VOLUME_NAME))) {
      await tauriPage.evaluate(`(function() {
          var invoke = window.__TAURI_INTERNALS__.invoke;
          invoke('plugin:event|emit', { event: 'mcp-volume-select', payload: { pane: 'left', name: '${LOCAL_VOLUME_NAME}' } });
          invoke('plugin:event|emit', { event: 'mcp-volume-select', payload: { pane: 'right', name: '${LOCAL_VOLUME_NAME}' } });
      })()`)
      // Wait for both panes to show the local volume. Tight 25 ms poll interval
      // since the DOM read is sub-ms; the wait then exits ~one frame after the
      // panes flip, instead of 50–100 ms of poll latency.
      await expect
        .poll(async () => bothPanesOnLocalVolume(tauriPage), {
          timeout: waitBudget(5000),
          intervals: [10, 25, 50, 100],
        })
        .toBeTruthy()
    }
  })

  // Putting the shared `left/` + `right/` tree back is this spec's job: the
  // post-test leak guard fails whoever leaves it dirty, and the restore is
  // surgical, so it only rewrites what actually drifted.
  test.afterEach(() => {
    restoreFixtureTree(getFixtureRoot())
  })
}
