/**
 * User-friendly error message generation for transfer (copy/move) operations.
 * Extracted from TransferErrorDialog.svelte for testability.
 *
 * Error classification happens on the backend: each WriteOperationError variant
 * carries structured data, so the frontend just maps variant → user-facing text.
 * No string parsing needed.
 *
 * The literal English lives in the `errors.write.*` message catalog and is pulled
 * via `getMessage()` (a RAW catalog lookup, never ICU `t()`): these strings carry
 * interpolated paths/sizes/HTML escaping and bypass ICU's brace/apostrophe
 * grammar (so the catalog values use normal apostrophes, not doubled). This
 * file keeps the COMPOSITION (escaping, size colorizing, platform branches)
 * and selects per-operation message variants by key suffix
 * (`<field>.${operationType}`), so each language phrases each operation
 * naturally. See `$lib/intl`'s docs.
 */
import type { WriteOperationError, TransferOperationType, FriendlyError } from '$lib/file-explorer/types'
import type { TrashRefusalKind } from '$lib/ipc/bindings'
import { fdaIsMissing } from '$lib/onboarding/fda-status.svelte'
import type { ProgressAtStop } from '$lib/tauri-commands'
import { formatInteger } from '$lib/intl/number-format'
import { isMacOS } from '$lib/shortcuts/key-capture'
import { getEffectiveShortcuts, toDisplayShortcut } from '$lib/shortcuts'
import { colorizeSizeString } from '$lib/file-explorer/selection/selection-info-utils'
import { escapeHtml } from '$lib/tooltip/tooltip'
import { getMessage } from '$lib/intl/messages.svelte'
import type { MessageKey } from '$lib/intl/keys.gen'
import { formatByteSize } from '$lib/units'

/** Substitutes `{token}` placeholders in a catalog value with runtime strings. */
function interpolate(template: string, params: Record<string, string> = {}): string {
  let out = template
  for (const [name, value] of Object.entries(params)) {
    out = out.replaceAll(`{${name}}`, value)
  }
  return out
}

/** Raw catalog lookup for an `errors.write.*` key (no ICU). */
function w(key: string, params?: Record<string, string>): string {
  const value = getMessage(`errors.write.${key}` as MessageKey)
  return params ? interpolate(value, params) : value
}

export interface FriendlyErrorMessage {
  /** Short title for the error */
  title: string
  /** Main explanation of what happened */
  message: string
  /** Suggestion for what the user can do */
  suggestion: string
}

/** Simple error messages that only depend on the operation, not on error-specific fields. */
const simpleMessageFactories: Partial<
  Record<WriteOperationError['type'], (op: TransferOperationType) => FriendlyErrorMessage>
