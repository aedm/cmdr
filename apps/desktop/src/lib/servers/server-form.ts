/**
 * The add form's model, and the two translations around it: a pasted address in,
 * a `ServerTarget` out.
 *
 * Pure, so the sheet stays a renderer and the rules that decide what a typed
 * address MEANS are testable without mounting anything.
 */

import type { SavedServer, ServerProtocol, ServerTarget } from '$lib/ipc/bindings'
import type { SavedSftpServer, SavedWebdavServer } from '$lib/tauri-commands'
import { parseServerAddress, uncAsSmbUrl, type ParsedAddress } from './address-parser'

/** Every field the add and edit forms hold, across all three protocols. */
export interface ServerForm {
  protocol: ServerProtocol
  /** What the user typed, kept verbatim so their own spelling survives an edit. */
  address: string
  username: string
  /**
   * Whether `username` is the one the ADDRESS carried (`smb://x@nas/…`) rather than one
   * the person typed: an address-filled one follows the address, and goes when the
   * address stops naming it. Typing into the field makes it the person's.
   */
  usernameFromAddress: boolean
  /** ❗ Lives here only while the sheet is open. Nothing persists it; the backend's store does. */
  secret: string
  remember: boolean
  displayName: string
  remoteRoot: string
  /** Where the place lands when opened, at or under `remoteRoot`. Empty is the root. */
  startFolder: string
  keyFile: string
  useAgent: boolean
  autoReconnect: boolean
}

/** A blank form, on add mode's defaults. */
export function emptyServerForm(): ServerForm {
  return {
    // Until the person picks another. SMB is what a NAS on the home network
    // speaks, and it asks for nothing up front, so a wrong default costs nothing.
    protocol: 'smb',
    address: '',
    username: '',
    usernameFromAddress: false,
    secret: '',
    // ❗ Add mode ONLY. A person typing a password into a new server means to
    // come back to it; sign-in mode seeds this from what is already stored, so a
    // user who declined to remember stays declined.
    remember: true,
    displayName: '',
    remoteRoot: '',
    startFolder: '',
    keyFile: '',
    useAgent: true,
    autoReconnect: true,
  }
}

/**
 * Folds what an address turned out to say into the form: the account, and the
 * SFTP root folder.
 *
 * ❗ **It never touches the protocol.** The toggle is the person's; what the
 * address names only feeds the warning under the field (`addressLooksLike`).
 * The sheet re-runs this when the toggle moves too, so a path typed before
 * picking SFTP still lands in the root folder.
 *
 * ❗ An `unparsed` address changes nothing, because a half-typed address is the
 * normal state of a field someone is typing into.
 */
export function applyParsedAddress(form: ServerForm, parsed: ParsedAddress): ServerForm {
  if (parsed.kind === 'unparsed') return form
  // A path is a folder only on SFTP (an SMB path is a share, and a WebDAV one
  // stays in the base URL), and only when the address doesn't name another
  // protocol, whose path means something else.
  const pathIsRoot = form.protocol === 'sftp' && speaksOrNamesNone(parsed, 'sftp')
  return {
    ...form,
    // ❗ The address's account when it carries one. When it doesn't, a username
    // the address had filled goes with it, and one the person typed stays.
    ...(parsed.username !== undefined
      ? { username: parsed.username, usernameFromAddress: true }
      : form.usernameFromAddress
        ? { username: '', usernameFromAddress: false }
        : {}),
    remoteRoot: pathIsRoot ? (parsed.path ?? form.remoteRoot) : form.remoteRoot,
  }
}

/**
 * The form a prefilled add sheet opens on: `address`, with the toggle on the
 * protocol it SPELLS OUT, if it spells one.
 *
 * ❗ Not the typing rule, on purpose. A prefill arrives from Go to path or ⌘K
 * as a whole URL the person asked to open (`sftp://…`, `https://…`), so its
 * scheme is their choice already, and the sheet opens with the toggle in view
 * before anything is dialed. An address with no scheme leaves the default.
 */
export function formFromPrefill(address: string): ServerForm {
  const parsed = parseServerAddress(address)
  const protocol = parsed.kind === 'parsed' && parsed.protocol !== undefined ? parsed.protocol : 'smb'
  return applyParsedAddress({ ...emptyServerForm(), protocol, address }, parsed)
}

/**
 * The dial target this form names, or `null` when it names none.
 *
 * `null` covers both an address that doesn't parse and SMB, whose connect is a
 * share mount rather than a session and goes through `connectToServer` with
 * `smbAddressFrom` instead.
 */
