/**
 * Shared primitives for the two server specs (`server-ops-sftp.spec.ts`,
 * `server-ops-webdav.spec.ts`): the add sheet, the switcher row a saved server
 * gets, and the pane that browses it. Both specs run the same scenarios through
 * `defineServerOpsSuite`, so SFTP and WebDAV can't drift into testing different
 * things.
 *
 * ❗ Everything a person does goes through the UI: the Go menu's "Connect to
 * server…" (the `servers.connect` command, ⌘K), the sheet's own fields and
 * buttons, the volume switcher's rows and their → submenu, F-keys for the file
 * operations. The MCP client only moves the cursor and reads state, and every
 * server-side assertion goes through `server-fixtures.ts`'s side door rather
 * than through the app being tested.
 *
 * ❗ The app is SHARED by every spec on the shard, so a saved server is state
 * that outlives a test. Each suite forgets its own server in `afterAll`, and the
 * suite's scratch directory is unique per run, because other lanes and
 * worktrees lease the same fixture server.
 */

import { waitBudget } from './wait-budget.js'
import fs from 'fs'
import path from 'path'
import type { TauriPage, BrowserPageAdapter } from '@srsholmes/tauri-playwright'
import { expect } from './fixtures.js'
import { dispatchMenuCommand, escapeOverlayUntilGone, focusPane, pointerClick, pressKey } from './helpers.js'
import { mcpReadResource } from '../e2e-shared/mcp-client.js'
import { SERVER_PASSWORD, SERVER_USERNAME, type ServerFixture } from '../e2e-shared/server-fixtures.js'

export type PageLike = TauriPage | BrowserPageAdapter

/** The one sign-in sheet, wherever it opens from. */
export const SHEET = '[data-dialog-id="server-sign-in"]'

// ── The add sheet ────────────────────────────────────────────────────────────

/** Opens the add sheet the way the Go menu's "Connect to server…" (⌘K) does. */
export async function openAddServerSheet(tauriPage: PageLike): Promise<void> {
  await dispatchMenuCommand(tauriPage, 'servers.connect')
  await expect.poll(async () => tauriPage.isVisible(SHEET), { timeout: waitBudget(5000) }).toBeTruthy()
}

/** Replaces one of the sheet's text fields, the way a person retyping it would. */
export async function setSheetField(tauriPage: PageLike, id: string, value: string): Promise<void> {
  await tauriPage.evaluate(`(function () {
    var input = document.querySelector('${SHEET} #${id}');
    if (!input) throw new Error('sheet field #${id} is not on screen');
    var desc = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value');
    desc.set.call(input, ${JSON.stringify(value)});
    input.dispatchEvent(new Event('input', { bubbles: true }));
  })()`)
  await expect
    .poll(async () => tauriPage.evaluate<string>(`document.querySelector('${SHEET} #${id}')?.value ?? ''`), {
      timeout: waitBudget(2000),
    })
    .toBe(value)
}

/** The sheet's primary button: Connect in add mode. */
async function pressSheetSubmit(tauriPage: PageLike): Promise<void> {
  const outcome = await pointerClick(tauriPage, `document.querySelector('${SHEET} .modal-footer .btn-primary')`)
  expect(outcome, 'the sheet has a live primary button').toBe('clicked')
}

/** What one Connect press came back with, once the sheet stopped working on it. */
export type SheetRound = 'closed' | 'secret_refused' | 'address_refused'

/**
 * Fills the add form for `address` and presses Connect, trusting an SSH host
 * key on first contact the way a person would, until the sheet either closes
 * or puts a refusal on screen.
 *
 * ❗ A first SFTP connect is two rounds (the key, then the sign-in), and the
 * sheet stays open across them. The key step's button is the one trust answer
 * the sheet offers on first contact; a CHANGED key never reaches here.
 */