> = {
  source_not_found: (op) => ({
    title: w('sourceNotFound.title'),
    message: w(`sourceNotFound.message.${op}`),
    suggestion: w('sourceNotFound.suggestion'),
  }),
  destination_exists: () => ({
    title: w('destinationExists.title'),
    message: w('destinationExists.message'),
    suggestion: w('destinationExists.suggestion'),
  }),
  // The destination folder is gone, so nothing was written. Deliberately worded
  // nothing like `source_not_found`: the two used to share that variant, and
  // telling someone their source file vanished when it's sitting untouched is
  // the one wrong answer that starts a data-loss panic. Only copy and move
  // reach it (delete/trash have no destination), so `${op}` never resolves past
  // `.copy` / `.move`.
  destination_not_found: (op) => ({
    title: w('destinationNotFound.title'),
    message: w(`destinationNotFound.message.${op}`),
    suggestion: w('destinationNotFound.suggestion'),
  }),
  // A phone or saved server nobody has connected yet, refused before anything
  // was read or written. ❌ Never worded as a disconnect: nothing dropped. The
  // sentence names the half that isn't connected, and holds for every operation
  // that reaches it (a source for copy, move, and delete; a destination for copy,
  // move, and compress), so it doesn't vary by `op`.
  source_not_connected: () => ({
    title: w('notConnected.title'),
    message: w('notConnected.message.source'),
    suggestion: w('notConnected.suggestion'),
  }),
  destination_not_connected: () => ({
    title: w('notConnected.title'),
    message: w('notConnected.message.destination'),
    suggestion: w('notConnected.suggestion'),
  }),
  // destinationInsideSource can only happen on copy/move (delete/trash have no
  // destination), so `${op}` only ever resolves to `.copy` or `.move` here.
  destination_inside_source: (op) => ({
    title: w(`destinationInsideSource.title.${op}`),
    message: w(`destinationInsideSource.message.${op}`),
    suggestion: w(`destinationInsideSource.suggestion.${op}`),
  }),
  symlink_loop: () => ({
    title: w('symlinkLoop.title'),
    message: w('symlinkLoop.message'),
    suggestion: w('symlinkLoop.suggestion'),
  }),
  cancelled: (op) => ({
    title: w(`cancelled.title.${op}`),
    message: w(`cancelled.message.${op}`),
    suggestion: w('cancelled.suggestion'),
  }),
  trash_not_supported: () => {
    // Interpolate the live `file.deletePermanently` binding (platform-formatted)
    // at message-build time. Snapshot semantics are right here: a transient error
    // string isn't a live-updating surface. Falls back to the default if unbound.
    const deletePermanentlyKey = toDisplayShortcut(
      getEffectiveShortcuts('file.deletePermanently')[0] ?? (isMacOS() ? '⇧F8' : 'Shift+F8'),
    )
    return {
      title: w('trashNotSupported.title'),
      message: w('trashNotSupported.message'),
      suggestion: w('trashNotSupported.suggestion', { deletePermanentlyKey }),
    }
  },
  connection_interrupted: () => ({
    title: w('connectionInterrupted.title'),
    message: w('connectionInterrupted.message'),
    suggestion: w('connectionInterrupted.suggestion'),
  }),
  // The destination refused a write for lack of room or quota. Nothing measured
  // it, so the copy names no sizes (a measured refusal is `insufficient_space`).
  destination_full: () => ({
    title: w('destinationFull.title'),
    message: w('destinationFull.message'),
    suggestion: w('destinationFull.suggestion'),
  }),
  // STATUS_DELETE_PENDING: the file is marked for deletion on the server but an
  // open handle is keeping it alive. Transient: retry-after-a-moment.
  delete_pending: () => ({
    title: w('deletePending.title'),
    message: w('deletePending.message'),
    suggestion: w('deletePending.suggestion'),
  }),
  read_error: (op) => ({
    title: w(`readError.title.${op}`),
    message: w('readError.message'),
    suggestion: w('readError.suggestion'),
  }),
  write_error: (op) => ({
    title: w(`writeError.title.${op}`),
    message: w('writeError.message'),
    suggestion: w('writeError.suggestion'),
  }),
  name_too_long: () => ({
    title: w('nameTooLong.title'),
    message: w('nameTooLong.message'),
    suggestion: w('nameTooLong.suggestion'),
  }),
  io_error: (op) => ({
    title: w(`ioError.title.${op}`),
    message: w(`ioError.message.${op}`),
    suggestion: w('ioError.suggestion'),
  }),
  file_locked: () => ({
    title: w('fileLocked.title'),
    message: w('fileLocked.message'),
    suggestion: isMacOS() ? w('fileLocked.suggestion.mac') : w('fileLocked.suggestion.other'),
  }),
}

/** Drives the dialog's icon, container tint, and Retry-button visibility. */
export interface ErrorDisplayMeta {
  category: FriendlyError['category']
  retryHint: boolean
}

/**
 * Per-variant category + retryHint, mirrored verbatim from the values the Rust
 * write-error mapper assigned per `WriteOperationError` variant. The backend
 * ships only the typed variant; this is the FE side of that classification. The
 * dialog renders a Retry button when the category is `transient` or `retryHint`
 * is true. A `Record` keyed by every variant makes adding a variant a compile
 * error here, keeping the table exhaustive.
 *
 * `device_disconnected` keeps `retryHint: true` for the operation dialog (retry
 * the move/copy after reconnecting), unlike the listing path which shows no Retry.
 */
