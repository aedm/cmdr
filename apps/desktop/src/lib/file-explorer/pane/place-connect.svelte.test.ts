/**
 * The pane's half of opening a saved place: dial on landing, cancel while it
 * runs, reload once it is live, and say why when it isn't.
 *
 * ❗ The one-dial-per-landing rule is what these cells guard. The `$effect`
 * re-runs on every volume-list refresh, and a dial per refresh would be a dial
 * per second against a server the user opened once.
 *
 * Runes (`$effect.root` + `$state`), so the filename carries the `.svelte.`
 * infix.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { flushSync } from 'svelte'
import type { VolumeInfo } from '../types'

const { connectPlace, cancelPlaceConnect } = vi.hoisted(() => ({
  connectPlace: vi.fn(),
  cancelPlaceConnect: vi.fn().mockResolvedValue(undefined),
}))

vi.mock('$lib/servers/connect-flow', () => ({ connectPlace, cancelPlaceConnect }))
vi.mock('$lib/servers/connect-refusals', () => ({
  wordConnectRefusal: (kind: string, subject: { host: string; username: string }) =>
    `${kind} for ${subject.username} at ${subject.host}`,
}))
vi.mock('$lib/logging/logger', () => ({
  getAppLogger: () => ({ warn: vi.fn(), info: vi.fn(), error: vi.fn(), debug: vi.fn() }),
}))

import { createPlaceConnect, type PlaceConnect } from './place-connect.svelte'

const savedPlace: VolumeInfo = {
  id: 'sftp-nas-local-22-ada',
  name: 'Naspolya',
  path: 'sftp://ada@nas.local:22/srv/data',
  category: 'network',
  fsType: 'sftp',
  isEjectable: false,
  connectionState: 'saved',
}

describe('createPlaceConnect', () => {
  let dispose: (() => void) | undefined
  /** The pane's live `VolumeInfo`, reactive so a reassignment re-runs the factory's effect. */
  let info = $state<VolumeInfo | null>(null)

  function create(): { sub: PlaceConnect; enter: ReturnType<typeof vi.fn> } {
    const enter = vi.fn()
    let sub!: PlaceConnect
    dispose = $effect.root(() => {
      sub = createPlaceConnect({
        getVolumeId: () => info?.id ?? 'root',
        getCurrentVolumeInfo: () => info,
        getVolumePath: () => savedPlace.path,
        getCurrentPath: () => savedPlace.path,
        enter,
      })
    })
    flushSync()
    return { sub, enter }
  }

  beforeEach(() => {
    vi.clearAllMocks()
    info = { ...savedPlace }
    connectPlace.mockResolvedValue({ kind: 'connected', volumeId: savedPlace.id })
  })

  afterEach(() => {
    dispose?.()
    dispose = undefined
  })

  it('shows the connecting view the moment the pane lands on a saved place', () => {
    const { sub } = create()
    expect(sub.state?.kind).toBe('connecting')
    expect(connectPlace).toHaveBeenCalledWith(expect.objectContaining({ volumeId: savedPlace.id }))
  })

  it('enters the place again once it is live, and drops the view', async () => {
    const { sub, enter } = create()
    await vi.waitFor(() => {
      // A server place's root never moves on a connect, so it re-enters where it stands.
      expect(enter).toHaveBeenCalledWith({
        volumeId: savedPlace.id,
        volumePath: savedPlace.path,
        targetPath: savedPlace.path,
      })
    })
    expect(sub.state).toBeNull()
  })

  it('leaves a live or local volume alone', () => {
    info = { ...savedPlace, connectionState: 'direct' }
    const { sub } = create()
    expect(sub.state).toBeNull()
    expect(connectPlace).not.toHaveBeenCalled()

    info = { ...savedPlace, id: 'root', path: '/', category: 'main_volume', connectionState: undefined }
    dispose?.()
    const second = create()
    expect(second.sub.state).toBeNull()
    expect(connectPlace).not.toHaveBeenCalled()
  })

  it('dials ONCE per landing, however often the volume list refreshes', () => {
    const { sub } = create()
    // A refresh that changes nothing: a new object, same place, same state.
    info = { ...savedPlace }
    flushSync()
    info = { ...savedPlace }
    flushSync()
    expect(connectPlace).toHaveBeenCalledTimes(1)
    expect(sub.state?.kind).toBe('connecting')
  })

  it('arms Cancel with the attempt id the flow hands out', () => {
    connectPlace.mockImplementation((request: { onAttemptStarted?: (id: string) => void }) => {
      request.onAttemptStarted?.('server-connect-7')
      return new Promise(() => {
        // Never settles: the dial is still running while the user cancels.
      })
    })
    const { sub } = create()
    expect(sub.state?.kind).toBe('connecting')
    if (sub.state?.kind !== 'connecting') throw new Error('not connecting')
    sub.state.cancel()
    expect(cancelPlaceConnect).toHaveBeenCalledWith('server-connect-7')
  })

  it('words a refusal from the place’s own host and account, and offers Try again', async () => {
    connectPlace.mockResolvedValue({ kind: 'refused', refusal: 'unreachable' })
    const { sub } = create()
    await vi.waitFor(() => {
      expect(sub.state?.kind).toBe('refused')
    })
    if (sub.state?.kind !== 'refused') throw new Error('not refused')
    expect(sub.state.refusal).toBe('unreachable for ada at nas.local')
    // ❌ No Disconnect on a place with no session to drop.
    expect(sub.state.disconnect).toBeUndefined()

    connectPlace.mockResolvedValue({ kind: 'connected', volumeId: savedPlace.id })
    // ❗ A server refusal always carries one: the state's `retry` is optional
    // only so a phone that left can render its sentence with no button at all.
    expect(sub.state.retry).toBeTypeOf('function')
    sub.state.retry?.()
    await vi.waitFor(() => {
      expect(sub.state).toBeNull()
    })
    expect(connectPlace).toHaveBeenCalledTimes(2)
  })

  it('words a refusal for an account that is an email address from its own host and account', async () => {
    // Email logins are common on WebDAV hosts. A path that didn't parse put the
    // place's display name in for both, on every refused dial.
    info = {
      ...savedPlace,
      id: 'webdav-cloud-443-ada',
      name: 'Cloud',
      fsType: 'webdav',
      path: 'webdav://ada@example.com@cloud.example.com:443',
    }
    connectPlace.mockResolvedValue({ kind: 'refused', refusal: 'unreachable' })
    const { sub } = create()
    await vi.waitFor(() => {
      expect(sub.state?.kind).toBe('refused')
    })
    if (sub.state?.kind !== 'refused') throw new Error('not refused')
    expect(sub.state.refusal).toBe('unreachable for ada@example.com at cloud.example.com')
  })

  it('says nothing when the user cancels: the view just goes', async () => {
    connectPlace.mockResolvedValue({ kind: 'cancelled' })
    const { sub, enter } = create()
    await vi.waitFor(() => {
      expect(sub.state).toBeNull()
    })
    expect(enter).not.toHaveBeenCalled()
  })

  it('keeps the spinner while the reconnect manager owns the recovery', async () => {
    connectPlace.mockResolvedValue({ kind: 'reconnecting' })
    const { sub } = create()
    await vi.waitFor(() => {
      expect(connectPlace).toHaveBeenCalled()
    })
    expect(sub.state?.kind).toBe('connecting')
  })
})