export async function addServerThroughSheet(
  tauriPage: PageLike,
  fixture: ServerFixture,
  dir: string,
  password: string,
): Promise<SheetRound> {
  await openAddServerSheet(tauriPage)
  await setSheetField(tauriPage, 'server-address', fixture.addressFor(dir))
  // The account fields appear once the address reads as SFTP or WebDAV.
  await expect
    .poll(async () => tauriPage.isVisible(`${SHEET} #server-secret`), { timeout: waitBudget(5000) })
    .toBeTruthy()
  // SFTP's `user@host` fills the account in; a WebDAV URL names none.
  if ((await tauriPage.evaluate<string>(`document.querySelector('${SHEET} #server-username').value`)) === '') {
    await setSheetField(tauriPage, 'server-username', SERVER_USERNAME)
  }
  await setSheetField(tauriPage, 'server-secret', password)
  await pressSheetSubmit(tauriPage)

  let round: SheetRound | 'host_key' | 'busy' = 'busy'
  await expect
    .poll(
      async () => {
        round = await readSheetRound(tauriPage)
        if (round === 'host_key') {
          await pointerClick(tauriPage, `document.querySelector('${SHEET} .host-key .trust-row .btn-primary')`)
          return false
        }
        return round !== 'busy'
      },
      { timeout: waitBudget(45000) },
    )
    .toBeTruthy()
  return round as SheetRound
}

async function readSheetRound(tauriPage: PageLike): Promise<SheetRound | 'host_key' | 'busy'> {
  return tauriPage.evaluate<SheetRound | 'host_key' | 'busy'>(`(function () {
    var sheet = document.querySelector('${SHEET}');
    if (!sheet) return 'closed';
    if (sheet.querySelector('.host-key .trust-row .btn-primary:not([disabled])')) return 'host_key';
    if (sheet.querySelector('#server-secret-refusal')) return 'secret_refused';
    if (sheet.querySelector('#server-address-refusal')) return 'address_refused';
    return 'busy';
  })()`)
}

/** Closes the sheet through its own Cancel button. */
export async function cancelSheet(tauriPage: PageLike): Promise<void> {
  if (!(await tauriPage.isVisible(SHEET))) return
  const outcome = await pointerClick(tauriPage, `document.querySelector('${SHEET} .modal-footer .btn-secondary')`)
  expect(outcome).toBe('clicked')
  await expect.poll(async () => tauriPage.isVisible(SHEET), { timeout: waitBudget(5000) }).toBeFalsy()
}

// ── What the backend holds ───────────────────────────────────────────────────

interface SavedPlaceWire {
  volumeId: string
  appRoot: string
  connected: boolean
}

interface SavedServerWire {
  id: string
  protocol: string
  address: string
  username: string | null
  autoReconnect: boolean
  pinned: boolean
  places: SavedPlaceWire[]
}

/** Every saved server the backend holds, straight from its store. */
async function listSavedServers(tauriPage: PageLike): Promise<SavedServerWire[]> {
  return tauriPage.evaluate<SavedServerWire[]>(`window.__TAURI_INTERNALS__.invoke('list_saved_servers')`)
}

/**
 * Every app path on the fixture's account starts with this: `<scheme>://<user>@<host>:<port>`,
 * the tuple the backend mints the volume id from (`cmdr_fs::volume::ids`).
 *
 * ❗ Matched on this, ❌ never on the saved ADDRESS: a WebDAV address is the URL as
 * the backend keeps it, which drops a default port (`:80`), so a `host:port`
 * search through it finds nothing on the Docker lane.
 */
function appRootPrefix(fixture: ServerFixture): string {
  return `${fixture.protocol}://${SERVER_USERNAME}@${fixture.host.toLowerCase()}:${String(fixture.port)}`
}

/** Whether an app path belongs to the fixture's account, root included. */
function isOnFixture(appPath: string, fixture: ServerFixture): boolean {
  const prefix = appRootPrefix(fixture)
  return appPath === prefix || appPath.startsWith(`${prefix}/`)
}

/** The saved server this fixture names, or `null`. */
export async function savedServerFor(tauriPage: PageLike, fixture: ServerFixture): Promise<SavedServerWire | null> {
  const servers = await listSavedServers(tauriPage)
  return servers.find((s) => s.places.some((place) => isOnFixture(place.appRoot, fixture))) ?? null
}

/** The saved server this fixture names, failing the test when there is none. */
export async function requireSavedServer(tauriPage: PageLike, fixture: ServerFixture): Promise<SavedServerWire> {
  const saved = await savedServerFor(tauriPage, fixture)
  if (!saved)
    throw new Error(`the app has no saved ${fixture.label} server for ${fixture.host}:${String(fixture.port)}`)
  return saved
}

