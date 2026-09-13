<script lang="ts">
    import { dismissToast } from '$lib/ui/toast'
    import Button from '$lib/ui/Button.svelte'
    import { openPrivacySettings } from '$lib/tauri-commands'
    import { getAppLogger } from '$lib/logging/logger'
    import { tString } from '$lib/intl/messages.svelte'
    import { LATEST_DOWNLOAD_FDA_TOAST_ID } from './go-to-latest-ids'

    const log = getAppLogger('downloads')

    async function handleOpenSystemSettings() {
        try {
            await openPrivacySettings()
        } catch (error) {
            // The toast stays up, so the button is still there to try again.
            log.warn("Couldn't open System Settings from the latest-download toast: {error}", { error: String(error) })
            return
        }
        dismissToast(LATEST_DOWNLOAD_FDA_TOAST_ID)
    }

    function handleDismiss() {
        dismissToast(LATEST_DOWNLOAD_FDA_TOAST_ID)
    }
</script>

<div class="content">
    <span class="message">{tString('downloads.fda.message')}</span>
    <div class="actions">
        <Button size="mini" variant="secondary" onclick={handleDismiss}>{tString('downloads.fda.dismiss')}</Button>
        <Button size="mini" variant="primary" onclick={handleOpenSystemSettings}
            >{tString('downloads.fda.openSystemSettings')}</Button
        >
    </div>
</div>

<style>
    .content {
        font-size: var(--font-size-sm);
    }

    .message {
        color: var(--color-text-primary);
    }

    .actions {
        display: flex;
        justify-content: flex-end;
        gap: var(--spacing-sm);
        margin-top: var(--spacing-lg);
    }
</style>
