/**
 * The "Managed by your organization" card's lines: what the organization restricts, one line per
 * area, in words. A help-desk person reads it to check that Cmdr honors the profile.
 *
 * Pure wording over the backend's `ManagedPolicyView`: every fact comes from the view as the
 * backend decided it, ❌ never from a key name or a setting value.
 */

import type { ManagedPolicyView } from '$lib/ipc/bindings'
import { tString } from '$lib/intl/messages.svelte'
import { formatConjunctionList } from '$lib/intl/list-format'

export interface ManagedSummaryLine {
  label: string
  value: string
}

/** The restricted areas, in a fixed order; an area the policy leaves alone has no line. */
export function managedPolicySummary(view: ManagedPolicyView): ManagedSummaryLine[] {
  const off = tString('settings.managed.summary.off')
  const lines: ManagedSummaryLine[] = []
  if (view.usageStatsDisabled) lines.push({ label: tString('settings.managed.summary.usageStats'), value: off })
  if (view.reportsDisabled) lines.push({ label: tString('settings.managed.summary.reports'), value: off })
  const updates = describeUpdates(view.updates, off)
  if (updates !== null) lines.push({ label: tString('settings.managed.summary.updates'), value: updates })
  const ai = describeAi(view.ai, off)
  if (ai !== null) lines.push({ label: tString('settings.managed.summary.ai'), value: ai })
  return lines
}

function describeUpdates(updates: ManagedPolicyView['updates'], off: string): string | null {
  if (updates.kind === 'disabled') return off
  const { automaticChecks, ceiling } = updates
  if (ceiling !== null) {
    return automaticChecks
      ? tString('settings.managed.summary.upTo', { ceiling })
      : tString('settings.managed.summary.upToManualChecksOnly', { ceiling })
  }
  return automaticChecks ? null : tString('settings.managed.summary.manualChecksOnly')
}

function describeAi(ai: ManagedPolicyView['ai'], off: string): string | null {
  switch (ai.mode) {
    case 'off':
      return off
    case 'localOnly':
      return tString('settings.managed.summary.onDeviceOnly')
    case 'allowed':
      return ai.allowedCloudHosts === null
        ? null
        : tString('settings.managed.summary.cloudHostsOnly', { hosts: formatConjunctionList(ai.allowedCloudHosts) })
  }
}
