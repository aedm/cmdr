/**
 * The hub's list: every server the user saved, plus every host mDNS found, as
 * one row each.
 *
 * Pure, so the merge and the ordering are testable without a component. The hub
 * component reads the three inputs (the saved list, the discovery store, the
 * volume list) and renders what comes back.
 *
 * ❗ **The merge is the part that goes quietly wrong.** A manually-typed SMB host
 * is BOTH a saved server and a discovered host (adding one injects it into the
 * discovery state), so concatenating the two sources shows a person's NAS twice.
 * The dedup matches on the id first (a manual host keeps its store id in the
 * discovery list) and then on the name or resolved hostname, because
 * `known_shares` files a host under the server name statfs reported while mDNS
 * files the same machine under its Bonjour name. It claims EVERY host that
 * matches, since one machine can be in the discovery list under both spellings.
 *
 * An SMB server's saved SHARES (`SavedServer.places`, `docs/specs/saved-smb-shares.md`)
 * are rows of their own, right under their server: the list says "user + server +
 * share", which is what a person means to save (cmdr-reports#7).
 */

import type { SavedPlace, SavedServer } from '$lib/tauri-commands'
import type { ConnectionState, NetworkHost, VolumeInfo } from '../types'
import { signedInAsOfMount, signedInAsUser, type SignedInAs } from './signed-in-as'

/** SMB's own port, which an address leaves unsaid. */
const SMB_PORT = 445

/** What the Status column says about a row. */
export type HubRowStatus =
  /** A live session Cmdr owns, or an SMB share mounted through the OS. */
  | 'connected'
  /** Saved and idle: nothing in flight, and nothing is wrong. */
  | 'saved'
  /** mDNS is seeing it right now. */
  | 'found_nearby'
  /** The session dropped because a credential is what's missing. */
  | 'signed_out'
  /** An SFTP host key is waiting for the user to look at it. */
  | 'waiting_for_key'

/** One line in the hub's table. */
export interface HubRow {
  /** Stable across rebuilds: the saved server's id, else the host's; `share:<volume id>` for a share. */
  id: string
  /**
   * A SERVER (an account, or a host mDNS sees) or one of an SMB server's saved
   * SHARES, which sits right under it.
   */
  kind: 'server' | 'share'
  /** A share's server row, `null` for a server. */
  parentId: string | null
  /**
   * The account the row is signed in as, `null` when nothing known says. A share: the live mount's while it's
   * connected, else the saved one the next connect uses. An SMB server: the SERVER-level account, the same one its
   * share list's header names (the account its listing signed in as, else the one it's set to be used with), ❌ never
   * a share's mount, which disagreed with the header. A one-place server: `null` (its name is `user@host` already).
   */
  account: SignedInAs | null
  /** A share's place, `null` for a server row (a one-place server's is `saved.places[0]`). */
  place: SavedPlace | null
  /** What the Name column shows. */
  name: string
  /** Which protocol the row speaks, for the Type column. */
  protocol: 'smb' | 'sftp' | 'webdav'
  /** What the Address column shows: resolved where mDNS resolved it. */
  address: string
  status: HubRowStatus
  /** ISO 8601, or `null` when nothing ever recorded one. */
  lastConnectedAt: string | null
  /**
   * The place's volume id: a one-place server's, or a saved share's.
   *
   * ❗ `null` for an SMB HOST row: its places are its shares, each a row of its
   * own. Enter on a host opens its places list instead.
   */
  volumeId: string | null
  /** Whether the place is pinned to the switcher. Always `false` for an SMB host. */
  pinned: boolean
  /** The saved entry behind the row, when the user saved one. */
  saved: SavedServer | null
  /** The discovered host behind the row, when mDNS is seeing one. */
  host: NetworkHost | null
}

/** What the hub reads to build its list. */
export interface HubRowSources {
  /** `listSavedServers()`, the union of the three stores. */
  saved: SavedServer[]
  /** The discovery store's hosts, manual entries included. */
  hosts: NetworkHost[]
  /** The current volume list, which is where a place's standing lives. */
  volumes: VolumeInfo[]
  /**
   * The account a host's share list last signed in as, by the host's id (`network-store.svelte.ts`'s
   * `getListedAccount`), or `undefined` when no listing said.
   */
  listedAs?: (hostId: string) => SignedInAs | undefined
}

/**
 * Rank groups, most urgent first: a live session, then one asking something of
 * the user, then the rest of what they saved, then what mDNS is seeing.
 */
const STATUS_RANK: Record<HubRowStatus, number> = {
  connected: 0,
  signed_out: 1,
  waiting_for_key: 1,
  saved: 2,
  found_nearby: 3,
}

