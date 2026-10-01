/**
 * The S3 wrappers.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest'

vi.mock('$lib/ipc/bindings', () => ({
  commands: {
    saveS3Credentials: vi.fn(),
    hasS3Credentials: vi.fn(),
    deleteS3Credentials: vi.fn(),
    getS3UnattendedReconnect: vi.fn(),
    getKnownS3Places: vi.fn(),
  },
}))

import { commands, type S3ProviderChoice, type SavedS3Place } from '$lib/ipc/bindings'
import {
  deleteS3Credentials,
  getKnownS3Places,
  getS3UnattendedReconnect,
  hasS3Credentials,
  knownS3PlaceOf,
  saveS3Credentials,
} from './s3'

const AWS: S3ProviderChoice = { kind: 'aws', region: 'eu-west-1' }
const KEY = 'AKIAEXAMPLE'

const ok = { status: 'ok' as const, data: null }
const err = { status: 'error' as const, error: { type: 'access_denied' as const, message: 'nope' } }

beforeEach(() => {
  vi.clearAllMocks()
})

describe('the account secret', () => {
  it('saving forwards the provider, the key id, and the secret', async () => {
    vi.mocked(commands.saveS3Credentials).mockResolvedValueOnce(ok)
    await saveS3Credentials(AWS, KEY, 's3cr3t')
    expect(commands.saveS3Credentials).toHaveBeenCalledWith(AWS, KEY, 's3cr3t')
  })

  it('a refusing store throws rather than reporting success', async () => {
    vi.mocked(commands.saveS3Credentials).mockResolvedValueOnce(err)
    await expect(saveS3Credentials(AWS, KEY, 's3cr3t')).rejects.toThrow('nope')
  })

  it('deleting throws on refusal too', async () => {
    vi.mocked(commands.deleteS3Credentials).mockResolvedValueOnce(err)
    await expect(deleteS3Credentials(AWS, KEY)).rejects.toThrow('nope')
  })

  it('deleting forwards the account it forgets', async () => {
    vi.mocked(commands.deleteS3Credentials).mockResolvedValueOnce(ok)
    await deleteS3Credentials(AWS, KEY)
    expect(commands.deleteS3Credentials).toHaveBeenCalledWith(AWS, KEY)
  })

  it('asking whether one is stored is keyed per account', async () => {
    vi.mocked(commands.hasS3Credentials).mockResolvedValueOnce(true)
    expect(await hasS3Credentials(AWS, KEY)).toBe(true)
    expect(commands.hasS3Credentials).toHaveBeenCalledWith(AWS, KEY)
  })
})

describe('the saved places', () => {
  const photos: SavedS3Place = {
    volumeId: 's3-photos',
    provider: AWS,
    accessKeyId: KEY,
    bucket: 'photos',
    displayName: '',
    autoReconnect: true,
    pinned: false,
  }
  const root: SavedS3Place = { ...photos, volumeId: 's3-root', bucket: null }

  it('reads the list through', async () => {
    vi.mocked(commands.getKnownS3Places).mockResolvedValueOnce([photos, root])
    expect(await getKnownS3Places()).toEqual([photos, root])
  })

  it('finds the place a volume id names by the id the backend published, ❌ never a hash of its own', async () => {
    vi.mocked(commands.getKnownS3Places).mockResolvedValue([photos, root])
    expect((await knownS3PlaceOf('s3-root'))?.bucket).toBeNull()
    expect(await knownS3PlaceOf('s3-nothing')).toBeNull()
  })
})

describe('unattended reconnect', () => {
  it('passes the backend answer through, null included', async () => {
    vi.mocked(commands.getS3UnattendedReconnect).mockResolvedValueOnce('no_stored_secret')
    expect(await getS3UnattendedReconnect('s3-abc')).toBe('no_stored_secret')
    vi.mocked(commands.getS3UnattendedReconnect).mockResolvedValueOnce(null)
    expect(await getS3UnattendedReconnect('s3-gone')).toBeNull()
    expect(commands.getS3UnattendedReconnect).toHaveBeenCalledWith('s3-abc')
  })
})
