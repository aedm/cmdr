<script lang="ts">
    /**
     * The 5-tier rainbow dot for a phone's negotiated USB generation, in both placements:
     * the breadcrumb chip and each switcher row. Same shape as the connection dot; dark
     * green is `--color-allow`, the shade a healthy direct SMB session wears.
     *
     * The dot is the only visual: no inline text in the chip, and no extra line under the
     * disk-space bar, by design.
     */
    import { tooltip } from '$lib/tooltip/tooltip'
    import { tString } from '$lib/intl/messages.svelte'
    import { describeUsbSpeed, type UsbSpeed } from '../types'

    interface Props {
        speed: UsbSpeed
        /** Chip placement: a small left margin so it doesn't jam against the label. */
        breadcrumb?: boolean
    }

    const { speed, breadcrumb = false }: Props = $props()

    const described = $derived(describeUsbSpeed(speed))

    /** "USB 3.2 Gen 1 (Max. 625 MB/s)", then the line saying what that number is about. */
    const hint = $derived.by(() => {
        const { label, maxMBps } = described
        const mbps = maxMBps >= 10 ? String(Math.round(maxMBps)) : maxMBps.toFixed(1)
        // The global tooltip CSS is `white-space: pre-line`, so `\n` becomes a real break.
        return `${tString('fileExplorer.navigation.usbSpeed', { label, mbps })}\n${tString('fileExplorer.navigation.usbSpeedNegotiated')}`
    })
</script>

<span
    class="usb-speed-indicator usb-speed-indicator-{described.tier}"
    class:breadcrumb-usb-speed-indicator={breadcrumb}
    use:tooltip={hint}
></span>

<style>
    .usb-speed-indicator {
        width: 10px;
        height: 10px;
        border-radius: 50%;
        flex-shrink: 0;
        opacity: 0.8;
    }

    /*noinspection CssUnusedSymbol*/
    .usb-speed-indicator-low {
        background-color: var(--color-apple-red);
    }

    /*noinspection CssUnusedSymbol*/
    .usb-speed-indicator-full {
        background-color: var(--color-apple-orange);
    }

    /*noinspection CssUnusedSymbol*/
    .usb-speed-indicator-high {
        background-color: var(--color-apple-yellow);
    }

    /*noinspection CssUnusedSymbol*/
    .usb-speed-indicator-super {
        background-color: var(--color-apple-green);
    }

    /*noinspection CssUnusedSymbol*/
    .usb-speed-indicator-super_plus {
        background-color: var(--color-allow);
    }

    /*noinspection CssUnusedSymbol*/
    .breadcrumb-usb-speed-indicator {
        margin-left: var(--spacing-xs);
    }
</style>