const errorDisplayMetaMap: Record<WriteOperationError['type'], ErrorDisplayMeta> = {
  cancelled: { category: 'transient', retryHint: true },
  connection_interrupted: { category: 'transient', retryHint: true },
  delete_pending: { category: 'transient', retryHint: true },
  device_disconnected: { category: 'needs_action', retryHint: true },
  // Retry: nothing is broken and nothing was lost. The originals are all where
  // they were, so running the same move again is exactly the way out.
  move_not_confirmed: { category: 'needs_action', retryHint: true },
  read_error: { category: 'serious', retryHint: true },
  write_error: { category: 'serious', retryHint: true },
  io_error: { category: 'serious', retryHint: true },
  // No Retry. The two reasons that reach here in practice are permission-shaped, and
  // the identical request can only be refused the same way; offering Retry is what the
  // old `io_error` wording did, and it was the useless half of the dialog.
  trash_refused: { category: 'needs_action', retryHint: false },
  symlink_loop: { category: 'serious', retryHint: false },
  source_not_found: { category: 'needs_action', retryHint: false },
  // No Retry: the folder is missing, so the identical request can only fail
  // again. The way out is picking another destination or restoring the folder.
  destination_not_found: { category: 'needs_action', retryHint: false },
  // No Retry: the same request refuses again until the phone or server is
  // opened in a pane, which is what the suggestion asks for.
  source_not_connected: { category: 'needs_action', retryHint: false },
  destination_not_connected: { category: 'needs_action', retryHint: false },
  destination_exists: { category: 'needs_action', retryHint: false },
  permission_denied: { category: 'needs_action', retryHint: false },
  insufficient_space: { category: 'needs_action', retryHint: false },
  destination_full: { category: 'needs_action', retryHint: false },
  destination_inside_source: { category: 'needs_action', retryHint: false },
  // No Retry: the same selection can only be refused again. The way out is
  // picking one of the two, or transferring them one at a time.
  duplicate_source_names: { category: 'needs_action', retryHint: false },
  read_only_device: { category: 'needs_action', retryHint: false },
  // No Retry: the same folder refuses again. The way out is another destination.
  destination_not_writable: { category: 'needs_action', retryHint: false },
  file_locked: { category: 'needs_action', retryHint: false },
  trash_not_supported: { category: 'needs_action', retryHint: false },
  name_too_long: { category: 'needs_action', retryHint: false },
  invalid_name: { category: 'needs_action', retryHint: false },
  files_too_large_for_filesystem: { category: 'needs_action', retryHint: false },
  // No Retry: nothing is broken to retry. The new file is written and complete,
  // it is just under a different name, and the one move left is the user's
  // (renaming it), which the suggestion spells out.
  new_data_kept_at: { category: 'needs_action', retryHint: false },
  // No Retry: the copy stopped for its own reason, and re-running it would land
  // on the folder that's already there. The move left is the user's, once
  // they've decided what to do with the folder.
  originals_kept_aside: { category: 'needs_action', retryHint: false },
  // A password-protected archive source. The FE prompts for a password and
  // retries, so this classification is only the fallback if the prompt is
  // bypassed; retryHint stays on so the generic dialog still offers a retry.
  archive_needs_password: { category: 'needs_action', retryHint: true },
}

export function getErrorDisplayMeta(error: WriteOperationError): ErrorDisplayMeta {
  return errorDisplayMetaMap[error.type]
}

/**
 * Builds the message for the too-large-for-filesystem error. Only FAT32 produces
 * it (the one common format with a hard 4 GiB per-file cap), so the prose names
 * FAT32 directly; the offending files are listed separately by
 * `FallbackErrorContent` from `error.files`. Split out to keep
 * `getUserFriendlyMessage` under the complexity ceiling.
 */
