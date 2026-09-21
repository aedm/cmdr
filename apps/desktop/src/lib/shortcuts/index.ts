/**
 * Keyboard shortcuts module.
 * Re-exports all public APIs for shortcut customization.
 */

// Key capture
export { formatKeyCombo, physicalKeyCombo, isModifierKey, isMacOS, toDisplayShortcut } from './key-capture'

// Shortcuts store
export {
  initializeShortcuts,
  getEffectiveShortcuts,
  getDefaultShortcuts,
  isShortcutModified,
  setShortcut,
  addShortcut,
  removeShortcut,
  resetShortcut,
  resetAllShortcuts,
  onShortcutChange,
  flushPendingSave,
  isNativeShortcutCommand,
  isFixedKeyCommand,
  resyncMenuAccelerators,
} from './shortcuts-store'

// The whole registry as one map, for the native popup menus' accelerator labels
export { boundShortcuts } from './bound-shortcuts'

// Conflict detection
export { findConflictsForShortcut, getConflictCount, getConflictingCommandIds } from './conflict-detector'

// Event → command matching for local handlers (the document dispatcher imports
// `lookupCommand` / `init` / `destroy` from `shortcut-dispatch` directly).
export { eventMatchesCommand, comboMatchesCommand } from './shortcut-dispatch'

// How a local handler says it acted on a key, so the document dispatcher doesn't
// run the same command again.
export { claimKey } from './claim-key'

// MCP shortcuts listener
export { setupMcpShortcutsListener, cleanupMcpShortcutsListener } from './mcp-shortcuts-listener'
