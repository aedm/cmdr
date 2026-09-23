# Viewer rows: follow-ups

The F3 viewer serves bounded ROWS (a row ends at a newline or after `SEGMENT_BYTES`, 20 000), so a file with no newline
in it costs a bounded read per fetch instead of the whole file (`ERR-RQ8BY`, a 300 MB single-line JSON). The rule, the
wire shape, and invariants I1–I6 live in `apps/desktop/src-tauri/src/file_viewer/DETAILS.md` § "Rows, not lines"; the
frontend side (gutter, continuation marker, fetch walk, copy sizing) in `apps/desktop/src/routes/viewer/DETAILS.md` §
"Rows, not lines".

## 1. Rename the viewer's IPC coordinate from "line" to "row"

- **Problem**: the viewer counts rows everywhere, but the wire still spells them as lines: `SeekTarget::Line` and
  `SeekTargetKind::Line` (`file_viewer/mod.rs`), `RangeEnd::Line { line, offset }` (`file_viewer/range_read.rs`), and
  `SearchMatch.line`. On a minified file a row and a line are different numbers, so a field called `line` that means
  "row" is how the two coordinates get mixed up later. The frontend contains the damage by converting at exactly two
  places (`toRangeEnds` and `viewerSearchPoll`), and doc comments on both Rust types flag the mismatch.
- **Impact**: no user-visible bug today. It's a trap for the next change that reads `line` off the wire and feeds it to
  something that wants a physical line number (the gutter, the status bar's "N lines").
- **Solution**: rename to `SeekTarget::Row`, `RangeEnd::Row { row, offset }`, and `SearchMatch.row`; keep `RangeEnd::Eof`
  as is. Regenerate `bindings.ts`, then drop the two frontend conversions so `row` travels straight through. Mechanical
  but wide (about 18 Rust files plus the viewer route), so one commit of its own.
- **Size**: S–M. Not blocked.

## 2. A viewer fetch that times out keeps reading in the background

- **Problem**: `viewer_get_lines` runs under `blocking_typed_result_with_timeout` with the 2 s `VIEWER_TIMEOUT`. When it
  times out, the frontend gets `TimedOut`, but the blocking read has no cancel flag, so it runs to completion with
  nobody waiting on it. (A plain open that times out is handled: it's abandoned and the session it builds late is
  closed. Range reads and search already have their own cancel flags.)
- **Impact**: small since rows landed. Every read is bounded by `CHUNK_BUDGET_BYTES` (2 MiB) and two segments per row,
  so the orphaned work is sub-second on a healthy disk; before rows it was the 49 s read in `ERR-RQ8BY`. It still
  matters on a slow or wedged network mount, where each orphan holds a blocking-pool thread.
- **Solution**: thread a per-fetch cancel flag into `get_lines`, checked in the per-row loop the way `read_range` already
  checks its own, and flip it when the deadline fires.
- **Size**: S. Low priority, `someday` unless a report shows orphaned reads piling up on a slow mount.

## 3. The viewer's generic read error breaks the copy rules

- **Problem**: `viewer.error.readFailed` reads "Failed to read file" (`apps/desktop/src/lib/intl/messages/en/viewer.json`,
  shown by `routes/viewer/viewer-open-failure.ts` for any open failure without a more specific message). The style
  guide says error copy never uses "failed" and stays conversational and actionable.
- **Impact**: the one catch-all message a user sees when the viewer can't open a file is the least helpful one it has.
- **Solution**: rewrite it in the house voice (for example "Couldn't read this file. Try opening it again?"), then a
  translator pass over the 10 locales per `docs/guides/i18n-translation.md`.
- **Size**: S. Needs David's copy review.

## 4. Review the viewer's selection announcement copy

- **Problem**: the screen-reader announcement for a viewer selection now names the PHYSICAL line the gutter draws (it
  used to name the row index). Two strings cover the case where the line can't be resolved from the row cache:
  `viewer.selection.charsOnly` ("Selected {chars} characters") and `viewer.selection.toEndOfFileNoLine` ("Selected to
  the end of the file"). Both are agent drafts, already translated into all 10 locales.
- **Impact**: user-facing copy that hasn't had a human pass, per the "humans to humans" principle.
- **Solution**: David reads the two strings and the announcement shape (`routes/viewer/selection.svelte.ts`); any edit
  is a one-line change plus a translator pass.
- **Size**: S. Needs David's review.
