<script lang="ts">
    /**
     * The one-time toast after Escape took the main window out of full screen
     * (`escape-key.ts`). It carries the switch itself, so someone who didn't want
     * that can turn it off right here, plus a link to where it lives in Settings.
     */

    import type { Snippet } from 'svelte'
    import { dismissToast } from '$lib/ui/toast'
    import LinkButton from '$lib/ui/LinkButton.svelte'
    import Switch from '$lib/ui/Switch.svelte'
    import Trans from '$lib/intl/Trans.svelte'
    import { tString } from '$lib/intl/messages.svelte'
    import { useBooleanSetting } from '$lib/settings/components/boolean-setting.svelte'
    import { openSettingsWindow, settingAnchorId } from '$lib/settings/settings-window'

    import { ESCAPE_FULL_SCREEN_TOAST_ID } from './escape-full-screen-toast-id'

    const setting = useBooleanSetting('advanced.exitFullScreenOnEscape')

    function handleOpenSettings() {
        dismissToast(ESCAPE_FULL_SCREEN_TOAST_ID)
        void openSettingsWindow(
            'escape-full-screen-toast',
            ['Advanced'],
            settingAnchorId('advanced.exitFullScreenOnEscape'),
        )
    }
</script>

{#snippet settingsLink(children: Snippet)}<LinkButton onclick={handleOpenSettings}>{@render children()}</LinkButton>{/snippet}

<div class="content">
    <p>{tString('main.escapeFullScreenHint.message')}</p>
    <div class="switch-row">
        <Switch checked={setting.checked} onCheckedChange={(next: boolean) => { setting.set(next) }}>
            {tString('main.escapeFullScreenHint.switchLabel')}
        </Switch>
    </div>
    <p class="settings-line">
        <Trans key="main.escapeFullScreenHint.settingsLine" snippets={{ settingsLink }} />
    </p>
    <p class="once-note">{tString('main.escapeFullScreenHint.onceNote')}</p>
</div>

<style>
    .content {
        font-size: var(--font-size-sm);
        color: var(--color-text-primary);
        max-width: 24rem;
    }

    .switch-row {
        margin-top: var(--spacing-sm);
    }

    .settings-line {
        margin-top: var(--spacing-sm);
        color: var(--color-text-secondary);
    }

    .once-note {
        margin-top: var(--spacing-sm);
        font-size: var(--font-size-xs);
        color: var(--color-text-tertiary);
    }
</style>
