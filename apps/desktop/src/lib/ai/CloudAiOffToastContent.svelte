<script lang="ts">
    /**
     * The "Cloud AI is off" toast an AI search or selection raises when the AI mode is Cloud and
     * "Allow cloud AI" isn't on (the backend refused before sending anything). It carries the
     * way out: a button to the switch in Settings > AI > Provider. ❌ It never grants consent
     * itself; only the switch does (`AiCloudConsentToggle.svelte`).
     */
    import Button from '$lib/ui/Button.svelte'
    import { tString } from '$lib/intl/messages.svelte'
    import { openCloudConsentSettings } from './cloud-consent.svelte'
</script>

<div class="content">
    <strong class="title">{tString('ai.translateError.noCloudConsent.title')}</strong>
    <span class="body">{tString('ai.translateError.noCloudConsent.body')}</span>
    <div class="actions">
        <Button size="mini" variant="secondary" onclick={() => { openCloudConsentSettings('ai-translate-toast'); }}>
            {tString('queryUi.ai.openSettings')}
        </Button>
    </div>
</div>

<style>
    .content {
        font-size: var(--font-size-sm);
    }

    /* Its own line, while the root stays block flow so the body wraps around the toast's corner. */
    .title {
        display: block;
        color: var(--color-text-primary);
        font-weight: 600;
    }

    .body {
        color: var(--color-text-primary);
    }

    .actions {
        display: flex;
        justify-content: flex-end;
        margin-top: calc(var(--spacing-xs) + var(--spacing-xxs));
    }
</style>
