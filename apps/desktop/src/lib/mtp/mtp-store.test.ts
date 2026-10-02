/**
 * Tests for MTP store reactive behavior and device state management.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest'

vi.mock('$lib/tauri-commands', async () => {
  const { getMtpDeviceDisplayName } = await import('$lib/tauri-commands/mtp')
  return {
    getMtpDeviceDisplayName,
    connectMtpDevice: vi.fn(),
    disconnectMtpDevice: vi.fn(),
    onMtpDeviceConnected: vi.fn(),
    onMtpDeviceDisconnected: vi.fn(),
    onMtpExclusiveAccessError: vi.fn(),
    onMtpPermissionError: vi.fn(),
  }
})

import type { MtpDeviceInfo, MtpStorageInfo, ConnectedMtpDeviceInfo } from '$lib/tauri-commands'
import {
  connectMtpDevice,
  disconnectMtpDevice,
  onMtpDeviceConnected,
  onMtpDeviceDisconnected,
  onMtpExclusiveAccessError,
  onMtpPermissionError,
  type MtpDeviceConnectedEvent,
  type MtpDeviceDisconnectedEvent,
  type MtpExclusiveAccessErrorEvent,
} from '$lib/tauri-commands'
import {
  getDevices,
  getDevice,
  getConnectedDevices,
  hasConnectedDevices,
  isInitialized,
  connect,
  disconnect,
  initialize,
  cleanup,
  resetForTesting,
} from './mtp-store.svelte'

const mockDevice: MtpDeviceInfo = {
  id: 'mtp-336592896',
  locationId: 336592896,
  vendorId: 0x18d1,
  productId: 0x4ee1,
  manufacturer: 'Google',
  product: 'Pixel 8',
}

const mockStorage: MtpStorageInfo = {
  id: 65537,
  name: 'Internal shared storage',
  totalBytes: 128_000_000_000,
  availableBytes: 64_000_000_000,
  storageType: 'FixedRAM',
  isReadOnly: false,
}

const mockConnectedInfo: ConnectedMtpDeviceInfo = {
  device: mockDevice,
  storages: [mockStorage],
}

/**
 * Puts `mockDevice` in the store the way the backend can: an access problem reported before a connect, which leaves
 * it in `error` and so open to a manual `connect`.
 */
async function seedErroredDevice(): Promise<void> {
  let exclusiveCallback: ((event: MtpExclusiveAccessErrorEvent) => void) | undefined
  vi.mocked(onMtpExclusiveAccessError).mockImplementation((callback) => {
    exclusiveCallback = callback
    return Promise.resolve(vi.fn())
  })
  await initialize()
  exclusiveCallback?.({ deviceId: mockDevice.id, blockingProcess: 'ptpcamerad' })
}

