# Viewer row wrapping: the viewer survives a line of any length

**Problem.** `ERR-RQ8BY`: F3 on a ~300 MB single-line minified JSON showed "Couldn't load the file. The volume may be
slow or unresponsive." after 2 s. The backend call actually returned 49 s later, and a second attempt ran concurrently
for another 46 s after its window had already given up. The viewer has no bound on how much of a line one read may
touch, so a file with no newline in it costs the whole file per fetch.

**Shape of the fix.** The viewer stops rendering physical lines and starts rendering **rows**: a row ends at a newline
or after `SEGMENT_BYTES`, whichever comes first. Nothing is dropped; a long line simply occupies several rows. A row
Cmdr ended itself carries a marker, so a break the user sees is never confused with a break the file contains.

## Invariants

The register every milestone is checked against. A change that breaks one of these is wrong even if every test passes.

- **I1. Bounded work.** No read path allocates or copies more than `O(SEGMENT_BYTES × rows_requested)`, whatever the
  file size or line length. Opening a 50 GB single-line file reads tens of kilobytes, not gigabytes.
- **I2. No total-length dependency.** Nothing needed to render a row may require knowing where its physical line ends,
  how long that line is, or how many lines the file has. This is what makes the 50 GB case work, and it is the invariant
  most easily lost by accident (a `memchr` "just to find the line end" reintroduces the bug).
- **I3. No silent truncation.** Anything the user can select, copy, or save is delivered in full or refused out loud.
  Byte counts shown to the user are true counts. This is the data-safety invariant; when in doubt, refuse rather than
  deliver less than asked.
- **I4. Deterministic rows.** `row_start(offset)` yields the same boundary for every probe offset inside that row, in
  both backends, on every fetch, in every encoding. Rows must not shift as the user scrolls back and forth.
- **I5. Rows do not depend on wrap mode.** Toggling word wrap must not renumber a single row. (Which is why the fetch
  size, not the segment size, is what adapts to wrapped rows.)
- **I6. Ordinary files are unaffected.** For any file whose every line is shorter than `SEGMENT_BYTES`, rows and lines
  are one-to-one and every observable behavior is byte-identical to today.

## Constants

- `SEGMENT_BYTES = 20_000`. The longest a row may be. Chosen for a readable horizontal scroll with wrap off; not a
  round power of two on purpose, so a stray 16384/65536 elsewhere is obviously unrelated.
- `CHUNK_BUDGET_BYTES = 2 MiB`. The most one `viewer_get_lines` answer may carry. A fetch that hits it returns fewer
  rows; the frontend already copes with a short chunk by asking again. This is what keeps a wrap-on viewport from
  handing the offscreen height measurer megabytes of text, and it replaces the "shrink the segment when wrapped" idea,
  which would have broken I5.

## The row rule

Write it once, in one shared helper, and have both backends call it. Two functions, no third definition anywhere:

```
row_start(offset)  = max( byte after the last `\n` at or before `offset`,
                          largest multiple of SEGMENT_BYTES at or before `offset` )
                     then snapped FORWARD to the next character boundary
row_end(row_start) = min( byte after the next `\n` at or after `row_start`,
                          row_start + SEGMENT_BYTES snapped BACK to a character boundary,
                          end of file )
```

Boundaries are **anchored to absolute file offsets, never to the start of the physical line.** Anchoring to the line
start would require finding that start, which on a single-line file means scanning to byte 0. Absolute anchoring makes
both directions O(1) from any offset and satisfies I2 and I4 at once.

Consequences worth knowing before you implement:

- The backward scan for a newline is bounded by `SEGMENT_BYTES`, because a segment boundary always stops it. This
  **replaces** today's `MAX_BACKWARD_SCAN = 8192` and its "no newline found, treat scan_start as the line start"
  fallback, which was an approximation; the new rule is exact.
- "Character boundary" is encoding-specific: UTF-8 snaps off continuation bytes (`0b10xxxxxx`, at most 3 bytes of
  adjustment); UTF-16 snaps to an even offset and never splits a surrogate pair. Both are decidable locally, in O(1),
  with no knowledge of anything outside a few bytes. Keep it that way.