/**
 * ❗ A saved SMB share comes back wherever its NEXT mount sat (`/Volumes/naspi-1`
 * when another server's `naspi` took the plain name), so the pane reloads there
 * rather than at the path the saved row remembered.
 */
describe('createPlaceConnect: a saved SMB share', () => {
  const savedShare: VolumeInfo = {
    id: 'smb-naspolya-445-naspi',
    name: 'naspi on Naspolya',
    path: '/Volumes/naspi',
    category: 'network',
    fsType: 'smbfs',
    isEjectable: false,
    connectionState: 'saved',
  }
  let dispose: (() => void) | undefined
  let shareInfo = $state<VolumeInfo>({ ...savedShare })
  let volumePath = $state('/Volumes/naspi')
  let currentPath = $state('/Volumes/naspi/docs')

  function create(landingOf = vi.fn(() => Promise.resolve<string | null>('/Volumes/naspi'))) {
    const enter = vi.fn()
    dispose = $effect.root(() => {
      createPlaceConnect({
        getVolumeId: () => savedShare.id,
        getCurrentVolumeInfo: () => shareInfo,
        getVolumePath: () => volumePath,
        getCurrentPath: () => currentPath,
        enter,
        landingOf,
      })
    })
    flushSync()
    return enter
  }

  beforeEach(() => {
    vi.clearAllMocks()
    shareInfo = { ...savedShare }
    volumePath = '/Volumes/naspi'
    currentPath = '/Volumes/naspi/docs'
    connectPlace.mockResolvedValue({ kind: 'connected', volumeId: savedShare.id })
  })

  afterEach(() => {
    dispose?.()
  })

  /**
   * ❗ The pane ENTERS the volume at the path the mount got, the same route a
   * switcher pick takes: root, path, listing, and disk space all move together.
   * Reloading only the listing left the pane's root at the old path, and its
   * missing-folder poll walked it to Macintosh HD (QA round 4, R3-A case 1).
   */
  it('enters the share where the mount landed when that moved, keeping the folder inside it', async () => {
    const enter = create(vi.fn(() => Promise.resolve<string | null>('/Volumes/naspi-1')))
    await vi.waitFor(() => {
      expect(enter).toHaveBeenCalledWith({
        volumeId: savedShare.id,
        volumePath: '/Volumes/naspi-1',
        targetPath: '/Volumes/naspi-1/docs',
      })
    })
  })

  /**
   * ❗ A pane whose volume and path disagree could write to the wrong server:
   * 11480's share listed at `/Volumes/public`, which 11482's mount held (QA round
   * 4, R3-A case 2). Whatever put it there, the pane follows its LIVE row's path.
   */
  it('follows a live share whose mount path differs from where the pane stands', () => {
    shareInfo = { ...savedShare, path: '/Volumes/naspi-1', category: 'attached_volume', connectionState: 'direct' }
    const enter = create()
    expect(connectPlace, 'a live share is not dialed').not.toHaveBeenCalled()
    expect(enter).toHaveBeenCalledWith({
      volumeId: savedShare.id,
      volumePath: '/Volumes/naspi-1',
      targetPath: '/Volumes/naspi-1/docs',
    })
  })

  it('leaves a live share alone when the pane already stands on its mount path', () => {
    shareInfo = { ...savedShare, category: 'attached_volume', connectionState: 'direct' }
    const enter = create()
    expect(enter).not.toHaveBeenCalled()
  })
})