/**
 * The hub's rows, merged and ordered.
 *
 * ❗ **Every row's `id` is unique, and that is this function's job, ❌ not its
 * caller's.** The hub keys its `{#each}` on it, and Svelte THROWS
 * (`each_key_duplicate`) on a repeat, so a duplicate is a CRASHED pane rather
 * than a row shown twice — and it takes the whole servers hub down with it. The
 * sources can genuinely repeat one: `listSavedServers()` unions three stores, and
 * a server recorded in two of them arrives twice. First writer wins, so the
 * order below still decides which row a person sees.
 */
export function buildHubRows(sources: HubRowSources): HubRow[] {
  const states = new Map(sources.volumes.map((volume) => [volume.id, volume.connectionState ?? null]))
  const mountAccounts = new Map(sources.volumes.map((volume) => [volume.id, volume.mountAccount ?? null]))
  const listed = (ids: (string | undefined)[]): SignedInAs | null => {
    for (const id of ids) {
      const answer = id === undefined ? undefined : sources.listedAs?.(id)
      if (answer) return answer
    }
    return null
  }
  const claimed = new Set<string>()
  const taken = new Set<string>()
  const rows: HubRow[] = []

  const add = (row: HubRow): void => {
    if (taken.has(row.id)) return
    taken.add(row.id)
    rows.push(row)
  }

  for (const server of sources.saved) {
    const hosts = matchingHosts(server, sources.hosts)
    for (const host of hosts) claimed.add(host.id)
    add(savedRow(server, primaryHost(hosts), states))
  }

  for (const host of sources.hosts) {
    if (claimed.has(host.id)) continue
    add({ ...nearbyRow(host), account: listed([host.id]) })
  }

  // Servers in rank order, each followed by its shares in name order. ❗ Shares
  // are placed AFTER the sort, so a share never drifts away from its server.
  const ordered: HubRow[] = []
  for (const row of rows.sort(compareRows)) {
    ordered.push(row)
    if (!row.saved || row.protocol !== 'smb') continue
    const shares = [...row.saved.places].sort((a, b) =>
      a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }),
    )
    const shareRows = shares.map((place) => shareRow(row, place, states, mountAccounts.get(place.volumeId) ?? null))
    row.account = listed([row.host?.id, row.id]) ?? signedInAsUser(row.saved.username)
    for (const share of shareRows) {
      if (taken.has(share.id)) continue
      taken.add(share.id)
      ordered.push(share)
    }
  }
  return ordered
}

/**
 * A saved SMB share, as the row under its server.
 *
 * Its status is off the VOLUME LIST like every other place's: connected while
 * its volume is mounted (through the kernel or directly), saved otherwise.
 *
 * ❗ While connected it names the account the LIVE mount signed in as (`mountAccount`,
 * off the mount table; `GUEST` is nobody). The saved account is for the next connect:
 * an Add as otheruser over a mount signed in as testuser read "Connected … as otheruser".
 */
function shareRow(
  server: HubRow,
  place: SavedPlace,
  states: Map<string, ConnectionState | null>,
  mountAccount: string | null,
): HubRow {
  const state = states.get(place.volumeId) ?? null
  const live = state === 'direct' || state === 'os_mount'
  const liveAccount = live ? signedInAsOfMount(mountAccount) : null
  return {
    id: `share:${place.volumeId}`,
    kind: 'share',
    parentId: server.id,
    account: liveAccount ?? signedInAsUser(place.username),
    place,
    name: place.name,
    protocol: 'smb',
    address: server.address,
    status: live ? 'connected' : 'saved',
    lastConnectedAt: null,
    volumeId: place.volumeId,
    pinned: place.pinned,
    saved: server.saved,
    host: server.host,
  }
}

/**
 * Every host in the discovery list that IS this saved SMB server.
 *
 * ❗ Plural on purpose: ONE machine can be in that list twice, because adding a
 * host by hand injects a `manual` host beside the `discovered` one mDNS already
 * found. Claiming only the first would leave the other as a second row for the
 * same NAS.
 */
function matchingHosts(server: SavedServer, hosts: NetworkHost[]): NetworkHost[] {
  if (server.protocol !== 'smb') return []
  const address = server.address.toLowerCase()
  const name = server.displayName.toLowerCase()
  return hosts.filter(
    (host) =>
      host.id === server.id ||
      host.name.toLowerCase() === address ||
      host.name.toLowerCase() === name ||
      host.hostname?.toLowerCase() === address,
  )
}

/**
 * The ids of the discovery list's hosts that ARE one of the saved servers, by the
 * same match the hub's merge makes. Every other host is one Cmdr merely found.
 */