function tooLargeForFilesystemMessage(
  error: Extract<WriteOperationError, { type: 'files_too_large_for_filesystem' }>,
): FriendlyErrorMessage {
  const maxSize = colorizeSizeString(formatByteSize(error.maxSize))
  if (error.totalCount === 1) {
    const file = error.files[0]
    return {
      title: w('filesTooLargeForFilesystem.title.one'),
      message: w('filesTooLargeForFilesystem.message.one', {
        name: escapeHtml(file.name),
        size: colorizeSizeString(formatByteSize(file.size)),
        maxSize,
      }),
      suggestion: w('filesTooLargeForFilesystem.suggestion'),
    }
  }
  return {
    title: w('filesTooLargeForFilesystem.title.many'),
    message: w('filesTooLargeForFilesystem.message.many', {
      count: String(error.totalCount),
      maxSize,
    }),
    suggestion: w('filesTooLargeForFilesystem.suggestion'),
  }
}

/**
 * The read-only refusal, worded for the half that actually refused.
 *
 * A read-only DESTINATION means "put it somewhere else". A read-only SOURCE
 * means "you can copy out of here, you just can't move out of here", because a
 * move needs a delete the source will never do: a repo's virtual `.git` history,
 * or a tar/7z archive. Telling that second user to choose a different
 * destination would send them to fix the half that was fine.
 *
 * The backend states the side; ❌ never infer it from the path. Split out to
 * keep `getUserFriendlyMessage` under the complexity ceiling.
 */
function readOnlyMessage(error: Extract<WriteOperationError, { type: 'read_only_device' }>): FriendlyErrorMessage {
  const side = error.side === 'source' ? 'source' : 'destination'
  return {
    title: w(`readOnlyDevice.${side}.title`),
    message: w(`readOnlyDevice.${side}.message`, {
      deviceName: escapeHtml(error.deviceName ?? w(`readOnlyDevice.${side}.fallbackName`)),
    }),
    suggestion: w(`readOnlyDevice.${side}.suggestion`),
  }
}

/**
 * A refused write, worded for the folder that actually refused and the kind of
 * refusal it was.
 *
 * Two facts the backend proves and ❌ nothing here guesses. `refusedFolder` is a
 * folder it asked the OS about with `access(W_OK)` at the moment of the refusal;
 * when it's absent nothing could be proved, and the message falls back to the
 * per-operation sentence. `refusal` comes from the errno: `folderPermissions` is a
 * folder an administrator could write to, `systemProtected` is macOS itself saying
 * no, where administrator rights change nothing, and sending someone chasing
 * permissions for that is a wrong answer they'd act on.
 *
 * The `unclassified` arm keeps the older split, which is per-operation and
 * platform: a refused delete sends a macOS user to a locked file, everyone else to
 * permissions, and a refused copy to the destination.
 */
function permissionDeniedMessage(
  error: Extract<WriteOperationError, { type: 'permission_denied' }>,
  op: TransferOperationType,
): FriendlyErrorMessage {
  const mac = isMacOS()
  const suggestionKey =
    error.refusal === 'folderPermissions'
      ? mac
        ? 'needsAdminMac'
        : 'needsAdminOther'
      : error.refusal === 'systemProtected'
        ? mac
          ? 'systemProtectedMac'
          : 'systemProtectedOther'
        : op === 'delete' || op === 'trash'
          ? mac
            ? 'deleteMac'
            : 'deleteOther'
          : 'default'
  return {
    title: w('permissionDenied.title'),
    message: error.refusedFolder
      ? w('permissionDenied.message.named', { folder: escapeHtml(error.refusedFolder) })
      : w(`permissionDenied.message.${op}`),
    suggestion: w(`permissionDenied.suggestion.${suggestionKey}`),
  }
}

/**
 * The refusal for a destination folder that takes no writes, found out before
 * anything was created or its space measured. One sentence per reason the backend
 * could tell apart. ❗ Every sentence is about the FOLDER, never the device (a
 * phone's `/` refuses writes while its shared storage takes them), and
 * `unexplained` stays neutral rather than claiming read-only or a permission.
 */
