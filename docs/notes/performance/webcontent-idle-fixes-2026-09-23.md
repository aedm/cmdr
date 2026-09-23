# Cutting the frontend's idle cost: what changed and what it bought (2026-09-23)

**What this settles:** the five fixes the WebContent diagnosis led to, the two visible-behavior follow-ups after them
(the hourglass delay and free-space precision), and what each measured. WebContent's idle cost on a pane on `~` with
indexing on fell from a median 1.50% to 0.36% of a core, and the GPU helper's from 0.51% to 0.11%, with what's left
being values that genuinely changed on screen.

The diagnosis these fixes answer: `webcontent-idle-cost-2026-09-23.md`. How each mechanism works now lives beside its
code; this note keeps the numbers and the reasoning.

## The fixes (landed 2026-09-23)

1. **Scanning tooltips mount their live body only while open** (`08e67f535`). A new `onOpenChange` on the tooltip action
   lets `DriveIndexBadge` and `IndexingStatusIndicator` mount their hidden `IndexingDriveRow` host only while the
   tooltip shows, which ends three hidden 1 Hz clocks and the unseen re-renders on every scan-progress event. Visible
   side effect: the ETA's sliding window starts empty on each open, so for about 5 s the ETA rests on the elapsed-time
   estimate alone.
2. **The Size column holds its width while the same rows are on screen** (`c6694952d`, plus `67d486826` so a directory
   diff doesn't release the hold). `holdSizeColumnWidth` (`views/measure-column-widths.ts`) grows the column at once but
   never shrinks it until the next scroll, resize, navigation, refresh, or size-setting change. `useShortenMiddle` skips
   re-measuring when width and text are unchanged, and skips the `textContent` write when the result is the same.
   Visible change: a column that would have shrunk stays wider until one of those events.
3. **Disk-space events only when the readout would change, and none while hidden** (`f607a050f`).
   `space_poller/readout.rs` models the readout at displayed resolution, and `main_window_visibility.rs` carries the
   webview's `document.visibilityState` to the backend. Hidden, the poller checks only the boot volume's low-space
   threshold and emits nothing; on show it polls everything at once.
4. **Folder-size updates reach only the listings they touch** (`00f74f84c`). The backend's `listing_index_sizes/`
   observer replaces the frontend's `index-dir-updated` handler, and `touched.rs` maps a batch to rows: a listing's own
   path means its `..` row, a path under it means the child row on the way down, and ancestors mean nothing. Only `/`
   alone or the volume id means "everything". This removed the bug that refreshed both panes on every write anywhere on
   the disk.
5. **A pane refreshes only when a shown value moved, with no IPC per update and nothing while hidden** (`548e6bea3`,
   plus `d17ce3f1f` throttling the MCP pane mirror to one push per 5 s). The backend worker compares each touched row's
   sizes, counts, and hourglass against what it last sent, paces at most one refresh per listing per 2 s with a trailing
   refresh, and holds everything while the window is hidden, sending one merged refresh on show. Comparison is on raw
   values; formatting stays in the webview.
6. **The hourglass shows only after 2 s of continuous updating** (`26cc400dd`). The rule lives in the index
   (`cmdr-index` `read/pending_sizes.rs`: `SHOW_AFTER` 2 s, `MIN_SHOWN` 1 s), so every reader of `DirStats` agrees,
   including the agent's `list_dir`. Under background cache writes, `Library` and `..` on a pane on `~` stay still.
7. **Free-space precision follows the drive's size** (`f44663c82`). The figure keeps the most fraction digits, up to
   two, whose step is still at least 1/1000 of the drive (roughly a pixel of the usage bar), so a 1 TB SSD reads "261 GB
   of 926 GB free" instead of "261.39 GB of 926.00 GB". The emit gate mirrors the rule, and both sides test against one
   table (`apps/desktop/src/lib/units/drive-figure-cases.json`). The low-space toast now formats its percent through the
   locale.

Accepted trade-offs: the MCP pane mirror lags index-driven size changes by up to 5 s, and while the window is hidden the
backend's listing-cache sizes and the pane volumes' cached free space stay as they were when it hid.

## Measurements

Method for both rounds: one isolated dev instance (one data dir, one index) launched alternately from a base worktree
and from the branch; left pane `~`, right pane `~/Downloads`, Full view, indexing on; CPU-time deltas over 180 s from
`ps -o time`. WebContent identified per launch by a busy loop, or by 20 MCP-driven pane navigations when the bridge's
`execute_js` wasn't available (exactly one WebContent gained 1.2–1.4 s). The machine was under very heavy build churn
throughout (load average up to 135, index replays of 40,000–230,000 events), which is why the medians come with ranges.

**Fixes 1–5**, base versus after, interleaved (verified on dev builds, `ps -o time` deltas, 2026-09-23):

- WebContent: median 1.50 (0.56–2.57) → **0.36** (0.22–0.51).
- GPU helper: median 0.51 (0.07–0.79) → **0.27** (0.23–0.29).
- IPC calls per 180 s: 611–765 → 126–184. The `~/Downloads` pane made zero index-driven refreshes.
- 60 s hidden: zero listing or disk-space events, WC 0.05; on show, one catch-up event within 2.5 s.
- The per-update MCP pane push alone was worth ~0.23 points, hence the 5 s throttle.

**Fixes 6–7**, against a fresh interleaved base on top of fixes 1–5:

- WebContent: median 0.58 (0.42–0.70) → **0.41** (0.24–0.56), down 29% against its own base.
- GPU helper: median 0.28 (0.25–0.34) → **0.11** (0.02–0.16), down 60%.
- `volume-space-changed` emits: 17–28 a minute → 1–8 a minute, each a real 1 GB step while free space swung by gigabytes
  under the builds.

**Why the diagnosis's targets (WC 0.2–0.3, GPU 0.01) weren't reached**: what's left is values that really change on
screen. The GPU's 0.01 was measured with disk-space events suppressed entirely; under build churn the free space itself
moves. The WebContent remainder was mostly the hourglass blink (fixed by 6) and the hidden-entry directory diffs below.

## Diffs made only of hidden entries

The watcher re-reads `~` whenever anything in it changes, dotfiles included, and a pane with hidden files off used to
refetch its rows for each such diff. Fixed, with its numbers, in `hidden-entry-diffs-2026-09-23.md`.