export function serverTargetFrom(form: ServerForm): ServerTarget | null {
  const parsed = parseServerAddress(form.address)
  if (parsed.kind === 'unparsed') return null

  const username = form.username.trim()
  // ❗ Empty stays empty. The backend calls an unnamed server by its account and
  // host, and a name that repeated the typed address left the edit sheet with a
  // name that looked like the address, which sent a person to edit the wrong field.
  const displayName = form.displayName.trim()

  // ❗ The TOGGLE decides which target this is, ❌ never the address: this is
  // the one place a protocol is chosen for a dial, and it is the person's pick
  // (cmdr-reports#8). The address only supplies the endpoint, and a port its
  // scheme named for a different protocol is not this protocol's port.
  if (form.protocol === 'sftp') {
    return {
      protocol: 'sftp',
      displayName,
      host: parsed.host,
      port: speaksOrNamesNone(parsed, 'sftp') ? (parsed.port ?? 22) : 22,
      username,
      remoteRoot: normalizeRoot(form.remoteRoot),
      startFolder: startFolderOf(form),
      keyFile: form.keyFile.trim() === '' ? null : form.keyFile.trim(),
      useAgent: form.useAgent,
      autoReconnect: form.autoReconnect,
    }
  }

  if (form.protocol === 'webdav') {
    return {
      protocol: 'webdav',
      displayName,
      url: webdavBaseUrl(parsed),
      username,
      remoteRoot: normalizeRoot(form.remoteRoot),
      startFolder: startFolderOf(form),
      autoReconnect: form.autoReconnect,
    }
  }

  // SMB: its connect is a share mount rather than a session, so it goes through
  // `connectToServer` and has no target here.
  return null
}

/**
 * The address SMB's add hands `connect_to_server`, which reads a bare host,
 * `host:port`, or an `smb://` URL and refuses anything else.
 *
 * ❗ An address with no scheme travels as the SMB URL it means, because the
 * backend's bare-host reader refuses an `@` or a `/`: `sven@192.168.0.153` is
 * SMB's natural spelling for a NAS share that needs a user. One that names
 * ANOTHER protocol keeps only its host, since SMB is what's selected and that
 * scheme's port and path mean nothing here. One this side can't read goes as
 * typed, so the backend says what is wrong with it.
 */
export function smbAddressFrom(address: string): string {
  const trimmed = address.trim()
  // The backend reads `smb://`, not Windows' backslashes.
  const unc = uncAsSmbUrl(trimmed)
  if (unc) return unc
  const parsed = parseServerAddress(trimmed)
  if (parsed.kind === 'unparsed' || parsed.protocol === 'smb') return trimmed
  if (parsed.protocol === undefined) return `smb://${trimmed}`
  return `smb://${parsed.host}`
}

/**
 * What the server is called when the Name field stays empty, for its
 * placeholder, or `null` while the address doesn't parse yet.
 *
 * ❗ The MIRROR of the backend's stand-in labels, so the placeholder promises
 * what the hub will show: an SMB host goes by its address (with the port when it
 * isn't 445, `manual_servers::display_name`), an account by `username@host`
 * (`saved_server_fields::server_label`). The backend stays the one that decides.
 */
export function nameFallbackOf(form: ServerForm): string | null {
  const parsed = parseServerAddress(form.address)
  if (parsed.kind === 'unparsed') return null
  if (form.protocol === 'smb') {
    const port = speaksOrNamesNone(parsed, 'smb') ? parsed.port : undefined
    return port !== undefined && port !== 445 ? `${parsed.host}:${String(port)}` : parsed.host
  }
  const username = form.username.trim()
  return username === '' ? parsed.host : `${username}@${parsed.host}`
}

/**
 * A saved SMB host, as the edit form holds it.
 *
 * ❗ The name field opens on what a person TYPED: the listing's label is that
 * name only when `nameSource` says so, and otherwise a stand-in nobody chose.
 */
export function formFromSmbHost(server: SavedServer): ServerForm {
  return {
    ...emptyServerForm(),
    protocol: 'smb',
    address: server.address,
    username: server.username ?? '',
    displayName: server.nameSource === 'user' ? server.displayName : '',
    remember: false,
  }
}

/** Whether the address means `protocol` or names no protocol at all, so its port and path are this protocol's. */
function speaksOrNamesNone(parsed: Extract<ParsedAddress, { kind: 'parsed' }>, protocol: ServerProtocol): boolean {
  return parsed.protocol === undefined || parsed.protocol === protocol
}