function destinationNotWritableMessage(
  error: Extract<WriteOperationError, { type: 'destination_not_writable' }>,
): FriendlyErrorMessage {
  return {
    title: w(`destinationNotWritable.${error.reason}.title`),
    message: w(`destinationNotWritable.${error.reason}.message`),
    suggestion: w(`destinationNotWritable.${error.reason}.suggestion`),
  }
}

/**
 * Builds the message for a copy that stopped with one of the user's files
 * renamed out of a folder's way.
 *
 * Two facts, in that order: their file still exists and here is where, then why
 * the copy stopped. The second half comes from the cause's own message, so a
 * full disk still reads as a full disk instead of being flattened into "the copy
 * stopped". Split out to keep `getUserFriendlyMessage` under the complexity
 * ceiling.
 */
function originalsKeptAsideMessage(
  error: Extract<WriteOperationError, { type: 'originals_kept_aside' }>,
  operationType: TransferOperationType,
): FriendlyErrorMessage {
  const cause = getUserFriendlyMessage(error.cause, operationType)
  const one = error.recovered.length === 1 ? error.recovered[0] : undefined
  const moved = one
    ? w('originalsKeptAside.message.one', { path: escapeHtml(one.path), keptAt: escapeHtml(one.keptAt) })
    : w('originalsKeptAside.message.many', { count: String(error.recovered.length) })
  const next = one
    ? w('originalsKeptAside.suggestion.one', { keptAt: escapeHtml(one.keptAt) })
    : w('originalsKeptAside.suggestion.many')
  return {
    title: w('originalsKeptAside.title'),
    message: `${moved} ${cause.message}`,
    suggestion: `${next} ${cause.suggestion}`,
  }
}

/**
 * The sentence for a drive that left in the middle of a transfer.
 *
 * Three facts, in the order a worried person needs them: which drive went, how
 * far the transfer got, and where their files are now. The backend names the
 * side (it captured both volumes when the transfer started, because a volume
 * that vanishes can't be looked up afterwards), so ❌ nothing here guesses one
 * from a path.
 *
 * Falls back to the volume-agnostic sentence for a backend session that dropped
 * with no typed side (MTP, SMB), and for a transfer that stopped before the
 * counts existed: a sentence reading "copied 0 of 0 files" would be worse than
 * one that doesn't count at all.
 */
function deviceDisconnectedMessage(
  error: Extract<WriteOperationError, { type: 'device_disconnected' }>,
  operationType: TransferOperationType,
  progress: ProgressAtStop | null,
): FriendlyErrorMessage {
  const title = w('deviceDisconnected.title')
  const suggestion = w('deviceDisconnected.suggestion')
  const side = error.side
  const counted = progress !== null && progress.filesTotal > 0
  // A move that stops keeps every original, so its destination sentence reports
  // no partial progress: there isn't any to report.
  const needsCounts = !(side?.role === 'destination' && operationType === 'move')
  const sidedOperation = operationType === 'copy' || operationType === 'move'
  if (side && sidedOperation && (counted || !needsCounts)) {
    return {
      title,
      message: w(`deviceDisconnected.sided.${side.role}.${operationType}`, {
        volumeName: escapeHtml(side.volumeName),
        counterpart: escapeHtml(side.counterpartName),
        done: formatInteger(progress?.filesDone ?? 0),
        total: formatInteger(progress?.filesTotal ?? 0),
      }),
      suggestion,
    }
  }
  return { title, message: w(`deviceDisconnected.message.${operationType}`), suggestion }
}

/**
 * Whether a missing Full Disk Access grant is a plausible explanation for a refusal,
 * and so whether a surface may offer that as the next step.
 *
 * Plausible, ❗ never proven: both codes also come back for an item that is genuinely
 * locked down, or on a volume that really has no Trash. So the offer is ADDITIVE, an
 * extra line under whatever the OS actually said, never a replacement for it.
 *
 * Lives here rather than beside the Rust enum because this is the only question anyone
 * asks of the value; splitting the vocabulary from the judgement keeps one rule in one
 * place. `trash-refused-messages.test.ts` walks every variant, so a new one can't
 * silently default into the offer.
 */