export function savedSmbHostIds(saved: SavedServer[], hosts: NetworkHost[]): Set<string> {
  return new Set(saved.flatMap((server) => matchingHosts(server, hosts)).map((host) => host.id))
}

/**
 * Whether the row is a host Cmdr only FOUND: no saved server, no saved share,
 * nothing the person added. The hub folds these into one group under the saved
 * servers (`servers-hub-items.ts`), and nothing lists their shares until the
 * person opens one (`network-store.svelte.ts`).
 */
export function isNearbyOnly(row: HubRow): boolean {
  return row.saved === null
}

/**
 * Which of them the row speaks for.
 *
 * A DISCOVERED host wins: its name is the Bonjour name a person recognizes,
 * where a manual host is named after the address they typed.
 */
function primaryHost(hosts: NetworkHost[]): NetworkHost | null {
  const discovered = hosts.find((host) => host.source === 'discovered')
  if (discovered) return discovered
  // ❗ Length-checked, ❌ not `[0] ?? null`: the index signature types the gap
  // away, so an empty list would hand back `undefined` wearing `NetworkHost`.
  return hosts.length > 0 ? hosts[0] : null
}

function savedRow(server: SavedServer, host: NetworkHost | null, states: Map<string, ConnectionState | null>): HubRow {
  // ❗ Length-checked, not `[0] ?? null`: an SMB server may carry no places,
  // and the index signature would otherwise type the gap away.
  const place: SavedPlace | null = server.places.length > 0 ? server.places[0] : null
  return {
    id: server.id,
    kind: 'server',
    parentId: null,
    account: null,
    place: null,
    name: displayName(server, host),
    protocol: server.protocol,
    address: hostAddress(host) ?? server.address,
    status: savedStatus(server, host, place ? (states.get(place.volumeId) ?? null) : null),
    lastConnectedAt: server.lastConnectedAt,
    // An SMB host's places are its shares, each a row of its own.
    volumeId: server.protocol === 'smb' ? null : (place?.volumeId ?? null),
    pinned: server.protocol === 'smb' ? false : (place?.pinned ?? false),
    saved: server,
    host,
  }
}

/**
 * Which of the three names the Name column shows.
 *
 * ❗ A name a PERSON chose wins, then the Bonjour name mDNS found, then the
 * stand-in nobody chose. The top rank is a FACT the backend publishes
 * (`SavedServer.nameSource`), ❌ never a guess at the string's shape: an SMB
 * host's label is either the way `statfs` spells the server
 * (`smb-consumer-guest`, written to `known_shares` the first time the host is
 * opened) or the address typed into "Add server". Without the rank, the friendly
 * name a person recognizes (`SMB Test (Guest)`, `Naspolya`) would vanish from
 * the column the moment they used the host.
 */
function displayName(server: SavedServer, host: NetworkHost | null): string {
  if (server.nameSource === 'user') return server.displayName
  return host?.name ?? server.displayName
}

function nearbyRow(host: NetworkHost): HubRow {
  return {
    id: host.id,
    kind: 'server',
    parentId: null,
    account: null,
    place: null,
    name: host.name,
    protocol: 'smb',
    address: hostAddress(host) ?? host.name,
    status: 'found_nearby',
    lastConnectedAt: null,
    volumeId: null,
    pinned: false,
    saved: null,
    host,
  }
}

/**
 * How live a saved row is.
 *
 * ❗ Off the VOLUME LIST, ❌ never off `SavedPlace.connected`: that flag is a
 * snapshot from the moment the listing was built, while the volume list is what
 * the switcher's dot and the pane both read. Two surfaces disagreeing about
 * whether a server is up is worse than either being briefly stale.
 *
 * An SMB host has no place to ask about, so mDNS seeing it is the whole answer:
 * a mounted share is its own volume row, not this host. ❗ A DISCOVERED host,
 * ❌ never the host's own manual entry: Cmdr injects every typed-in host into the
 * discovery list at startup, reachable or not, so that entry says nothing about
 * the network. `primaryHost` puts a discovered one first when there is one.
 */
function savedStatus(server: SavedServer, host: NetworkHost | null, state: ConnectionState | null): HubRowStatus {
  if (server.protocol === 'smb') return host?.source === 'discovered' ? 'found_nearby' : 'saved'
  switch (state) {
    case 'direct':
    case 'os_mount':
      return 'connected'
    case 'needs_sign_in':
      return 'signed_out'
    case 'needs_host_key_approval':
      return 'waiting_for_key'
    // `disconnected` says a backoff loop is running, which is a detail of HOW the
    // row gets back: to a person it is still one of their saved servers, and the
    // switcher's dot is where liveness is spelled out.
    case 'disconnected':
    case 'saved':
    case null:
      return 'saved'
  }
}