/** The volume the backend has registered for this fixture right now, if any. */
export async function registeredVolumeFor(
  tauriPage: PageLike,
  fixture: ServerFixture,
): Promise<{ id: string; path: string; connectionState: string | null } | null> {
  const volumes = await tauriPage.evaluate<{ id: string; path: string; connectionState: string | null }[]>(
    `window.__TAURI_INTERNALS__.invoke('list_volumes').then(function (r) { return r.data; })`,
  )
  return volumes.find((v) => isOnFixture(v.path, fixture) && v.connectionState !== 'saved') ?? null
}

/**
 * Forgets the fixture's server and its stored password, so the app ends a suite
 * the way it started one. Best-effort: a teardown that throws would bury the
 * test's own verdict.
 */
export async function forgetServer(tauriPage: PageLike, fixture: ServerFixture): Promise<void> {
  const saved = await savedServerFor(tauriPage, fixture).catch(() => null)
  if (!saved) return
  await tauriPage
    .evaluate(`(async function () {
      var invoke = window.__TAURI_INTERNALS__.invoke;
      try { await invoke('forget_server_secret', { id: ${JSON.stringify(saved.id)} }); } catch (e) {}
      try { await invoke('forget_server', { id: ${JSON.stringify(saved.id)} }); } catch (e) {}
    })()`)
    .catch(() => undefined)
  await expect.poll(async () => savedServerFor(tauriPage, fixture), { timeout: waitBudget(5000) }).toBeNull()
}

// ── Panes ────────────────────────────────────────────────────────────────────

export type Side = 'left' | 'right'

/** One pane's volume and path, as `cmdr://state` publishes them. */
export async function paneLocation(side: Side): Promise<{ volumeId: string; path: string }> {
  const state = await mcpReadResource('cmdr://state')
  const start = state.indexOf(`\n${side}:\n`)
  const end = state.indexOf(side === 'left' ? '\nright:\n' : '\nvolumes:', start + 1)
  const section = state.slice(start, end === -1 ? undefined : end)
  return {
    volumeId: /\n {2}volumeId: ([^\n]+)/.exec(section)?.[1] ?? '',
    path: (/\n {2}path: ([^\n]+)/.exec(section)?.[1] ?? '').replace(/^"|"$/g, ''),
  }
}

const PANE_INDEX: Record<Side, 0 | 1> = { left: 0, right: 1 }

/** The expression for one pane's volume switcher trigger. */
function switcherTriggerExpr(side: Side): string {
  return `document.querySelectorAll('.file-pane')[${String(PANE_INDEX[side])}]?.querySelector('.volume-name')`
}

/** Opens one pane's volume switcher (the chip at the pane's top), or leaves it open. */
export async function openSwitcher(tauriPage: PageLike, side: Side): Promise<void> {
  if (await tauriPage.isVisible('[data-menu]')) return
  expect(await pointerClick(tauriPage, switcherTriggerExpr(side))).toBe('clicked')
  await expect.poll(async () => tauriPage.isVisible('[data-menu]'), { timeout: waitBudget(5000) }).toBeTruthy()
}

/** Closes whatever switcher is open. */
export async function closeSwitcher(tauriPage: PageLike): Promise<void> {
  if (!(await tauriPage.isVisible('[data-menu]'))) return
  await escapeOverlayUntilGone(tauriPage, '[data-menu]')
}

/** A switcher row, by the volume id it stands for. */
export function switcherRow(volumeId: string): string {
  return `[data-menu]:not([data-menu-submenu]) [data-menu-row="${volumeId}"]`
}

/**
 * The "Reconnect automatically" checkbox row in a server row's → submenu. The
 * row's value is `row:<volumeId>:<entry>` (`row-menu.ts`), which is what makes it
 * addressable without reading its (translated) label.
 */
export function autoReconnectRow(volumeId: string): string {
  return `[data-menu-submenu] [data-menu-row="row:${volumeId}:toggle:auto-reconnect"]`
}

