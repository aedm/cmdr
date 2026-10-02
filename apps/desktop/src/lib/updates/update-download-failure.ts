/**
 * A failed update download, carried across the throw with its typed reason so the updater can pick the log level
 * (`downloadInstallLogLevel` in `updater.svelte.ts`). Lives outside `$lib/tauri-commands` so the updater's tests,
 * which mock that barrel, still get the real class.
 */
import type { UpdateDownloadError } from '$lib/ipc/bindings'
import { serverRequestDiagnostic } from '$lib/error-messages/server-request'
import { TypedFailure } from '$lib/ipc/typed-failure'

/** An `Error` that still carries the backend's typed download failure, so the updater can pick its log level. */
export class UpdateDownloadFailure extends TypedFailure<UpdateDownloadError> {
  constructor(failure: UpdateDownloadError) {
    super(
      failure,
      failure.type === 'request'
        ? `update download: ${serverRequestDiagnostic(failure.failure)}`
        : `update download ${failure.type}: ${failure.detail}`,
    )
    this.name = 'UpdateDownloadFailure'
  }
}
