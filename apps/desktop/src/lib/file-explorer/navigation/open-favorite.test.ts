/**
 * The one way a favorite opens.
 *
 * Three things are pinned here and nowhere else: the pane lands on the
 * CONTAINING volume (never the favorite's own virtual id) with the favorite's
 * path as the destination, an unresolvable favorite navigates NOWHERE (rather
 * than evicting the pane to `/` with a junk path, which is what the switcher's
 * branch used to do), and `favorite_opened` fires exactly once per open and not
 * at all on the refusal.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'
import type { VolumeInfo } from '$lib/file-explorer/types'

const resolvePathVolume = vi.fn<(path: string) => Promise<{ volume: VolumeInfo | null; timedOut: boolean }>>()
const trackEvent = vi.fn()

vi.mock('$lib/tauri-commands', () => ({
  resolvePathVolume: (path: string) => resolvePathVolume(path),
  trackEvent: (...args: unknown[]) => {
    trackEvent(...(args as []))
    return Promise.resolve()
  },
}))

const warn = vi.fn()
vi.mock('$lib/logging/logger', () => ({
  getAppLogger: () => ({
    warn: (...args: unknown[]) => {
      warn(...(args as []))
    },
  }),
}))

import { openFavorite } from './open-favorite'

const drive: VolumeInfo = {
  id: 'disk1',
  name: 'Macintosh HD',
  path: '/',
  category: 'main_volume',
  isEjectable: false,
}

beforeEach(() => {
  resolvePathVolume.mockReset()
  trackEvent.mockReset()
  warn.mockReset()
})

describe('openFavorite', () => {
  it('sends the pane to the CONTAINING volume, at the favorite`s path', async () => {
    resolvePathVolume.mockResolvedValue({ volume: drive, timedOut: false })
    const go = vi.fn(() => 'went')

    const result = await openFavorite({
      favoritePath: '/Users/me/projects',
      picked: { surface: 'favorites_menu', via: 'digit' },
      go,
    })

    expect(resolvePathVolume).toHaveBeenCalledWith('/Users/me/projects')
    // The favorite's own `fav-…` id never reaches the pane: it's a virtual row,
    // and the pane has to end up on a volume that can list.
    expect(go).toHaveBeenCalledWith({ volumeId: 'disk1', volumePath: '/', targetPath: '/Users/me/projects' })
    expect(result).toEqual({ kind: 'opened', opened: 'went' })
  })

  it('passes whatever `go` returns straight back, so each caller keeps its own outcome type', async () => {
    resolvePathVolume.mockResolvedValue({ volume: drive, timedOut: false })
    const outcome = { kind: 'selected' as const, volumeId: 'disk1' }

    const result = await openFavorite({
      favoritePath: '/Users/me',
      picked: { surface: 'command', via: 'command' },
      go: () => outcome,
    })

    expect(result).toEqual({ kind: 'opened', opened: outcome })
  })

  it('emits `favorite_opened` once, carrying the caller`s surface and via', async () => {
    resolvePathVolume.mockResolvedValue({ volume: drive, timedOut: false })

    await openFavorite({
      favoritePath: '/Users/me',
      picked: { surface: 'favorites_menu', via: 'pointer' },
      go: () => undefined,
    })

    expect(trackEvent).toHaveBeenCalledTimes(1)
    expect(trackEvent).toHaveBeenCalledWith('favorite_opened', { surface: 'favorites_menu', via: 'pointer' })
  })

  it('navigates NOWHERE when no volume claims the path', async () => {
    // A dead `search-results://` snapshot id, or a share that just went away. The
    // old switcher branch sent the pane to volume `root` at `/` carrying the raw
    // path, which evicts the user and then errors about a path they never typed.
    resolvePathVolume.mockResolvedValue({ volume: null, timedOut: false })
    const go = vi.fn()

    const result = await openFavorite({
      favoritePath: 'search-results://dead-id',
      picked: { surface: 'favorites_menu', via: 'keyboard' },
      go,
    })

    expect(result).toEqual({ kind: 'unresolved' })
    expect(go).not.toHaveBeenCalled()
    expect(warn).toHaveBeenCalledTimes(1)
  })

  it('reports nothing when it refused: `favorite_opened` counts arrivals', async () => {
    resolvePathVolume.mockResolvedValue({ volume: null, timedOut: true })

    await openFavorite({
      favoritePath: '/Volumes/gone/docs',
      picked: { surface: 'command', via: 'command' },
      go: () => undefined,
    })

    expect(trackEvent).not.toHaveBeenCalled()
  })
})
