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

- `SEGMENT_BYTES = 20_000`. The grid the row boundaries sit on, and the length of every row inside a long line except
  its first. A row is at most just under `2 × SEGMENT_BYTES` (see the row rule for why the first row of a long line can
  be longer). Chosen for a readable horizontal scroll with wrap off; not a round power of two on purpose, so a stray
  16384/65536 elsewhere is obviously unrelated.
- `CHUNK_BUDGET_BYTES = 2 MiB`. The most one `viewer_get_lines` answer may carry. A fetch that hits it returns fewer
  rows; the frontend already copes with a short chunk by asking again. This is what keeps a wrap-on viewport from
  handing the offscreen height measurer megabytes of text, and it replaces the "shrink the segment when wrapped" idea,
  which would have broken I5.

## The row rule

Write it once, in one shared helper, and have both backends call it. Everything downstream trusts it, so it is defined
as a **boundary set** and the two functions are derived from it. No third definition anywhere.

A byte offset `b` is a row boundary exactly when one of these holds:

1. `b == 0`, or
2. `b` is the byte after a newline (the start of a physical line), or
3. `b` is a multiple of `SEGMENT_BYTES` **and the window `[b - SEGMENT_BYTES, b)` contains no newline.**

Then `row_start(offset)` is the greatest boundary `<= offset`, and `row_end(row_start)` is the least boundary
`> row_start`, or EOF.

Clause 3's second half is the whole design. Without it, an ordinary file gets a spurious break every 20 000 bytes,
because a line that happens to straddle a multiple gets cut (I6 dies). With it, a multiple only becomes a boundary
inside a run of `SEGMENT_BYTES` with no newline in it, which by definition only happens inside a line at least that
long. So:

- **Ordinary files are untouched, provably.** A `SEGMENT_BYTES`-wide window with no newline implies a line of at least
  `SEGMENT_BYTES`. Contrapositive: if every line is shorter than `SEGMENT_BYTES`, no multiple ever qualifies, no row
  ever ends anywhere but at a newline, and rows are exactly lines. That is I6, as a proof rather than a hope.
- **Rows stay bounded**, at just under `2 × SEGMENT_BYTES`. The worst case is a newline a byte or two before a
  multiple: that multiple is disqualified, so the row runs to the next one. Inside a long line every subsequent row is
  exactly `SEGMENT_BYTES`. A 300 MB line is one row under 40 000 bytes followed by ~15 000 rows of exactly 20 000.
- **Both directions stay O(1)**, bounded by a `2 × SEGMENT_BYTES` backward scan: that window contains every candidate
  boundary near `offset` AND the newline evidence needed to test clause 3 for each of them. Nothing depends on where
  the physical line starts or ends, which is I2.
- Boundaries are **absolute file offsets** in every backend, BOM or no BOM. `full_load.rs:70` scans `&bytes[bom_len..]`
  today; applying the rule to those shifted indices would put FullLoad's rows out of step with ByteSeek's across a
  reload or a tail escalation (`session.rs:1283`), which swap backends under a live `lineCache`.
- **State the rule in code units, not bytes, for UTF-16.** "The byte after a newline" is `nl + 2` for UTF-16 LE
  (`byte_seek.rs:213`, `full_load.rs:82-86`), and a character-boundary snap must never split a surrogate pair. Writing
  it in code units keeps the snap from looking like a removable optimization.
- This **replaces** today's `MAX_BACKWARD_SCAN = 8192` and its "no newline found, so treat `scan_start` as the line
  start" fallback, which was an approximation. The new rule is exact.
- On a file with no newline at all, `row_start(offset) == offset - offset % SEGMENT_BYTES` and row numbers come out
  exact, including in ByteSeek mode where line numbers are estimates today.

❌ **Do not derive `row_end` as `row_start + SEGMENT_BYTES`.** That reads like the same thing and is not: after a
newline at byte 100, iterating gives a row `[101, 20101)` while probing at 20050 gives `row_start = 20000`, so the same
byte belongs to two different rows and a byte-offset seek re-renders overlapping text. Derive both ends from the
boundary set, always.

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
   reconstructs text from row strings must join them with nothing at all. ❗ The backend is NOT already safe here, as an
   earlier draft of this spec claimed: `range_read.rs:215` and `:224` push a `'\n'` after every entry, so without a fix
   every segment break lands in the clipboard and in save-as as a real newline. Emit the delimiter only for a row that
   actually ended at one.
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
8. **Search is a second copy of the same unbounded read, and the same quadratic loop.** `byte_seek.rs:406` decodes a
   whole newline-free file into one `String`, and the `combined = leftover + chunk` rebuild at `byte_seek.rs:358-367`
   (mirrored at `line_index.rs:467-476`) is landmine 2 again. Fixing only `read_lines_ascii` leaves ⌘F on the reported
   file hanging exactly as before, so I1 would still fail on the very file that prompted this work. Search scans rows.