/** Where Enter on a row leads. */
export type HubOpenMove =
  /** A host's share list. `label` is what the row calls it, for the list's words. */
  | { kind: 'host'; host: NetworkHost; label: string }
  /** The pane lands on the row's place: a one-place server's, or a saved share's. */
  | { kind: 'place'; row: HubRow }
  /** A saved share no mount went through yet: its host's share list, mounting that share. */
  | { kind: 'share_via_host'; host: NetworkHost; share: string; label: string }

/**
 * What Enter does to `row`: the hub's ONE decision about where a row leads.
 *
 * ❗ A host opens its share list and ❌ never mounts a share on its own (the
 * person picked a HOST, cmdr-reports#7). A saved share with a place in the volume
 * list lands the pane on it the way an SFTP place does, and a place that isn't
 * mounted is mounted right there, in the pane. One nothing mounted yet (a share
 * Add named) goes through its host's share list: its first mount is what gives
 * it a place. `null` when a share has no host to go through, which shouldn't
 * happen and is logged by the caller.
 */
export function openMoveFor(row: HubRow, rows: HubRow[], volumes: VolumeInfo[]): HubOpenMove | null {
  if (row.kind === 'share') {
    if (row.volumeId && volumes.some((volume) => volume.id === row.volumeId)) return { kind: 'place', row }
    const server = rows.find((candidate) => candidate.id === row.parentId)
    const host = row.host ?? (server ? savedHostFor(server) : null)
    return host && row.place
      ? { kind: 'share_via_host', host, share: row.place.name, label: server?.name ?? host.name }
      : null
  }
  if (row.protocol === 'smb') return { kind: 'host', host: row.host ?? savedHostFor(row), label: row.name }
  return { kind: 'place', row }
}

/**
 * A saved SMB host mDNS isn't seeing right now, as a host the places list can
 * take. Its address is the only spelling anything has for it: `host`, or
 * `host:port` off 445 (the listing's `SavedServer.address`).
 */
export function savedHostFor(row: HubRow): NetworkHost {
  const withPort = /^(.+):(\d+)$/.exec(row.address)
  const [hostname, port] = withPort ? [withPort[1], Number(withPort[2])] : [row.address, SMB_PORT]
  return { id: row.id, name: row.name, hostname, port, source: 'manual' }
}

/** A share is a folder under its server; a server is a machine, or a service on one. */
export function hubRowIcon(row: HubRow): 'folder' | 'monitor' | 'server' {
  if (row.kind === 'share') return 'folder'
  return row.protocol === 'smb' ? 'monitor' : 'server'
}

/** `Last used`, as the seconds-based `DateLabel` takes it, or `null` for never. */
export function lastUsedSeconds(row: HubRow): number | null {
  if (!row.lastConnectedAt) return null
  const parsed = Date.parse(row.lastConnectedAt)
  return Number.isNaN(parsed) ? null : Math.floor(parsed / 1000)
}

/**
 * The most useful spelling of where a discovered host lives, with its port when
 * it isn't 445: `localhost` alone would name another server on the same machine.
 */
function hostAddress(host: NetworkHost | null): string | null {
  const address = host?.ipAddress ?? host?.hostname ?? null
  if (!host || address === null || host.port === SMB_PORT) return address
  return address.includes(':') ? `[${address}]:${String(host.port)}` : `${address}:${String(host.port)}`
}

/**
 * What the person saved first (live, then what's asking for them, then idle, then
 * the ones mDNS also sees), then the hosts Cmdr only found.
 *
 * ❗ The found-only hosts stay CONTIGUOUS at the end, whatever their recency or
 * name: the hub's group header sits in front of the first one.
 */
function compareRows(a: HubRow, b: HubRow): number {
  const byOwnership = Number(isNearbyOnly(a)) - Number(isNearbyOnly(b))
  if (byOwnership !== 0) return byOwnership
  const byStatus = STATUS_RANK[a.status] - STATUS_RANK[b.status]
  if (byStatus !== 0) return byStatus
  const byRecency = recency(b) - recency(a)
  if (byRecency !== 0) return byRecency
  return a.name.localeCompare(b.name, undefined, { sensitivity: 'base' })
}

/** When the row was last used, as a sortable number. Never used sorts last. */
function recency(row: HubRow): number {
  if (!row.lastConnectedAt) return 0
  const parsed = Date.parse(row.lastConnectedAt)
  return Number.isNaN(parsed) ? 0 : parsed
}
