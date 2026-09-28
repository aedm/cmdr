import { invoke } from '@tauri-apps/api/core'
import type { BreadcrumbEvent } from '$lib/ipc/bindings'
import type { CommandId } from '$lib/commands'

type DiagnosticBreadcrumbEvent =
  | Exclude<BreadcrumbEvent, { type: 'command' }>
  | { type: 'command'; commandId: CommandId }

/**
 * Record a triage breadcrumb. Fire-and-forget. Failures (e.g. backend not ready
 * during early startup) are silently swallowed.
 *
 * The generated `BreadcrumbEvent` union is deliberately closed: producers can send
 * only the event identities and explicitly reviewed facts the backend manifest accepts.
 * The command variant narrows its string payload further to the registry's `CommandId`.
 */
export function recordBreadcrumb(event: DiagnosticBreadcrumbEvent): void {
  // eslint-disable-next-line cmdr/no-raw-tauri-invoke -- ubiquitous best-effort instrumentation must stay independent of feature command mocks; the generated BreadcrumbEvent still pins the wire schema
  void invoke('record_breadcrumb', { event }).catch(() => {
    // Best-effort: a failing breadcrumb shouldn't break the UI flow.
  })
}