- A forward snap can in principle reach past a newline that sits within those few bytes. Take the later of the two
  candidates after snapping, and pin the case with a test.
- On a file with no newline at all, `row_start(offset) == offset - offset % SEGMENT_BYTES` and row numbers come out
  exact, including in ByteSeek mode where line numbers are estimates today.

## Wire model

`LineChunk` stops being a bag of strings and starts describing rows:

- Each row carries its `text`, its `byte_offset`, whether it `continues` (it ended at a segment boundary rather than at
  a newline or EOF), and the physical line it belongs to: `Some(n)` on a row that starts a line, `None` on a
  continuation row. The gutter prints the number on the first row of a line and nothing on its continuations, the usual
  editor convention.
- `total_lines` becomes `total_rows`; `first_line_number` becomes the first row's index.
- `SeekTarget::Line` becomes `SeekTarget::Row`, `SearchMatch.line` becomes a row index with `column` relative to that
  row, and `RangeEnd::Line { line, offset }` becomes `RangeEnd::Row { row, offset }`. Rename rather than redefine: a
  field called `line` that means "row" is how the two coordinates get mixed up six months from now. `RangeEnd::Eof`
  stays exactly as it is.

Everything the frontend counts in (`lineCache`, `heightMap`, `scrollScale`, `estimatedTotalLines`, `visibleFrom` /
`visibleTo`, the selection's `(line, offset)` endpoints) is already a row index in all but name, so the frontend change
is mostly a rename plus the gutter and the marker.

## The marker

A row with `continues: true` draws a marker at the end of its text, **identically whether word wrap is on or off**. It
says one thing: this break is Cmdr's, not the file's.

- Not `⏎`. That glyph means "there is a line break here", which is the opposite of the truth. Use a distinct mark with
  a tooltip; the exact glyph and colour are David's call and he reviews the copy before it ships.
- It must not be selectable and must not reach the clipboard. A `::after` with `content` satisfies both, which then
  needs its own visually-hidden label so assistive tech still hears it.
- Red must still clear AA+ contrast in both themes; take it from a theme token, never a literal.
- Tooltip and label go through i18n (`apps/desktop/src/lib/intl/messages/en/viewer.json`), sentence case, no
  "just/simple", no em dash, per `docs/style-guide.md`.

**Open question for David, do not decide it here:** with word wrap ON the user also sees soft breaks that WebKit made,
and those stay unmarked under this spec. Marking those too would mean Cmdr doing its own wrapping instead of CSS, which
contradicts the deliberate "measure, don't predict" decision in `viewer-line-heights.svelte.ts`. This spec marks the
segment break only, in both modes. Flag it in the final report.

## Landmines

Each of these is a way to ship something that passes tests and is still wrong.

1. **Joining rows with `\n` anywhere.** A Cmdr break is not a newline. Any copy, save, search, or announcement path that
   reconstructs text from row strings must join them with nothing at all. The backend byte-range paths are already
   safe because they read the file; check the frontend for a fast path that does not.
2. **`read_lines_ascii` / `read_lines_ascii_from` are quadratic.** Per 64 KB chunk with no newline in it, they copy the
   whole accumulated `leftover` into a fresh `combined` and then back again: ~1.4 TB of `memcpy` for a 300 MB line,
   which is the 49 s in the report. Rows bound it, but fix the loop too: `get_lines` has callers outside the viewer
   (`agent/tools/read/inspect/`) and the next reader deserves a linear function.
3. **Select-all must stay honest.** `viewer-keyboard.ts` builds ⌘A from `getLineText(totalLines - 1).length`. Once that
   is a row it is still correct, but only while `totalRows` and the last row's text agree. `EOF_LINE` →
   `RangeEnd::Eof` stays the path for "we have no count yet"; do not replace it with arithmetic.
4. **Save-as does not stream, whatever its comment says.** See the separate milestone below.
5. **Encoding.** UTF-16 rows must split on code-unit pairs, and a decoded row must never contain a replacement
   character produced by our own boundary. Both encodings need the round-trip test.
6. **The LineIndex checkpoint array counts lines.** It has to count rows too (keep both: rows for seeking, lines for
   the gutter), and the row count has to come out of the same single scan; a second pass over the file breaks I1.
7. **Search columns are UTF-16 code units.** Converting a match's byte offset to a column within its row means decoding
   the row prefix, which is bounded by `SEGMENT_BYTES`. Bounded is fine; unbounded is the bug.

## Milestones

Each lands as its own commit (or a small series), green before the next starts.

1. **Characterize.** Regression tests pinning TODAY's behavior for ordinary files: open, scroll, search, copy,
   select-all, save-as, both encodings, all three backends. Pin reality, not what this spec wishes were true. Plus two
   tests that must be RED now: a newline-free file whose first fetch is bounded (structural: assert bytes touched, so
   CI cannot flake on it), and a save-as of a range larger than memory comfort. Red must be seen and reported.
2. **The row rule.** The shared `row_start` / `row_end` helper with its property tests (idempotence from every probe
   offset in a row, both encodings, the snap-past-newline edge). No backend wired up yet. Pure functions, exhaustively
   tested, because everything downstream trusts them.
3. **Backends emit rows.** `byte_seek.rs` and `line_index.rs` call the helper; `read_lines_ascii` and
   `read_lines_ascii_from` become linear; `LineChunk` gains the row fields; the LineIndex checkpoints carry rows and
   lines from one scan; `CHUNK_BUDGET_BYTES` bounds the answer. `full_load.rs` and `media_backend.rs` keep working
   (a FullLoad file can still hold a long line, so it needs the rule too).
4. **Rename the coordinate across IPC.** `SeekTarget::Row`, `SearchMatch` on rows, `RangeEnd::Row`. Mechanical but
   wide; its own commit so the interesting commits stay readable.
5. **Frontend rows.** Gutter (number on a line's first row, blank on continuations), marker plus tooltip plus
   visually-hidden label plus i18n, the row rename through `viewer-scroll`, `selection`, `viewer-keyboard`,
   `viewer-copy`, `viewer-search-scroll`. Verify no path joins rows with `\n`.
6. **Save-as streams.** `session.rs::write_range_to_file` currently does `read_range(..)` into one `String` and then
   `fs::write`, so the > 100 MiB copy refusal hands the user a button that allocates exactly what the refusal just
   protected them from. Make it chunk source → temp → rename, honouring the per-read cancel flag and keeping the
   temp+rename atomicity. It must keep DECODING (a UTF-16 file saves as UTF-8 text today; a raw byte copy would
   silently change that), so chunk on row boundaries and decode each chunk. Then the comment claiming it streams
   becomes true. Independent of milestones 2-5; can run in parallel.
7. **Docs and close-out.** `file_viewer/CLAUDE.md` + `DETAILS.md`, `routes/viewer/CLAUDE.md` + `DETAILS.md`, the
   `docs/specs/index.md` entry. An adversarial pass against the invariants above, and a docs audit that reads every
   commit body.

## Out of scope, on purpose

- **The abandoned read keeps running.** `blocking_typed_result_with_timeout` mints `TimedOut` and abandons the pending
  open, but the blocking read has no cancel flag, so it runs to completion unwatched. Bounded rows make it a
  sub-second waste instead of a 49 s one, so it stops being urgent. Worth a follow-up; not this change.
- **The 2 s `VIEWER_TIMEOUT`** stays as it is. With bounded reads it no longer fires on this case.
- **ByteSeek row numbers are estimates mid-file** for files that do contain newlines, exactly as line numbers are
  estimates today. Unchanged, not made worse. (On a newline-free file they become exact, which is a small win.)
- **`viewer.error.readFailed` = "Failed to read file"** breaks the style guide twice over. Real, unrelated, leave it.
