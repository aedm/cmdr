/**
 * What's left of the switcher's hand-rolled machinery: the breadcrumb chip's own inline
 * popup (the one behind the yellow `os_mount` dot). The dropdown's keyboard mode, submenu,
 * and key handling belong to the house `Menu` primitive (`$lib/ui/menu-controller.svelte.ts`),
 * and the chooser reaches them through `VolumeChooserMenu.svelte`.
 */

/** Breadcrumb inline popup state (yellow os_mount indicator). */
export function createBreadcrumbPopupController() {
  let open = $state(false)

  return {
    get isOpen() {
      return open
    },
    toggle() {
      open = !open
    },
    close() {
      open = false
    },
  }
}
