# Hidden-entry diffs: a pane stops hearing about rows it doesn't show

A pane with hidden files off refetched its visible rows on every dotfile write in `~`. The fix makes `directory-diff`
speak the pane's rows and skip changes to rows it doesn't show. The mechanism and its rules live in
`apps/desktop/src-tauri/src/file_system/listing/DETAILS.md` § "Diffs speak the pane's rows"; this note holds the
measurements.

## Method

- One isolated dev instance (`pnpm dev --worktree hidden-diffs`, one data dir, one index, fresh), launched alternately
  from a base worktree at `main` (`0e7672aca`) and from the branch. Debug builds, so read the numbers as a ratio.
- Left pane `~`, right pane `~/Downloads`, Full view, indexing on, hidden files off. The window was visible in every
  window (`document.visibilityState` read at both ends).
- WebContent identified per launch with a 3 s busy loop over the Tauri bridge: exactly one pid gained ~3.05 s each time,
  no other gained over 0.09 s.
- CPU: `ps -o time` deltas, as % of one core. Counts: in-page, `window.fetch` wrapped to count every IPC command by name
  (Tauri 2 IPC goes through it), and a `directory-diff` listener counting events and change names.
- Load average 2–30 over the session. It fell steadily, so the late windows are quieter than the early ones for both
  builds.

## Natural churn, 180 s windows

The machine was quiet compared to the earlier idle-followups session: base saw 2–6 diffs per window, not the ~18 or more
seen then. Every change named was `.claude.json`, a hidden entry.

- Base (B1 18:07, B2 18:10, B3 18:20, B4 18:33):
  - `directory-diff`: 6 / 2 / 2 / 2, each one change
  - IPC calls: 343 / 279 / 270 / 231, median 275
  - WebContent: 0.400 / 0.417 / 0.539 / 0.489, median 0.45% (0.40–0.54)
- After (A1 17:58, A2 18:02, A3 18:15, A4 18:24):
  - `directory-diff`: 0 / 0 / 0 / 0
  - IPC calls: 343 / 898 / 287 / 256, median 315. A2 includes 238 index-status polls from an unrelated indexing episode.
  - WebContent: 0.456 / 0.750 / 0.550 / 0.522, median 0.54% (0.46–0.75)
- At two to six hidden diffs per three minutes, the saving is below this machine's noise: the after median is higher,
  driven by the busier early windows. The diff count is the clean signal: all gone.

## Controlled churn, 120 s windows

To measure the cost the fix removes, the left pane showed a scratch folder (five visible files) while a loop rewrote a
dotfile in it once a second. That's `~`'s pattern at a known rate, without writing to `~` or touching the prod app.

- Base (CB1, CB2): 118 / 81 `directory-diff`; 815 / 610 IPC calls; WebContent 1.058 / 0.575%
- After (CA1, CA2): 0 / 0 `directory-diff`; 120 / 84 IPC calls (all but six are the 1 Hz `path_exists` poll); WebContent
  0.300 / 0.200%
- Each hidden-only diff cost the webview six IPC calls: `get_total_count`, `get_file_range`, `get_listing_stats`,
  `enrich_tags`, `get_sync_status`, and `update_services_selection`, plus a re-render. At one per second that's about
  0.3–0.8 points of a core in WebContent.
