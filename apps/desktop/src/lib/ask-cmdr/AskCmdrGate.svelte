<!--
  The rail's closed state, in place of the chat. Two kinds (`ask-cmdr-gate.svelte.ts` decides
  which): `off` when Ask Cmdr's switch is off, with a button that turns it on; `cloudOff` when
  the AI mode is Cloud and "Allow cloud AI" isn't on, with a button that opens that switch in
  Settings. ❌ Neither grants cloud consent: only the switch itself does
  (`$lib/ai/AiCloudConsentToggle.svelte`).

  Turning Ask Cmdr on here writes the same `askCmdr.enabled` setting as Settings > AI > Ask Cmdr;
  the rail turns into the chat by itself once the setting reads on, and loads the last thread.
-->
<script lang="ts">
    import Button from '$lib/ui/Button.svelte'
    import Icon from '$lib/ui/Icon.svelte'
    import { tString } from '$lib/intl/messages.svelte'
    import { setSetting } from '$lib/settings'
    import { openCloudConsentSettings } from '$lib/ai/cloud-consent.svelte'

    interface Props {
        kind: 'off' | 'cloudOff'
    }

    const { kind }: Props = $props()
</script>

<div class="ask-cmdr-gate" data-gate={kind} role="group" aria-labelledby="ask-cmdr-gate-title">
    <span class="gate-glyph"><Icon name="sparkles" size={28} aria-hidden="true" /></span>
    {#if kind === 'off'}
        <h2 id="ask-cmdr-gate-title" class="gate-title">{tString('askCmdr.gate.off.title')}</h2>
        <p class="gate-body">{tString('askCmdr.gate.off.body')}</p>
        <!-- Focused on mount: the rail mounts this only when there's no composer to focus. -->
        <Button variant="primary" autoFocus onclick={() => { setSetting('askCmdr.enabled', true); }}>
            {tString('askCmdr.gate.off.turnOn')}
        </Button>
    {:else}
        <h2 id="ask-cmdr-gate-title" class="gate-title">{tString('askCmdr.gate.cloudOff.title')}</h2>
        <p class="gate-body">{tString('askCmdr.gate.cloudOff.body')}</p>
        <Button variant="primary" autoFocus onclick={() => { openCloudConsentSettings('ask-cmdr-cloud-gate'); }}>
            {tString('askCmdr.gate.cloudOff.openSettings')}
        </Button>
    {/if}
</div>

<style>
    .ask-cmdr-gate {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        flex: 1;
        min-height: 0;
        overflow-y: auto;
        padding: var(--spacing-md);
    }

    .gate-glyph {
        display: block;
        color: var(--color-accent-text);
    }

    .gate-title {
        margin: var(--spacing-sm) 0 var(--spacing-xs);
        font-size: var(--font-size-lg);
        font-weight: 600;
        color: var(--color-text-primary);
    }

    .gate-body {
        margin: 0 0 var(--spacing-md);
        font-size: var(--font-size-sm);
        line-height: var(--font-line-height-prose);
        color: var(--color-text-secondary);
    }
</style>
