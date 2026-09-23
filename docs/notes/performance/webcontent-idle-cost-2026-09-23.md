# What the frontend (WebContent) costs at idle, and why (2026-09-23)

**What this settles:** where an idle Cmdr's WebContent and GPU helper processes spend their CPU, measured on a dev and a
prod instance. The cost was backend-driven listing refreshes and the DOM churn they set off, not timers or animations,
and window state barely changed it. It also carries the frontend memory picture and what WebKit won't tell you.

The fixes it led to, with before/after numbers: `webcontent-idle-fixes-2026-09-23.md`.

## Method

- **Instances**: an isolated dev instance (`pnpm dev --worktree <slug>`, own data dir), a freshly relaunched prod build
  (read-only), and a two-day-old release build from another worktree as a long-uptime memory reference.
- **Which process is WebContent**: run a 4 s busy loop in the webview (Tauri MCP bridge) and see which WebContent pid
  gains ~4 s of CPU. For prod, `log show --predicate 'processID == <pid>'` names the WebContent it spawned, and the GPU
  helper is the one spawned in the same second.
- **CPU**: CPU-time deltas from `ps -o time` over 120–180 s windows, as % of one core. Window state from CGWindowList
  plus a covering-windows check, and `document.visibilityState` logged in the page.
- **JS attribution**: monkeypatched `setTimeout`, `setInterval`, `requestAnimationFrame`, and `requestIdleCallback`
  (call site from the stack), a document-wide `MutationObserver`, every Tauri event callback, and `window.fetch`. Tauri
  2 IPC goes through `fetch` to the IPC protocol, so patching `fetch` names every command; `__TAURI_INTERNALS__.invoke`
  is frozen.
- **Blind spot**: intervals created at module load, before the patch. Covered by reading the code and by their IPC
  footprint.
- **Dev versus prod**: no dev-only timer showed at idle, and under the same machine churn prod's WebContent read 1.41%
  and dev's 1.40% in the same three-minute window. Dev overhead doesn't distort the picture.

## Idle CPU by window state (before the fixes)

WebContent (WC) and GPU helper, % of one core:

- Visible, quiet folder, indexing off, disk-space threshold raised to 1 GB (the floor): WC 0.18, GPU 0.01.
- Visible, quiet folder, indexing off, default settings: WC 0.25, GPU 0.25.
- Visible, `~` and `~/Downloads`, indexing off: WC 0.21, GPU 0.17.
- Visible, `~` and `~/Downloads`, indexing on, FS churn from builds: **WC 1.40, GPU 0.24**.
- Hidden (fully covered), same panes, indexing on, churn: WC 1.43 (0.96 in a calmer window).
- Hidden, quiet folder, indexing on: WC 0.30, GPU 0.08.
- Minimized, `~` and `~/Downloads`, indexing off: WC 0.55, noisy (three downloads landed in the window).
- Prod WebContent, fresh, fully covered, indexing active: 0.87, 2.06, and 1.41 over three windows.
- A two-day-old release WebContent, on screen in the background: 0.38.

**Window state barely matters.** A hidden or minimized page stops `requestAnimationFrame`, painting, and GPU work, and
throttles timers, but the dominant cost is backend events turning into DOM, style, and IPC work, which keeps running
while hidden. Hidden-state CSS transitions even pile up as pending (64 `grid-template-columns` transitions stuck at
`startTime: null`).

## Attribution, ranked (a pane on `~`, indexing on, WC ≈ 1.4%)

1. **Index-driven listing refresh cascade: ~80% of WC.**
   - `index-dir-updated` events (118 in 180 s) matched ANY change under the pane's folder, so a pane on `~` refreshed on
     every write under `~/Library`. Worse, every live batch carries its ancestor chain up to `/`, and the handler read
     `/` as "refresh everything", so both panes refreshed on every write anywhere on the disk.
   - Each refresh fired about six IPC calls whether or not a shown value changed (`get_dir_stats_batch`,
     `refresh_listing_index_sizes`, `get_file_range`, `get_listing_stats`, the MCP pane-state push, and
     `update_services_selection`).
   - DOM cost dominated, not JS (handler JS was ~70 ms per 180 s): the Size column's shrink-wrap changed width (the
     hourglass slot coming and going), which started a 300 ms `grid-template-columns` transition on every row; each
     frame resized every name cell, and the `useShortenMiddle` `ResizeObserver` rewrote `textContent` unconditionally.
     2,673 name-cell rewrites in 180 s.
2. **Disk-space events: ~5% of WC, but most of the GPU helper's idle cost.** The poller ran every 2 s with a 1 MB
   threshold, emitting 0.6–0.9 events a second; each re-rendered both status bars, and about 60% changed no visible
   text. Raising the threshold to 1 GB took WC 0.25 → 0.18 and GPU 0.25 → 0.01.
3. **The deleted-folder poll (`path_exists`): ~2%.** Two panes at 0.5 Hz, about 0.03 points per call a second. Cheap,
   left alone.
4. **Timers: under 1%.** The log bridge's 1 Hz reset, the sync-status poll's early return, and, only while scanning, the
   scan clock ticking 1 Hz per drive row including rows inside hidden tooltip hosts, whose bodies also re-rendered on
   every scan-progress event.
5. **CSS animations: ~0%.** The scanning pulses are opacity animations and run composited; injecting three extra into a
   floor page moved nothing. No `requestAnimationFrame` loop or `requestIdleCallback` is live at idle.
6. **WebKit's own housekeeping**: the rest of the ~0.18% floor, not attributable from JS.

## Memory

- **Footprints** (`footprint`): fresh prod WebContent 138 MB (peak 220; WebKit Malloc 91, graphics 21, Untagged 12, JIT
  4). The two-day release: **262 MB** (WebKit Malloc 130, Untagged **88**, graphics 25), its Untagged regions (4,656)
  almost all swapped or compressed, so cold retained memory. Dev: 276 MB. GPU helpers: 20–22 MB, peaks over 300 MB.
- **What WebKit doesn't expose**: no JS heap size (`performance.memory` is Chromium-only), no heap snapshot through the
  bridge, and libpas mixes JS objects, DOM, and decoded images inside "WebKit Malloc". Any JS-versus-DOM split is an
  estimate.
- **Measured in the page**: about 1,140 DOM elements; 65 icon `<img>`s, all ~1.2 KB WebP data URLs (47 KB unique); a 52
  KB icon cache in `localStorage`. Both negligible.
- **No navigation leak**: 100 navigations across big folders swung WebKit Malloc 193 → 473 → 317 MB, GC-shaped rather
  than monotonic. What the two-day process's extra ~120 MB holds is unknown (retained state or JSC fragmentation); a Web
  Inspector heap snapshot on a long-running build would settle it.
- **Largest known retained structure: every translation catalog, loaded eagerly.** `intl/messages.svelte.ts` uses an
  eager `import.meta.glob` over all 13 locales: 40,800 strings (~2.0 M chars) plus ~2.3 M chars of `@key` metadata, kept
  referenced after `stripMetadata`. That's 4.07 MB of the 6 MB of prod JS, an estimated 10–25 MB of heap, of which a
  session needs only the active locale and `en`.
- **Rough split of the ~140 MB fresh footprint**: JS heap (framework, app, catalogs) 50–70 MB, graphics backing stores
  20–25, DOM, CSS, and fonts 10–15, WebKit's per-process baseline 30–40.