describe('mtp-store', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    resetForTesting()

    // Default mock for event listeners - return unlisten functions
    vi.mocked(onMtpDeviceConnected).mockResolvedValue(vi.fn())
    vi.mocked(onMtpDeviceDisconnected).mockResolvedValue(vi.fn())
    vi.mocked(onMtpExclusiveAccessError).mockResolvedValue(vi.fn())
    vi.mocked(onMtpPermissionError).mockResolvedValue(vi.fn())
  })

  describe('initial state', () => {
    it('returns empty devices before initialization', () => {
      expect(getDevices()).toEqual([])
      expect(isInitialized()).toBe(false)
    })

    it('has no connected devices initially', () => {
      expect(hasConnectedDevices()).toBe(false)
      expect(getConnectedDevices()).toEqual([])
    })
  })

  describe('connect', () => {
    it('connects a known device and updates state', async () => {
      vi.mocked(connectMtpDevice).mockResolvedValue(mockConnectedInfo)
      await seedErroredDevice()

      await connect('mtp-336592896')

      const device = getDevice('mtp-336592896')
      expect(device?.connectionState).toBe('connected')
      expect(device?.storages).toEqual([mockStorage])
      expect(device?.displayName).toBe('Pixel 8')
      expect(hasConnectedDevices()).toBe(true)
      expect(getConnectedDevices()).toHaveLength(1)
    })

    it('returns undefined for unknown device', async () => {
      const result = await connect('mtp-unknown')

      expect(result).toBeUndefined()
    })

    it('returns existing info for already connected device', async () => {
      vi.mocked(connectMtpDevice).mockResolvedValue(mockConnectedInfo)
      await seedErroredDevice()
      await connect('mtp-336592896')

      const result = await connect('mtp-336592896')

      expect(result).toBeDefined()
      expect(connectMtpDevice).toHaveBeenCalledTimes(1) // Should not call again
    })

    it('sets error state on connect failure', async () => {
      vi.mocked(connectMtpDevice).mockRejectedValue(new Error('Exclusive access error'))
      await seedErroredDevice()

      await expect(connect('mtp-336592896')).rejects.toThrow('Exclusive access error')

      const device = getDevice('mtp-336592896')
      expect(device?.connectionState).toBe('error')
      expect(device?.error).toBe('Exclusive access error')
    })
  })

  describe('disconnect', () => {
    it('disconnects from a device and clears storages', async () => {
      vi.mocked(connectMtpDevice).mockResolvedValue(mockConnectedInfo)
      vi.mocked(disconnectMtpDevice).mockResolvedValue(undefined)
      await seedErroredDevice()
      await connect('mtp-336592896')
      expect(hasConnectedDevices()).toBe(true)

      await disconnect('mtp-336592896')

      const device = getDevice('mtp-336592896')
      expect(device?.connectionState).toBe('disconnected')
      expect(device?.storages).toEqual([])
      expect(hasConnectedDevices()).toBe(false)
    })

    it('handles disconnect for unknown device gracefully', async () => {
      // Should not throw
      await disconnect('mtp-unknown')
    })

    it('handles double disconnect gracefully (only calls backend once)', async () => {
      vi.mocked(connectMtpDevice).mockResolvedValue(mockConnectedInfo)
      vi.mocked(disconnectMtpDevice).mockResolvedValue(undefined)
      await seedErroredDevice()
      await connect('mtp-336592896')

      await disconnect('mtp-336592896')
      expect(disconnectMtpDevice).toHaveBeenCalledTimes(1)

      // Second disconnect - should not call backend again
      await disconnect('mtp-336592896')
      expect(disconnectMtpDevice).toHaveBeenCalledTimes(1)
    })
  })

  describe('initialize', () => {
    it('sets up event listeners (passive consumer, no scan)', async () => {
      await initialize()

      expect(isInitialized()).toBe(true)
      // No scan: backend auto-connects devices, store is passive
      expect(getDevices()).toHaveLength(0)
      expect(onMtpDeviceConnected).toHaveBeenCalledWith(expect.any(Function))
      expect(onMtpDeviceDisconnected).toHaveBeenCalledWith(expect.any(Function))
      expect(onMtpExclusiveAccessError).toHaveBeenCalledWith(expect.any(Function))
      expect(onMtpPermissionError).toHaveBeenCalledWith(expect.any(Function))
    })

    it('is idempotent (only initializes once)', async () => {
      await initialize()
      await initialize()

      // Event listeners should only be registered once
      expect(onMtpDeviceConnected).toHaveBeenCalledTimes(1)
    })
  })

  describe('cleanup', () => {
    it('unregisters event listeners and resets state', async () => {
      const unlistenConnected = vi.fn()
      const unlistenDisconnected = vi.fn()
      const unlistenExclusive = vi.fn()
      const unlistenPermission = vi.fn()

      vi.mocked(onMtpDeviceConnected).mockResolvedValue(unlistenConnected)
      vi.mocked(onMtpDeviceDisconnected).mockResolvedValue(unlistenDisconnected)
      vi.mocked(onMtpExclusiveAccessError).mockResolvedValue(unlistenExclusive)
      vi.mocked(onMtpPermissionError).mockResolvedValue(unlistenPermission)

      await initialize()
      expect(isInitialized()).toBe(true)

      cleanup()

      expect(unlistenConnected).toHaveBeenCalled()
      expect(unlistenDisconnected).toHaveBeenCalled()
      expect(unlistenExclusive).toHaveBeenCalled()
      expect(unlistenPermission).toHaveBeenCalled()
      expect(isInitialized()).toBe(false)
      expect(getDevices()).toHaveLength(0)
    })
  })

  describe('event handling', () => {
    it('updates state on mtp-device-connected event', async () => {
      let connectedCallback: ((event: MtpDeviceConnectedEvent) => void) | undefined
      vi.mocked(onMtpDeviceConnected).mockImplementation((callback) => {
        connectedCallback = callback
        return Promise.resolve(vi.fn())
      })

      await initialize()

      // Simulate event from backend
      connectedCallback?.({ deviceId: 'mtp-336592896', deviceName: 'Pixel', storages: [mockStorage] })

      const device = getDevice('mtp-336592896')
      expect(device?.connectionState).toBe('connected')
      expect(device?.storages).toEqual([mockStorage])
    })

    it('updates state on mtp-device-disconnected event', async () => {
      let connectedCallback: ((event: MtpDeviceConnectedEvent) => void) | undefined
      let disconnectedCallback: ((event: MtpDeviceDisconnectedEvent) => void) | undefined
      vi.mocked(onMtpDeviceConnected).mockImplementation((callback) => {
        connectedCallback = callback
        return Promise.resolve(vi.fn())
      })
      vi.mocked(onMtpDeviceDisconnected).mockImplementation((callback) => {
        disconnectedCallback = callback
        return Promise.resolve(vi.fn())
      })

      await initialize()

      // Simulate backend auto-connect (populates the store)
      connectedCallback?.({ deviceId: 'mtp-336592896', deviceName: 'Pixel', storages: [mockStorage] })
      expect(getDevice('mtp-336592896')?.connectionState).toBe('connected')

      // Simulate disconnect event from backend
      disconnectedCallback?.({ deviceId: 'mtp-336592896', reason: 'removed' })

      const device = getDevice('mtp-336592896')
      expect(device?.connectionState).toBe('disconnected')
      expect(device?.storages).toEqual([])
    })

    it('sets error state on mtp-exclusive-access-error event', async () => {
      let exclusiveCallback: ((event: MtpExclusiveAccessErrorEvent) => void) | undefined
      vi.mocked(onMtpExclusiveAccessError).mockImplementation((callback) => {
        exclusiveCallback = callback
        return Promise.resolve(vi.fn())
      })

      await initialize()

      // Simulate event from backend
      exclusiveCallback?.({ deviceId: 'mtp-336592896', blockingProcess: 'ptpcamerad' })

      const device = getDevice('mtp-336592896')
      expect(device?.connectionState).toBe('error')
      expect(device?.error).toContain('ptpcamerad')
    })

    // Note: mtp-device-detected and mtp-device-removed events are no longer
    // handled by the frontend store; the backend auto-connects/disconnects
    // and emits mtp-device-connected/mtp-device-disconnected instead.
  })
})