/** The saved SFTP server, as the edit form holds it. */
export function formFromSftpServer(server: SavedSftpServer): ServerForm {
  return {
    ...emptyServerForm(),
    protocol: 'sftp',
    address: `${server.username}@${server.host}:${String(server.port)}`,
    username: server.username,
    displayName: server.displayName,
    remoteRoot: server.remoteRoot,
    startFolder: server.startFolder ?? '',
    keyFile: server.keyFile ?? '',
    useAgent: server.useAgent,
    autoReconnect: server.autoReconnect,
    // Seeded by the sheet from `hasServerSecret`, ❌ never defaulted on here: a
    // default-on box would offer to seed a secret the user already declined.
    remember: false,
  }
}

/** The saved WebDAV server, as the edit form holds it. */
export function formFromWebdavServer(server: SavedWebdavServer): ServerForm {
  return {
    ...emptyServerForm(),
    protocol: 'webdav',
    address: server.url,
    username: server.username,
    displayName: server.displayName,
    remoteRoot: server.remoteRoot,
    startFolder: server.startFolder ?? '',
    autoReconnect: server.autoReconnect,
    remember: false,
  }
}

/**
 * The Nextcloud collection path, appended to a bare origin.
 *
 * ❗ The one remedy worth a button: a person who pastes their Nextcloud's home
 * page gets "nothing here answers WebDAV", and the fix is a path nobody knows.
 * ownCloud shares it.
 */
export function nextcloudAddress(address: string, username: string): string {
  const account = username.trim()
  if (account === '') return address
  return `${address.replace(/\/+$/, '')}/remote.php/dav/files/${encodeURIComponent(account)}/`
}

/**
 * The base URL a WebDAV form dials, port included only when it isn't the
 * scheme's own. An address with no scheme reads as TLS, because defaulting the
 * other way would send a password in the clear.
 *
 * ❗ A port and a path the address's scheme named for ANOTHER protocol are
 * dropped: `sftp://nas:2222/srv` names an SSH port and a server folder, and
 * carrying either over would dial something nothing answers HTTP on.
 */
function webdavBaseUrl(parsed: Extract<ParsedAddress, { kind: 'parsed' }>): string {
  if (!speaksOrNamesNone(parsed, 'webdav')) return `https://${parsed.host}`
  const scheme = parsed.secure === false ? 'http' : 'https'
  const port = parsed.port ?? (scheme === 'https' ? 443 : 80)
  const isDefaultPort = (scheme === 'https' && port === 443) || (scheme === 'http' && port === 80)
  const authority = isDefaultPort ? parsed.host : `${parsed.host}:${String(port)}`
  return `${scheme}://${authority}${parsed.path ?? ''}`
}

/**
 * The start folder a target carries: `null` for the root. The backend refuses one
 * outside the root and stores it normalized, so this only trims.
 */
function startFolderOf(form: ServerForm): string | null {
  const trimmed = form.startFolder.trim()
  return trimmed === '' ? null : trimmed
}

/**
 * Whether `startFolder` is the root or sits under it, compared by whole path
 * components. An empty start folder means the root, so it always is.
 *
 * ❗ The sheet's inline MIRROR of `saved_server_fields::start_folder_under_root`,
 * for feedback before a round-trip. The backend stays authoritative and answers
 * `start_folder_outside_root` on its own. Both sides normalize first, the way
 * `cmdr_fs::volume::remote_paths::normalize_remote_path` does: `.` and `..`
 * resolved, and a relative path read from `/`. That's why `/srv/data-1` and
 * `/srv/data/../etc` are both outside `/srv/data`.
 */
export function isStartFolderUnderRoot(remoteRoot: string, startFolder: string): boolean {
  if (startFolder.trim() === '') return true
  const root = normalizeServerPath(remoteRoot)
  const folder = normalizeServerPath(startFolder)
  return root === '/' || folder === root || folder.startsWith(`${root}/`)
}

/** A server-side path made absolute, with `.` and `..` resolved lexically. */
function normalizeServerPath(path: string): string {
  const parts: string[] = []
  for (const part of path.trim().split('/')) {
    if (part === '' || part === '.') continue
    if (part === '..') parts.pop()
    else parts.push(part)
  }
  return `/${parts.join('/')}`
}

/** The three root spellings (`''`, `'.'`, `'/'`) all mean the volume root. */
function normalizeRoot(root: string): string {
  const trimmed = root.trim()
  return trimmed === '' || trimmed === '.' ? '/' : trimmed
}
