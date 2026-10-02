/**
 * Reactive store for MTP (Android device) state management.
 * Tracks connected devices, their connection status, and storages.
 */

import { SvelteMap } from 'svelte/reactivity'
import {
  type ConnectedMtpDeviceInfo,
  type MtpDeviceInfo,
  type MtpStorageInfo,
  type UnlistenFn,
  connectMtpDevice,
  disconnectMtpDevice,
  getMtpDeviceDisplayName,
  onMtpDeviceConnected,
  onMtpDeviceDisconnected,
  onMtpExclusiveAccessError,
  onMtpPermissionError,
} from '$lib/tauri-commands'
import { getAppLogger } from '$lib/logging/logger'
import { pluralize } from '$lib/utils/pluralize'
import { tString } from '$lib/intl/messages.svelte'

const logger = getAppLogger('mtp')

/** Connection state for a device. */
export type DeviceConnectionState = 'disconnected' | 'connecting' | 'connected' | 'error'

/** Extended device info with connection state. */
export interface MtpDeviceState {
  device: MtpDeviceInfo
  connectionState: DeviceConnectionState
  storages: MtpStorageInfo[]
  /** Error message if connectionState is 'error'. */
  error?: string
  /** Display name for the device. */
  displayName: string
}

/** Store state. */
interface MtpStoreState {
  /** Map of device ID to device state. */
  devices: SvelteMap<string, MtpDeviceState>
  /** Whether the store has been initialized. */
  initialized: boolean
}

// Reactive state using Svelte 5 runes
let state = $state<MtpStoreState>({
  devices: new SvelteMap(),
  initialized: false,
})

// Event listeners
let unlistenConnected: UnlistenFn | undefined
let unlistenDisconnected: UnlistenFn | undefined
let unlistenExclusiveAccess: UnlistenFn | undefined
let unlistenPermissionError: UnlistenFn | undefined

/**
 * Gets all devices as an array (for iteration in components).
 */
export function getDevices(): MtpDeviceState[] {
  return Array.from(state.devices.values())
}

/**
 * Gets a specific device by ID.
 */
export function getDevice(deviceId: string): MtpDeviceState | undefined {
  return state.devices.get(deviceId)
}

/**
 * Gets all connected devices.
 */
export function getConnectedDevices(): MtpDeviceState[] {
  return getDevices().filter((d) => d.connectionState === 'connected')
}

/**
 * Checks if any device is connected.
 */
export function hasConnectedDevices(): boolean {
  return getConnectedDevices().length > 0
}

/**
 * Checks if the store has been initialized.
 */
export function isInitialized(): boolean {
  return state.initialized
}

/**
 * Connects to an MTP device.
 * Updates the store with connection state and storages.
 */
export async function connect(deviceId: string): Promise<ConnectedMtpDeviceInfo | undefined> {
  const deviceState = state.devices.get(deviceId)
  if (!deviceState) {
    logger.warn('Cannot connect: device {deviceId} not found in store', { deviceId })
    return undefined
  }

  if (deviceState.connectionState === 'connected') {
    logger.debug('Device {deviceId} already connected', { deviceId })
    return { device: deviceState.device, storages: deviceState.storages }
  }

  if (deviceState.connectionState === 'connecting') {
    logger.debug('Device {deviceId} connection already in progress', { deviceId })
    return undefined
  }

  // Update state to connecting
  state.devices.set(deviceId, {
    ...deviceState,
    connectionState: 'connecting',
    error: undefined,
  })

  try {
    const result = await connectMtpDevice(deviceId)

    // Update state with connected info
    state.devices.set(deviceId, {
      ...deviceState,
      device: result.device,
      connectionState: 'connected',
      storages: result.storages,
      displayName: getMtpDeviceDisplayName(result.device),
      error: undefined,
    })

    logger.info('Connected to MTP device: {displayName}', { displayName: deviceState.displayName })
    return result
  } catch (error) {
    // Handle various error formats from Tauri
    let errorMessage: string
    if (error instanceof Error) {
      errorMessage = error.message
    } else if (typeof error === 'object' && error !== null) {
      // Tauri errors often come as objects with message or userMessage
      const errObj = error as Record<string, unknown>
      errorMessage = (errObj.userMessage as string) || (errObj.message as string) || JSON.stringify(error)
    } else {
      errorMessage = String(error)
    }

    state.devices.set(deviceId, {
      ...deviceState,
      connectionState: 'error',
      error: errorMessage,
    })

    logger.error('Failed to connect to {displayName}: {error}', {
      displayName: deviceState.displayName,
      error: errorMessage,
    })
    throw error
  }
}