/** Whether a checkbox menu row is showing its check. */
export async function isChecked(tauriPage: PageLike, row: string): Promise<boolean> {
  return tauriPage.evaluate<boolean>(`document.querySelector('${row}')?.hasAttribute('data-checked') === true`)
}

/**
 * Opens a switcher row's → submenu from the keyboard: arrows down to the row,
 * then → into its actions, the way the submenu is meant to be reached.
 */
export async function openRowSubmenuWithArrowKeys(tauriPage: PageLike, side: Side, volumeId: string): Promise<void> {
  await openSwitcher(tauriPage, side)
  const row = switcherRow(volumeId)
  await expect.poll(async () => tauriPage.isVisible(row), { timeout: waitBudget(5000) }).toBeTruthy()
  // One press per round trip, reading the highlight back before the next, and no
  // more presses than the menu has rows (it wraps, so that's a full lap).
  const highlighted = `!!document.querySelector('${row}[data-highlighted]')`
  const rows = await tauriPage.evaluate<number>(`document.querySelectorAll('[data-menu] [data-menu-row]').length`)
  for (let i = 0; i <= rows && !(await tauriPage.evaluate<boolean>(highlighted)); i++) {
    await pressKey(tauriPage, 'ArrowDown')
  }
  await expect.poll(async () => tauriPage.evaluate<boolean>(highlighted), { timeout: waitBudget(2000) }).toBeTruthy()
  await pressKey(tauriPage, 'ArrowRight')
  await expect
    .poll(async () => tauriPage.isVisible(`[data-menu-submenu] [data-menu-row^="row:${volumeId}:"]`), {
      timeout: waitBudget(5000),
    })
    .toBeTruthy()
}

/**
 * Puts `side` on the server through its switcher, the way a person picks it, and
 * waits for the pane to list `expectEntry` there.
 */
export async function openServerInPane(
  tauriPage: PageLike,
  side: Side,
  volumeId: string,
  expectEntry: string,
): Promise<void> {
  await openSwitcher(tauriPage, side)
  await expect.poll(async () => tauriPage.isVisible(switcherRow(volumeId)), { timeout: waitBudget(5000) }).toBeTruthy()
  expect(await pointerClick(tauriPage, `document.querySelector('${switcherRow(volumeId)}')`)).toBe('clicked')
  await expect.poll(async () => (await paneLocation(side)).volumeId, { timeout: waitBudget(15000) }).toBe(volumeId)
  await expect.poll(async () => paneLists(tauriPage, side, expectEntry), { timeout: waitBudget(15000) }).toBeTruthy()
}

/** Whether `side` lists an entry of that exact name (byte for byte). */
export async function paneLists(tauriPage: PageLike, side: Side, name: string): Promise<boolean> {
  return tauriPage.evaluate<boolean>(`(function () {
    var pane = document.querySelectorAll('.file-pane')[${String(PANE_INDEX[side])}];
    if (!pane) return false;
    var rows = pane.querySelectorAll('.file-entry');
    for (var i = 0; i < rows.length; i++) {
      if (rows[i].getAttribute('data-filename') === ${JSON.stringify(name)}) return true;
    }
    return false;
  })()`)
}

/** Focuses a pane by clicking it, as a person would. */
export async function focusSide(tauriPage: PageLike, side: Side): Promise<void> {
  await focusPane(tauriPage, PANE_INDEX[side])
}

/** Presses a key at whatever holds focus, the suite's one keyboard path. */
export async function press(tauriPage: PageLike, key: string): Promise<void> {
  await pressKey(tauriPage, key)
}

// ── Local side ───────────────────────────────────────────────────────────────

/** The bytes of a local file, or `null` when there's none. */
export function readLocal(filePath: string): Buffer | null {
  return fs.existsSync(filePath) ? fs.readFileSync(filePath) : null
}

/** The names a local directory holds, as the filesystem stores them. */
export function listLocal(dir: string): string[] {
  return fs.readdirSync(dir).sort()
}

/** A payload no other test writes, with a byte past ASCII so a text-mode slip shows. */
export function payload(tag: string): Buffer {
  return Buffer.from(`${tag}: ${'ő'.repeat(3)} ${'x'.repeat(2048)}\n`, 'utf-8')
}

export { path, SERVER_PASSWORD }
