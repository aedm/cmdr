/**
 * Keeps a pane consistent after the hidden-files toggle changes how many rows
 * its listing has: republish the total, then put the cursor somewhere sensible.
 *
 * "Somewhere sensible" means the file the user was looking at, wherever it moved
 * to once the hidden entries appeared or vanished. Only when that file is gone
 * (it WAS the hidden one) do we fall back to clamping the cursor into range.
 *
 * The backend hears the setting first: it numbers `directory-diff` rows in the
 * pane's row space and skips changes to rows the pane doesn't show.
 *
 * ## A resync can be overtaken, and that isn't an error
 *
 * The pane runs this whenever a listing lands, as well as on a toggle, and it
 * spans three IPC round trips. Three things can overtake it meanwhile:
 * - the pane moves to another listing (a navigation ends the old one in the same
 *   tick it clears the pane's id, `listing-loader.ts`),
 * - a newer resync starts on the same listing,
 * - the pane is destroyed (`dispose()`, from FilePane's teardown). Its state
 *   still reports the old listing id, but the teardown ends that listing. A tab
 *   switch or close does this, since `DualPaneExplorer.svelte` keys the pane on
 *   the active tab, and so does a dev hot reload.
 *
 * Either way this run's answers describe rows the pane no longer shows, so it
 * stops at the next await and writes nothing. A read that REJECTS once overtaken
 * is the expected "Listing not found" for the listing just ended; the caller is a
 * fire-and-forget `void`, so it would otherwise escape the window as an unhandled
 * rejection (seen on an MCP `select_volume` followed at once by `nav_to_path`).
 *
 * Overtaken is decided from the pane's state, ❌ never from the rejection's
 * message. A read that fails on the listing a live pane is still showing keeps
 * rejecting: that one is a real fault.
 */

import { findFileIndex, getTotalCount, setListingIncludeHidden } from '$lib/tauri-commands'

export interface HiddenFilesResyncInput {
  listingId: string
  includeHidden: boolean
  /** The file under the cursor before the toggle, if any. */
  nameToFollow: string | undefined
  /** The cursor position before the toggle. */
  cursorIndex: number
  /** Read late: the `..` row's presence can change with the new total. */
  getHasParent: () => boolean
  setTotalCount: (count: number) => void
  setCursorIndex: (index: number) => Promise<void>
}

/**
 * One pane's resync. `getPaneListingId` reads the listing the pane shows NOW,
 * which is how a run learns the pane moved on; it's only ever called after an
 * await, so an effect that starts a run doesn't come to depend on it. Call
 * `dispose()` from the pane's teardown: every run in flight, and any later one,
 * then counts as overtaken.
 */
export function createHiddenFilesResync(getPaneListingId: () => string) {
  let latestRun = 0
  let isDisposed = false

  async function resync(input: HiddenFilesResyncInput): Promise<void> {
    const run = ++latestRun
    const isOvertaken = (): boolean => isDisposed || run !== latestRun || getPaneListingId() !== input.listingId
    if (isDisposed) return

    try {
      await setListingIncludeHidden(input.listingId, input.includeHidden)
      if (isOvertaken()) return
      const count = await getTotalCount(input.listingId, input.includeHidden)
      if (isOvertaken()) return
      input.setTotalCount(count)

      const hasParent = input.getHasParent()
      const total = hasParent ? count + 1 : count

      // Try to keep cursor on the same file
      if (input.nameToFollow) {
        const foundIndex = await findFileIndex(input.listingId, input.nameToFollow, input.includeHidden)
        if (isOvertaken()) return
        if (foundIndex !== null) {
          await input.setCursorIndex(hasParent ? foundIndex + 1 : foundIndex)
          return
        }
      }

      // File not found (was hidden) or no file: clamp cursor
      if (input.cursorIndex >= total) {
        await input.setCursorIndex(Math.max(0, total - 1))
      }
    } catch (error) {
      if (isOvertaken()) return
      throw error
    }
  }

  function dispose(): void {
    isDisposed = true
  }

  return { resync, dispose }
}