export function mayBeAPermissionGrantAway(reason: TrashRefusalKind): boolean {
  return reason === 'notPermitted' || reason === 'noTrashForVolume'
}

/**
 * A refused trash, worded from the reason the OS gave rather than from its sentence.
 *
 * ❗ It never says "try again" for a permission-shaped refusal. Retrying cannot change
 * one, and a real report came back describing exactly that advice, under a folded
 * "Technical details" that held the only useful part.
 */
/**
 * The sentence naming how many items the OS turned down, and why.
 *
 * Two surfaces share it: the error dialog for a batch where NOTHING went, and
 * the toast beside a batch that took some items and had to leave the rest
 * (`composeTrashRefusedToast`). One wording for one fact, so the partial ending
 * can't drift from the total one.
 */
export function trashRefusedCountSentence(reason: TrashRefusalKind, itemCount: number): string {
  return w(`trashRefused.message.${reason}`, { count: formatInteger(itemCount) })
}

function trashRefusedMessage(error: Extract<WriteOperationError, { type: 'trash_refused' }>): FriendlyErrorMessage {
  const suggestion = w(`trashRefused.suggestion.${error.reason}`)
  // ❗ `onlineOnly` overrides the reason. An evicted cloud file refuses as 513
  // (`notPermitted`) as readily as 3328 (`noTrashForVolume`), so the reason can't
  // tell the two apart, and sending someone to System Settings to grant Full Disk
  // Access when their real problem is a file that lives on Dropbox's servers is a
  // wrong answer they'd act on. The backend reads the flag at the refusal
  // (`delete/cloud_trash.rs`).
  const offerGrant = isMacOS() && fdaIsMissing() && !error.onlineOnly && mayBeAPermissionGrantAway(error.reason)
  return {
    title: w('trashRefused.title'),
    message: trashRefusedCountSentence(error.reason, error.itemCount),
    suggestion: offerGrant ? `${suggestion} ${w('trashRefused.suggestion.noFullDiskAccess')}` : suggestion,
  }
}

/**
 * The variants whose words come from the error's OWN fields and not from the operation,
 * so they need neither `operationType` nor `progressAtStop`.
 *
 * Split out of `getUserFriendlyMessage` to keep that switch inside the complexity limit,
 * and it reads better anyway: these three answer a different question from the rest.
 * `null` means "not one of mine", never "no message".
 */
function fieldDrivenMessage(error: WriteOperationError): FriendlyErrorMessage | null {
  switch (error.type) {
    case 'trash_refused':
      return trashRefusedMessage(error)
    case 'read_only_device':
      return readOnlyMessage(error)
    case 'destination_not_writable':
      return destinationNotWritableMessage(error)
    default:
      return null
  }
}

/**
 * Returns a user-friendly message for a transfer operation error.
 * Volume-agnostic: doesn't mention MTP, SMB, etc. directly.
 *
 * `progressAtStop` rides on the `write-error` event rather than on the error, so
 * a surface that only kept the error (a retained failure in the queue) passes
 * nothing and gets the sentence that needs no counts.
 */