9. **`LineChunk.byte_offset` is a lie on the LineIndex backend.** `line_index.rs:427` returns the *checkpoint's* offset
   while `first_line_number` is the *target* line, and the code comment says so out loud ("approximate"). Anything that
   seeks onward from that offset lands short. Return the target row's real offset.

### What bounded rows break on the frontend

A per-fetch budget and 20 KB rows are not free on the other side of the IPC. Three of these are new breakage that this
change introduces, so they are part of it, not follow-ups.

- **A short chunk makes the fetch loop spin.** `viewer-scroll.svelte.ts:201` asks for ~267 rows; a 2 MiB budget answers
  with ~104 rows of 20 000 bytes. Row `to - 1` is then never cached, `needsFetch` at `:150` stays true, and a refetch
  fires every `FETCH_DEBOUNCE_MS` forever. **Drive the next fetch from the last row actually cached**, not from the
  requested range. This one is caused by `CHUNK_BUDGET_BYTES`, so it ships with it or not at all.
- **`lineCache` is never evicted.** It is a plain `SvelteMap` (`viewer-scroll.svelte.ts:40`) cleared only on open,
  reload, and encoding change (`+page.svelte:145,235,690`). At up to 40 KB per row that is fine today and parks
  hundreds of megabytes in the renderer once rows are long: scrolling through 1% of a 50 GB file is ~500 MB. Evict by
  distance from the viewport.
- **The height model does not survive wrapped 20 KB rows.** `avgWrappedLineHeight` (`viewer-scroll.svelte.ts:437`)
  would measure rows ~3 600 px tall while `linesOffset` spaces them at ~12 px, and the DOM height map is FullLoad-only
  (`+page.svelte:274`, `MAX_LINES` 50 000) so it never engages on the files that need it. **Size the render window by
  height, not by row count.**
- **`SeekTarget::Line(n)` estimates `n * 80` bytes** (`byte_seek.rs:251`), which is 250x wrong once `n` counts rows,
  and `range_read.rs:158` seeds its first chunk with it. ByteSeek should report `total_rows` and estimate from
  `SEGMENT_BYTES`, and the fraction path at `viewer-scroll.svelte.ts:237` must stop discarding
  `chunk.firstLineNumber` and caching at `fetchFrom` instead.
- **Checkpoints count lines, every 256 of them** (`mod.rs:75`, `line_index.rs:136`). A newline-free file therefore has
  exactly ONE checkpoint, and `read_lines_from_checkpoint` scans from byte 0 on every single fetch. The interval has to
  count ROWS, or the LineIndex backend violates I1 on precisely the file this work exists for. (And LineIndex IS
  reached on the 300 MB file: its scan finishes well inside `INDEXING_TIMEOUT_SECS`.)

### Milestone ordering

Findings that milestones 3-5 leave the app incoherent in between are correct but not a problem here: everything lands
on `worktree-viewer-row-wrap` and reaches `main` as one fast-forward, so `main` never sees a half-renamed coordinate.
Land 3, 4, and 5 as a series without stopping to make each independently shippable.

Milestone 6 is NOT as independent as this spec first claimed: save-as reaches the file through the same
`read_range` (`session.rs:975`). One streaming implementation, not two. The save-as work refactors `read_range` into a
form that can emit into a sink, and `write_range_to_file` drives it; the row changes then land in that one place.

### Carried over from the save-as milestone

- **Streaming save is bounded by one chunk PLUS one line**, because `ChunkedSink::push` appends a whole entry before
  testing the threshold. That is the right shape, and it is still unbounded on the 300 MB single-line file until rows
  land: the entry it appends IS the whole line. So "save-as of a newline-free 300 MB file holds one chunk, not the
  file" is part of milestone 3's DONE, not a separate concern. Nothing to change in the sink; it inherits the bound the
  moment its entries are rows.
- **ByteSeek and FullLoad disagree about the trailing empty line** of a newline-terminated file, so a `RangeEnd::Eof`
  read returns a different number of trailing newlines depending on which backend served it, i.e. on the file's size.
  Pre-existing, and the row work touches exactly this code. Pick one answer, pin it for all three backends, say which
  in the commit.

### Pre-existing bugs in the blast radius

These are broken on `main` today, independent of this change, and every one of them sits in code this work has to
touch. Fix them here, each in its own commit, each with a test that fails first.