/**
 * Disconnects from an MTP device.
 */
export async function disconnect(deviceId: string): Promise<void> {
  const deviceState = state.devices.get(deviceId)
  if (!deviceState) {
    logger.warn('Cannot disconnect: device {deviceId} not found in store', { deviceId })
    return
  }

  if (deviceState.connectionState === 'disconnected') {
    logger.debug('Device {deviceId} already disconnected', { deviceId })
    return
  }

  try {
    await disconnectMtpDevice(deviceId)

    state.devices.set(deviceId, {
      ...deviceState,
      connectionState: 'disconnected',
      storages: [],
      error: undefined,
    })

    logger.info('Disconnected from MTP device: {displayName}', { displayName: deviceState.displayName })
  } catch (error) {
    logger.error('Failed to disconnect from {displayName}: {error}', {
      displayName: deviceState.displayName,
      error: String(error),
    })
    throw error
  }
}

/**
 * Puts a device into the `error` state with `error` as its message. The device may be
 * unknown to the store (the backend reports access problems before a connect), so a
 * placeholder `MtpDeviceInfo` stands in until a real one arrives.
 */
function markDeviceError(deviceId: string, error: string): void {
  const existing = state.devices.get(deviceId)
  const device = existing?.device ?? {
    id: deviceId,
    locationId: 0,
    vendorId: 0,
    productId: 0,
  }
  state.devices.set(deviceId, {
    device,
    connectionState: 'error',
    storages: [],
    displayName: existing?.displayName ?? getMtpDeviceDisplayName(device),
    error,
  })
}

/**
 * Initializes the MTP store.
 * Sets up event listeners for device connection state tracking.
 * The backend handles device detection and auto-connection; this store
 * is a passive consumer that tracks connection state for UI purposes.
 * Should be called once when the app starts.
 */
export async function initialize(): Promise<void> {
  if (state.initialized) return

  // Track device connections (backend auto-connects on USB hotplug)
  unlistenConnected = await onMtpDeviceConnected((event) => {
    // Ensure device exists in store, or create it
    const existing = state.devices.get(event.deviceId)
    const device = existing?.device ?? {
      id: event.deviceId,
      locationId: 0,
      vendorId: 0,
      productId: 0,
    }
    state.devices.set(event.deviceId, {
      device,
      connectionState: 'connected',
      storages: event.storages,
      displayName: existing?.displayName ?? getMtpDeviceDisplayName(device),
    })
    logger.info('MTP device connected: {deviceId} ({count} {storagesNoun})', {
      deviceId: event.deviceId,
      count: event.storages.length,
      storagesNoun: pluralize(event.storages.length, 'storage'),
    })
  })

  unlistenDisconnected = await onMtpDeviceDisconnected((event) => {
    const deviceState = state.devices.get(event.deviceId)
    if (deviceState) {
      state.devices.set(event.deviceId, {
        ...deviceState,
        connectionState: 'disconnected',
        storages: [],
      })
      logger.info('Device {displayName} disconnected ({reason})', {
        displayName: deviceState.displayName,
        reason: event.reason,
      })
    }
  })

  unlistenExclusiveAccess = await onMtpExclusiveAccessError((event) => {
    markDeviceError(event.deviceId, tString('mtp.error.exclusiveAccess', { blocking: event.blockingProcess || 'none' }))
  })

  unlistenPermissionError = await onMtpPermissionError((event) => {
    markDeviceError(event.deviceId, tString('mtp.error.permissionDenied'))
  })

  state.initialized = true
  logger.debug('MTP store initialized')
}

/** Resets all state to initial values. For use in tests only. */
export function resetForTesting(): void {
  state = {
    devices: new SvelteMap(),
    initialized: false,
  }
  unlistenConnected = undefined
  unlistenDisconnected = undefined
  unlistenExclusiveAccess = undefined
  unlistenPermissionError = undefined
}

/**
 * Cleans up the MTP store.
 * Should be called when the app is shutting down.
 */
export function cleanup(): void {
  unlistenConnected?.()
  unlistenDisconnected?.()
  unlistenExclusiveAccess?.()
  unlistenPermissionError?.()

  state = {
    devices: new SvelteMap(),
    initialized: false,
  }
}