export function getUserFriendlyMessage(
  error: WriteOperationError,
  operationType: TransferOperationType = 'copy',
  progressAtStop: ProgressAtStop | null = null,
): FriendlyErrorMessage {
  const simpleFactory = simpleMessageFactories[error.type]
  if (simpleFactory) return simpleFactory(operationType)

  const fieldDriven = fieldDrivenMessage(error)
  if (fieldDriven) return fieldDriven

  switch (error.type) {
    case 'permission_denied':
      return permissionDeniedMessage(error, operationType)
    case 'device_disconnected':
      return deviceDisconnectedMessage(error, operationType, progressAtStop)
    // The move kept every original, which is the whole message. Named when the
    // destination volume had a name to capture.
    case 'move_not_confirmed':
      return {
        title: w('moveNotConfirmed.title'),
        message: error.volumeName
          ? w('moveNotConfirmed.message.named', { volumeName: escapeHtml(error.volumeName) })
          : w('moveNotConfirmed.message.unnamed'),
        suggestion: w('moveNotConfirmed.suggestion'),
      }
    case 'insufficient_space':
      return {
        title: w('insufficientSpace.title'),
        message: w('insufficientSpace.message', {
          required: colorizeSizeString(formatByteSize(error.required)),
          available: colorizeSizeString(formatByteSize(error.available)),
        }),
        suggestion: w('insufficientSpace.suggestion'),
      }
    case 'duplicate_source_names':
      // Two selected items carry one name, so they'd both want
      // `<destination>/<name>`. Refused before anything is written, and the
      // message names the name plus one of the two folders it came from, since
      // "you picked two things called invoices" is only actionable once the user
      // can see which two.
      return {
        title: w('duplicateSourceNames.title'),
        message: w('duplicateSourceNames.message', {
          name: escapeHtml(error.name),
          first: escapeHtml(error.first),
          second: escapeHtml(error.second),
        }),
        suggestion: w(`duplicateSourceNames.suggestion.${operationType}`),
      }
    case 'invalid_name':
      // The destination refused the NAME, so it never looked the file up and a
      // retry re-sends the same impossible request. One transfer can descend a
      // whole subtree, so naming the offending file is the message's whole job:
      // `error.path` is the item the walker was on, not the top-level source.
      return {
        title: w('invalidName.title'),
        message: w('invalidName.message', { path: escapeHtml(error.path) }),
        suggestion: w('invalidName.suggestion'),
      }
    case 'new_data_kept_at':
      // The new file landed complete and the one it was replacing is already
      // gone, so `keptAt` is the ONLY copy in existence. Naming it is the whole
      // job of this message: without it the user has an intact file they can't
      // find and an empty slot where their old one was.
      return {
        title: w('newDataKeptAt.title'),
        message: w('newDataKeptAt.message', {
          path: escapeHtml(error.path),
          keptAt: escapeHtml(error.keptAt),
        }),
        suggestion: w('newDataKeptAt.suggestion', { keptAt: escapeHtml(error.keptAt) }),
      }
    case 'originals_kept_aside':
      return originalsKeptAsideMessage(error, operationType)
    case 'files_too_large_for_filesystem':
      return tooLargeForFilesystemMessage(error)
    default:
      return {
        title: w(`fallback.title.${operationType}`),
        message: w(`fallback.message.${operationType}`),
        suggestion: w('fallback.suggestion'),
      }
  }
}

/** Error types where technical details are just the path. */
const pathOnlyTypes = new Set<WriteOperationError['type']>([
  'source_not_found',
  'destination_not_found',
  'source_not_connected',
  'destination_not_connected',
  'destination_exists',
  'symlink_loop',
  'file_locked',
  'trash_not_supported',
  'connection_interrupted',
  'name_too_long',
  'delete_pending',
  'destination_not_writable',
  'destination_full',
])

/** Error types where technical details include path + error message. */
const pathAndMessageTypes = new Set<WriteOperationError['type']>([
  'read_error',
  'write_error',
  'invalid_name',
  'io_error',
])

/**
 * The technical lines for a refused write.
 *
 * The errno is what a bug report needs and the prose deliberately never states:
 * `EACCES` is a folder an administrator could write to, `EPERM` is macOS refusing
 * outright. `Refused by` is the folder the backend PROVED with `access(W_OK)`, so
 * a report says which end of a move said no. Split out to keep
 * `variantDetailLines` under the complexity ceiling.
 */
function permissionDeniedDetailLines(error: Extract<WriteOperationError, { type: 'permission_denied' }>): string[] {
  const lines = [`Path: ${error.path}`]
  if (error.refusedFolder) lines.push(`Refused by: ${error.refusedFolder}`)
  if (error.errno !== null) lines.push(`Errno: ${String(error.errno)} (${error.refusal})`)
  if (error.message) lines.push(`Details: ${error.message}`)
  return lines
}