- **⌘A can silently select nothing.** `viewer-keyboard.ts:344` does `deps.getLineText(totalLines - 1) ?? ''`, so when
  the last row isn't in `lineCache` (the common case on a long file: you ⌘A without having scrolled to the end) the
  selection ends at offset 0 of the last line. ⌘C then copies the file minus its last line, quietly. Take the
  `RangeEnd::Eof` path whenever the last row's text is absent, instead of inventing a zero.
- **Byte counts are off by one per line.** `+page.svelte:332` adds a byte per line and `selection.svelte.ts:337,351`
  subtract one, both assuming a newline delimiter. That already mis-sizes a file with no trailing newline, and after
  rows it mis-sizes every continuation row. The 10 MiB confirm threshold, the 100 MiB refusal, and the "on clipboard"
  toast all read these numbers, so they have to be true (I3).
- **`chunk_end_offset` is computed from decoded text.** `range_read.rs:185` does `line.len() as u64 + 1`, where
  `line.len()` is the UTF-8 length of the DECODED string. On a UTF-16 file that is not the source byte length, so
  multi-chunk ranges already drift today; the `+ 1` then breaks again on continuation rows. Have `LineChunk` carry the
  chunk's true source end offset instead of deriving it from strings. The CRLF reasoning in the comment above that
  line is sound and should survive.

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
   `read_lines_ascii_from` become linear; `LineChunk` gains the row fields and its true source end offset; the
   LineIndex checkpoints carry rows and lines from one scan; `CHUNK_BUDGET_BYTES` bounds the answer. `full_load.rs`
   and `media_backend.rs` keep working (a FullLoad file can still hold a long line, so it needs the rule too).
   **Search scans rows too** (landmine 8), including the same de-quadratic-ing of its `combined` rebuild; ⌘F on the
   300 MB file is part of this milestone's DONE, not a follow-up.
   Fix `LineChunk.byte_offset` on LineIndex (landmine 9) here, with `range_read`'s onward seek as the witness.
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
7. **The pre-existing bugs**, listed under "Pre-existing bugs in the blast radius" above: ⌘A's silent zero, the
   off-by-one byte counts, and `range_read`'s decoded-length arithmetic. Land each with its own red test. These can go
   before or after milestone 5, but not after milestone 8: they must be provably fixed against the new row model, and
   two of them decide whether I3 holds.
8. **Docs and close-out.** `file_viewer/CLAUDE.md` + `DETAILS.md`, `routes/viewer/CLAUDE.md` + `DETAILS.md`, the
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

## Bugs the characterization milestone found (all live on `main`)

Pinned as `bug_pinned_*` in `row_characterization_test.rs`, all in code milestone 3 rewrites, so milestone 3 fixes them
and converts each pin into an assertion of correct behaviour. The first two are silent data corruption in copy and
save, which puts them ahead of the feature work in importance.

1. **A partial copy in ByteSeek mode gives an empty clipboard.** `range_read` seeks its first chunk by
   `SeekTarget::Line`, and ByteSeek answers with its 80-bytes-a-line estimate, which lands past the range's end line;
   the loop then returns having emitted nothing. On a 20-byte-line file a single-line range comes back as a DIFFERENT
   line instead. Anything over 1 MB is exposed, and the whole-file gesture escapes only because it starts at line 0.
2. **A range over 4 096 lines on LineIndex duplicates a line at every chunk seam.** `range_read` seeks the next chunk
   by the byte offset past the last one, and LineIndex rounds that down to its previous checkpoint, re-serving a line
   that already went out. Copy and save-as of anything longer than 4 096 lines are corrupt today.
3. **Search finds nothing in a UTF-16 file** unless it is small enough for FullLoad: ByteSeek and LineIndex both
   `memchr(b'\n')` over raw bytes.
4. **ByteSeek surfaces the UTF-16 BOM as a selectable `U+FEFF`.**
5. **LineIndex rounds a byte-offset seek down** by up to 255 lines, where ByteSeek resolves it exactly.
6. **The three backends disagree about the trailing empty line** of a newline-terminated file, so a `RangeEnd::Eof`
   copy returns a different number of trailing newlines depending on which backend served it.

## Notes for whoever runs the lane

The rust lane carries exactly two expected failures until milestone 3 lands:
`red_first_fetch_of_a_newline_free_file_must_be_bounded` and `red_search_in_a_newline_free_file_must_be_bounded`. Any
other failure is real. (`macos-availability` also fails on a pre-existing SDK 27.0-vs-26.5 mismatch, unrelated.)
