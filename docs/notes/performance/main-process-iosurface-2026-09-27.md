# The `IOSurface` in the main process isn't Cmdr's memory (2026-09-27)

**What this settles:** what owns the ~55 MiB of `IOSurface` (VM tag 88) that `memory_diagnostics` reports in the main
Rust process (`idle-census-2026-09-27.md`, lever 4), and whether it's reducible.

- **It isn't in the main process's footprint at all.** The regions are WebKit's layer backing stores. WebKit's GPU
  process creates them, WebContent owns them and pays for them, and the app process maps them so WindowServer can show
  them. The main process's `phys_footprint` stays flat while tag 88 swings from 0 to 216 MiB.
- **There's nothing to cut in the main process.** The idle footprints in the census (194–273 MiB) never included it.
- **The real cost lives in WebContent** and scales with the window's area: 21 MiB of owned graphics memory at 800 × 500
  pt, 49 MiB at 2,220 × 1,380 pt, on a 2× display. A bare `WKWebView` at the same size owns 120 MiB, so Cmdr's frontend
  is already lean here.
- **The fix is to the instrument**: `memory_diagnostics` now labels tag 88 and its docs say its dirty bytes aren't the
  process's cost. `docs/tooling/memory-debugging.md` § "The second trap" holds the rule.

## Setup

- **Binary**: a 0.47.0 release build (aarch64) a sibling agent made from 2026-09-27's `main`, run bare with its own
  `CMDR_INSTANCE_ID`, `CMDR_DATA_DIR`, `CMDR_MCP_PORT`, and `CMDR_SECRET_STORE=file`. `lsof` showed one listener on
  127.0.0.1 and no prod files.
- **Data**: a fresh data dir with indexing, media indexing, network (discovery included), direct SMB, update checks, and
  Ask Cmdr off. Two panes on `~` and `~/Downloads`.
- **Machine**: M3 Max, macOS 27.0. Both displays are 2× (a Dell at 3,360 × 1,890 pt and the built-in Retina panel), so a
  1× display wasn't measured.
- **Instruments**: `memory_diagnostics` (in-process VM walk), `footprint <pid>` (the kernel's per-owner accounting),
  `vmmap <pid>` (region detail), and window control through System Events. Prod was read with `vmmap` and `footprint`
  only.

## What the regions are

`vmmap` names every sizeable tag-88 region in the Cmdr process, prod and the isolated instance alike:

```
IOSurface  12d2e8000-12d6f4000 [ 4144K 0K 0K 0K] rw-/rw- SM=SHM PURGE=N ...&BGA) 4128K 'WebKit LayerBacking', shared with com.apple.WebKit.GPU[9483] WindowServer[626]
```

At 2,230 × 1,380 pt the isolated instance mapped 47 `WebKit LayerBacking` regions and 14 read-only 16 KiB surface
headers, and nothing else. None comes from Rust-side drawing (icons, QuickLook, drag images): every sizeable region is
WebKit's, and prod's `CG Image` and `CoreAnimation` rows total under 4 MiB.

## The main process doesn't pay for them

Main process footprint against tag 88 as `memory_diagnostics` reads it (MiB), with `footprint`'s view of the same
regions and of WebContent's owned graphics memory (dirty, then reclaimable), verified on the 0.47.0 release build,
2026-09-27:

- 1,080 × 720 pt, visible: footprint 149; tag 88 34; `footprint` says `IOSurface` 0 dirty. WebContent 124 (graphics 23 +
  45 reclaimable), GPU helper 18.
- 2,220 × 1,380, frontmost: 149; 108; 0. WebContent 151 (49 + 99).
- Same, 30 s later: 150; **0**; 0. WebContent 156 (49 + 1).
- 800 × 500: 150; 31; 0. WebContent 140 (21 + 32).
- Hidden, after 30 s and 90 s: 150; 31; 0. WebContent 140–141 (21 + 32). Hiding frees nothing.
- 2,220 × 1,380 again, after 30 s and 90 s: 150; **216** (68 regions); 0. WebContent 161–176 (49 + 317).
- Minimized: 151; 143; 0. WebContent **209** (97 + 48): minimizing doubles it until the window comes back.
- Restored: 151; 192; 0. WebContent 161 (49 + 144).

The main process's footprint moved by 2 MiB across the run while tag 88 ranged over 216. `footprint`'s totals match
`physFootprintBytes` to the MiB and put `IOSurface` at 0 dirty in every reading. Prod (0.47.0, 2 h up) agrees: 34
`WebKit LayerBacking` regions, 0 dirty, 41 MiB reclaimable, footprint 295 MiB.

## Why the in-process walk sees dirty pages

The kernel reports every page of a mapped shared object as resident and dirty in each process that maps it. A bare
`WKWebView` host (a 100-line Swift app, one 2,230 × 1,380 pt window, two scrolling 400-row panes) shows it cleanly,
verified on macOS 27.0 with `mach_vm_region_recurse`, `mach_vm_page_range_query`, and `footprint`, 2026-09-27:

- Its `phys_footprint` is **28.5 MiB**, while its tag 88 reads **120.2 MiB** resident and dirty by both `pages_dirtied`
  and the per-page query. The surfaces can't be inside a footprint a quarter their size.
- All 41 regions are `SM_TRUESHARED` with no external pager. `footprint` puts them at 0.
- Its WebContent owns **120 MiB** of graphics memory and its GPU helper 4.7 MiB: the bytes the window's layers cost.

The share mode can't tell whose surface it is: an `IOSurface` a test program creates for itself is `SM_TRUESHARED` too,
and that one does raise its creator's footprint (by 4.8 MiB for a 4 MiB surface). Ownership isn't in the VM region info,
which is why the instrument got a label rather than a filter.

## Is it reducible?

Not in the main process: it holds nothing there. In WebContent, it's the cost of showing the window's pixels.

- **Cmdr is already under the baseline.** 49 MiB of owned graphics memory at 2,220 × 1,380 pt, against 120 MiB for a
  bare `WKWebView` with a trivially simple page. The earlier compositor work (no permanent `will-change` layer, no
  per-row containment; `high-memory-gpu-compositor-investigation-2026-07.md`) is the likely reason.
- **It scales with the window's area** (21 MiB at 800 × 500), and WebKit marks what it isn't showing as volatile, so
  memory pressure reclaims that part without a purge from us.
- **Hiding the window frees nothing, and minimizing briefly doubles it.** Cutting it while hidden would need WebKit to
  drop its layer tree, which it controls. A lever from our side would be fewer composited layers in the frontend, and at
  under half the bare-webview cost there's little to gain.
- **On a 1× display** the backing stores should be about a quarter the size, since they're sized in device pixels
  (inferred, not measured).

## Reproducing it

- Main process versus `footprint`: `footprint <pid> | grep -E 'Footprint:|IOSurface'`, beside `memory_diagnostics`'s
  tag 88.
- What the window costs: `footprint <WebContent pid>`'s `Owned physical footprint (unmapped) (graphics)` row. WebContent
  and the GPU helper are the `com.apple.WebKit.*` processes started in the same second as the app.
- Region names: `vmmap <pid> | grep '^IOSurface'`.