/**
 * The technical lines for the variants the two sets above don't cover.
 *
 * Split out of `getTechnicalDetails` so that stays a three-way dispatcher: this
 * chain grows one arm per new variant, and its length is the shape of the error
 * union rather than of the function.
 */
function variantDetailLines(error: WriteOperationError): string[] {
  if (error.type === 'read_only_device') {
    return error.deviceName ? [`Path: ${error.path}`, `Device: ${error.deviceName}`] : [`Path: ${error.path}`]
  }
  if (error.type === 'permission_denied') {
    return permissionDeniedDetailLines(error)
  }
  if (error.type === 'insufficient_space') {
    const lines = [`Required: ${formatByteSize(error.required)}`, `Available: ${formatByteSize(error.available)}`]
    if (error.volumeName) lines.push(`Volume: ${error.volumeName}`)
    return lines
  }
  if (error.type === 'destination_inside_source') {
    return [`Source: ${error.source}`, `Destination: ${error.destination}`]
  }
  if (error.type === 'duplicate_source_names') {
    return [`Name: ${error.name}`, `First: ${error.first}`, `Second: ${error.second}`]
  }
  if (error.type === 'files_too_large_for_filesystem') {
    return [
      `Filesystem: ${error.filesystem}`,
      `Max file size: ${formatByteSize(error.maxSize)}`,
      `Files over the limit: ${String(error.totalCount)}`,
      ...error.files.map((file) => `  ${file.name} (${formatByteSize(file.size)})`),
    ]
  }
  // Both paths, because the whole point of this variant is that the user's new
  // file is at the second one and nowhere else.
  if (error.type === 'new_data_kept_at') {
    return [`Path: ${error.path}`, `New data kept at: ${error.keptAt}`, `Error: ${error.message}`]
  }
  // Every renamed file, because the message only names one of them, and the
  // details block is the only place a user with several can find the rest. The
  // cause's own lines follow, so a bug report still carries what stopped the copy.
  if (error.type === 'originals_kept_aside') {
    return [
      ...error.recovered.map((entry) => `Kept ${entry.path} at: ${entry.keptAt}`),
      ...getTechnicalDetails(error.cause).split('\n'),
    ]
  }
  if (error.type === 'cancelled' && error.message) {
    return [`Details: ${error.message}`]
  }
  return []
}

/**
 * The details for the two variants a vanished drive raises, or `null` for
 * everything else.
 *
 * The drive's identity is the thing worth having here: the volume list has
 * already dropped it, so this block is the only place its name and id survive a
 * bug report. A backend disconnect with no typed side (MTP, SMB) keeps the plain
 * path line it has always had. Split out so `variantDetailLines` stays within
 * its complexity ceiling.
 */
function driveDetailLines(error: WriteOperationError): string[] | null {
  if (error.type === 'device_disconnected') {
    return error.side
      ? [`Path: ${error.path}`, `Volume: ${error.side.volumeName} (${error.side.volumeId})`, `Side: ${error.side.role}`]
      : [`Path: ${error.path}`]
  }
  if (error.type === 'move_not_confirmed') {
    const lines = [`Path: ${error.path}`]
    if (error.errno !== null) lines.push(`Errno: ${String(error.errno)}`)
    if (error.volumeName) lines.push(`Volume: ${error.volumeName}`)
    return lines
  }
  return null
}

/**
 * Returns the technical details for an error (path, raw error message, etc.)
 */
export function getTechnicalDetails(error: WriteOperationError): string {
  const lines: string[] = []
  const drive = driveDetailLines(error)

  if (drive) {
    lines.push(...drive)
  } else if (pathOnlyTypes.has(error.type)) {
    lines.push(`Path: ${(error as { path: string }).path}`)
  } else if (pathAndMessageTypes.has(error.type)) {
    lines.push(`Path: ${(error as { path: string }).path}`)
    lines.push(`Error: ${(error as { message: string }).message}`)
  } else {
    lines.push(...variantDetailLines(error))
  }

  lines.push(`Error type: ${error.type}`)

  return lines.join('\n')
}
