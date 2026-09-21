//! Stitches a `(line, offset)` -> `(line, offset)` range read into UTF-8 text,
//! independent of which backend the session uses.
//!
//! `read_range_streamed` is the engine: it hands the range to a sink in bounded pieces,
//! so the save-to-file path never holds more than one chunk. `read_range` is the same
//! read with a sink that collects, for callers (the clipboard) that want one string.
//!
//! Offsets on the wire are UTF-16 code units (matches JS string indexing and the search
//! engine's `SearchMatch.column`). Conversion to UTF-8 byte positions happens here, at
//! the IPC boundary, via `clamp_utf16_offset_to_byte`. Lone surrogates (offsets that land
//! between the high and low surrogate of an astral codepoint) are clamped down to the
//! nearest codepoint boundary, so the output is always valid UTF-8.
//!
//! Range semantics are half-open `[start, end)`, matching the frontend selection model:
//! the start line is included from `start.offset` to its end, intermediate lines are
//! included in full (with their trailing newline), the end line is included from offset 0
//! up to but not including `end.offset`.
//!
//! Cancellation: the reader checks the cancel flag periodically (after each line in the
//! current implementation; for very long lines we'd need a finer-grained check, but the
//! backends already cap line length implicitly through `MAX_BACKWARD_SCAN`). When the
//! flag is set, the function returns `ViewerError::Cancelled`.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;

use super::{FileViewerBackend, SeekTarget, ViewerError};

/// One endpoint of a selection. Frontend uses `Line { line, offset }`; for the
/// "select all" path in ByteSeek-no-index mode (where `totalLines` is unknown),
/// it uses `Eof` so the backend can resolve the end without a fake line number.
#[derive(Debug, Clone, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RangeEnd {
    Line { line: u64, offset: u32 },
    Eof,
}

impl RangeEnd {
    /// True if this endpoint is `Eof`.
    fn is_eof(&self) -> bool {
        matches!(self, Self::Eof)
    }
}

/// Compares two endpoints under the assumption that `Eof` is greater than every
/// `Line { ... }`. Returns `std::cmp::Ordering`.
fn compare_ends(a: &RangeEnd, b: &RangeEnd) -> std::cmp::Ordering {
    use std::cmp::Ordering as O;
    match (a, b) {
        (RangeEnd::Eof, RangeEnd::Eof) => O::Equal,
        (RangeEnd::Eof, _) => O::Greater,
        (_, RangeEnd::Eof) => O::Less,
        (RangeEnd::Line { line: la, offset: oa }, RangeEnd::Line { line: lb, offset: ob }) => {
            la.cmp(lb).then_with(|| oa.cmp(ob))
        }
    }
}

/// Returns the byte index inside `line` corresponding to the given UTF-16 code-unit
/// offset, clamping down to the nearest codepoint boundary if the offset lands between
/// the high and low surrogate of an astral codepoint.
///
/// For an offset >= the line's total UTF-16 length, returns `line.len()` (byte length).
pub fn clamp_utf16_offset_to_byte(line: &str, utf16_offset: u32) -> usize {
    let target = utf16_offset as usize;
    let mut utf16_count: usize = 0;
    for (byte_idx, ch) in line.char_indices() {
        if utf16_count >= target {
            return byte_idx;
        }
        utf16_count += ch.len_utf16();
        if utf16_count > target {
            // The offset landed inside a surrogate pair; clamp down to the codepoint
            // start (which is `byte_idx`).
            return byte_idx;
        }
    }
    line.len()
}

/// How much range text the streaming reader holds before handing it to its sink.
///
/// This is the save path's peak allocation, whatever the selection's size: the copy
/// dialog refuses a clipboard copy past 100 MiB and offers "Save as" as the way out, so
/// the save must not allocate the very thing the refusal protects the user from. 1 MiB
/// stays out of the way on any machine, and still costs a multi-GB save only a few
/// thousand writes.
pub(crate) const STREAM_CHUNK_BYTES: usize = 1024 * 1024;

/// Collects range text and hands it to `sink` in pieces of at most `chunk_bytes` plus
/// the tail of the line that crossed the threshold.
///
/// The `\n` that joins two lines is held back until the next line arrives, so the
/// range's final newline (which half-open semantics drop) never has to be taken back
/// out of a piece that already left for the sink.
struct ChunkedSink<'a, S: FnMut(&str) -> Result<(), ViewerError>> {
    buf: String,
    chunk_bytes: usize,
    sink: &'a mut S,
    newline_owed: bool,
}

impl<'a, S: FnMut(&str) -> Result<(), ViewerError>> ChunkedSink<'a, S> {
    fn new(sink: &'a mut S, chunk_bytes: usize) -> Self {
        Self {
            buf: String::new(),
            chunk_bytes,
            sink,
            newline_owed: false,
        }
    }

    /// Appends `text`, first paying any newline owed to the previous line.
    fn push(&mut self, text: &str) -> Result<(), ViewerError> {
        self.pay_newline();
        self.buf.push_str(text);
        if self.buf.len() >= self.chunk_bytes {
            self.flush()?;
        }
        Ok(())
    }

    /// Ends a line: the `\n` lands only once something follows it.
    fn end_line(&mut self) {
        self.newline_owed = true;
    }

    fn pay_newline(&mut self) {
        if self.newline_owed {
            self.buf.push('\n');
            self.newline_owed = false;
        }
    }

    /// Hands what's left to the sink. `keep_trailing_newline` pays a newline still
    /// owed, which only the "ran past the end line" exit wants.
    fn finish(mut self, keep_trailing_newline: bool) -> Result<(), ViewerError> {
        if keep_trailing_newline {
            self.pay_newline();
        }
        self.flush()
    }

    fn flush(&mut self) -> Result<(), ViewerError> {
        if !self.buf.is_empty() {
            (self.sink)(&self.buf)?;
            self.buf.clear();
        }
        Ok(())
    }
}

/// Reads the selected range from the given backend, returning a single UTF-8 string.
///
/// Holds the whole range in memory by definition; a caller that only wants to put the
/// range somewhere (the save-to-file path) uses [`read_range_streamed`] instead.
pub fn read_range(
    backend: &dyn FileViewerBackend,
    anchor: RangeEnd,
    focus: RangeEnd,
    cancel: &AtomicBool,
) -> Result<String, ViewerError> {
    let mut out = String::new();
    let mut sink = |piece: &str| {
        out.push_str(piece);
        Ok(())
    };
    read_range_streamed(backend, anchor, focus, cancel, STREAM_CHUNK_BYTES, &mut sink)?;
    Ok(out)
}

/// Reads the selected range and hands it to `sink` in pieces of at most `chunk_bytes`
/// (plus the tail of the line that crossed the threshold), so a caller that writes the
/// pieces straight out never holds more than one chunk.
///
/// Endpoints are normalised internally; reversed input (focus before anchor) produces
/// the same output as the forward range.
///
/// Returns `ViewerError::Cancelled` if `cancel` is flipped during the read, and
/// `ViewerError::OutOfRange` if the requested line is past the file's last line (with
/// the exception that `Eof` is always valid). A `sink` that fails stops the read with
/// its own error, and nothing further is read.
///
/// Streaming: after the initial seek by line number, the function advances by **byte
/// offset** rather than line number. This is mandatory for the ByteSeek backend, which
/// only estimates line numbers (`SeekTarget::Line(N)` resolves to `N * 80` bytes); for
/// FullLoad and LineIndex backends, byte-offset seeking is equally well-supported and
/// gives a single code path.
pub fn read_range_streamed<S: FnMut(&str) -> Result<(), ViewerError>>(
    backend: &dyn FileViewerBackend,
    anchor: RangeEnd,
    focus: RangeEnd,
    cancel: &AtomicBool,
    chunk_bytes: usize,
    sink: &mut S,
) -> Result<(), ViewerError> {
    let (start, end) = if compare_ends(&anchor, &focus).is_le() {
        (anchor, focus)
    } else {
        (focus, anchor)
    };

    // Resolve start line + offset. `Eof` as the start is unusual but well-defined:
    // empty selection at end of file.
    let (start_line, start_offset_utf16) = match start {
        RangeEnd::Line { line, offset } => (line as usize, offset),
        RangeEnd::Eof => return Ok(()),
    };

    // Validate start_line against backend's total_lines if known.
    if let Some(total) = backend.total_lines()
        && start_line >= total
    {
        return Err(ViewerError::OutOfRange);
    }

    // Resolve end. `Eof` means "to the last line, all of it"; otherwise we have an
    // explicit `Line { line, offset }`.
    let end_is_eof = end.is_eof();
    let (end_line, end_offset_utf16) = match end {
        RangeEnd::Line { line, offset } => (line as usize, offset),
        RangeEnd::Eof => (usize::MAX, 0),
    };

    let mut emit = ChunkedSink::new(sink, chunk_bytes);

    if start_line == end_line && !end_is_eof {
        // Single-line read: fetch the one line, clamp both offsets, slice between them.
        let chunk = backend.get_lines(&SeekTarget::Line(start_line), 1)?;
        let line = chunk.lines.first().ok_or(ViewerError::OutOfRange)?;
        let start_byte = clamp_utf16_offset_to_byte(line, start_offset_utf16);
        let end_byte = clamp_utf16_offset_to_byte(line, end_offset_utf16);
        let lo = start_byte.min(end_byte);
        let hi = start_byte.max(end_byte);
        emit.push(&line[lo..hi])?;
        if cancel.load(Ordering::Relaxed) {
            return Err(ViewerError::Cancelled);
        }
        return emit.finish(/*keep_trailing_newline=*/ false);
    }

    // Multi-line streaming read. First chunk is keyed by start line (only call that
    // uses `SeekTarget::Line` so we land on the right starting line). Subsequent chunks
    // are keyed by **byte offset** of the end of the last chunk, which is exact for all
    // three backends (ByteSeek's `Line(N)` is approximate; its byte-offset seeks are
    // exact, just back-scan for the surrounding newline).
    const FETCH_CHUNK: usize = 4096;
    // Cancellation budget inside the per-line loop. The plan's "every 64 KB" was the
    // target; we check whichever lands first: 256 lines (cheap line counter) or 64 KB
    // of emitted text (cheap byte counter). At typical 80-byte lines that's a check
    // every 20 KB; at 4 KB-per-line files (which would dwarf the 256-line cap) we'd
    // check every ~16 lines. Either way the worst-case latency between Escape and
    // `Cancelled` returning is well under the 100 ms threshold for "feels responsive."
    const CANCEL_CHECK_LINES: usize = 256;
    const CANCEL_CHECK_BYTES: usize = 64 * 1024;
    let mut next_target = SeekTarget::Line(start_line);
    let mut first_chunk = true;
    let mut lines_since_cancel_check: usize = 0;
    let mut bytes_since_cancel_check: usize = 0;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ViewerError::Cancelled);
        }

        let chunk = backend.get_lines(&next_target, FETCH_CHUNK)?;
        if chunk.lines.is_empty() {
            break;
        }

        // Compute the byte offset just past this chunk's last line.
        //
        // CRLF assumption: line readers in all three backends keep the `\r` AS PART of
        // the line string (they only split on `\n`; `&data[pos..pos + nl_pos]` retains
        // bytes before the newline byte). So `line.len()` already includes the `\r`
        // for CRLF files, and the `+ 1` accounts for the single `\n` delimiter byte.
        // No drift on either LF or CRLF files. See `byte_seek.rs:118`,
        // `full_load.rs:43`, `line_index.rs:172` for the parallel patterns. Test
        // fixture: `read_range_full_load_crlf_*` in `session_test.rs`.
        let mut chunk_end_offset = chunk.byte_offset;
        for line in &chunk.lines {
            chunk_end_offset += line.len() as u64 + 1;
        }

        let first_line_idx_in_chunk = chunk.first_line_number;

        for (i, line) in chunk.lines.iter().enumerate() {
            // Check the cancel flag periodically inside the inner loop. Doing it only
            // between chunks meant a single 4096-line chunk of 4 KB/line files (16 MB)
            // was uninterruptible. Now Escape lands within ~64 KB of emitted output.
            if lines_since_cancel_check >= CANCEL_CHECK_LINES || bytes_since_cancel_check >= CANCEL_CHECK_BYTES {
                if cancel.load(Ordering::Relaxed) {
                    return Err(ViewerError::Cancelled);
                }
                lines_since_cancel_check = 0;
                bytes_since_cancel_check = 0;
            }

            let line_number = first_line_idx_in_chunk + i;
            let is_first_overall = first_chunk && i == 0;

            // For explicit-end ranges, stop past the end line. The newline owed to the
            // last line emitted is part of the range here, so it's paid out.
            if !end_is_eof && line_number > end_line {
                return emit.finish(/*keep_trailing_newline=*/ true);
            }

            let text = if is_first_overall {
                // First line of the whole selection: take from start_offset to end of line.
                &line[clamp_utf16_offset_to_byte(line, start_offset_utf16)..]
            } else if !end_is_eof && line_number == end_line {
                // Last line of an explicit range: take from offset 0 up to end_offset, and
                // no trailing delimiter (the range is half-open). Exits the walk here, so
                // it never reaches the `end_line()` below.
                let end_byte = clamp_utf16_offset_to_byte(line, end_offset_utf16);
                emit.push(&line[..end_byte])?;
                return emit.finish(/*keep_trailing_newline=*/ false);
            } else {
                line.as_str()
            };
            emit.push(text)?;
            // The one place the walk decides a line carries its delimiter.
            emit.end_line();
            lines_since_cancel_check += 1;
            bytes_since_cancel_check += text.len() + 1;
        }

        first_chunk = false;

        // Termination: backend returned fewer lines than requested means EOF.
        if chunk.lines.len() < FETCH_CHUNK {
            break;
        }

        // Advance by byte offset for the next chunk.
        next_target = SeekTarget::ByteOffset(chunk_end_offset);
    }

    // For the Eof case (or a short file that ended before reaching an explicit end), the
    // newline owed to the very last line emitted is dropped rather than paid: half-open
    // semantics say "include the last line's full content but not a final implicit
    // newline boundary marker beyond it".
    emit.finish(/*keep_trailing_newline=*/ false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_offset_inside_ascii() {
        assert_eq!(clamp_utf16_offset_to_byte("hello world", 0), 0);
        assert_eq!(clamp_utf16_offset_to_byte("hello world", 5), 5);
        assert_eq!(clamp_utf16_offset_to_byte("hello world", 11), 11);
        // Past the end: clamps to byte length.
        assert_eq!(clamp_utf16_offset_to_byte("hello world", 99), 11);
    }

    #[test]
    fn clamp_offset_in_surrogate_pair() {
        // "👋hello" — emoji is 2 UTF-16 units (a high + low surrogate) and 4 UTF-8 bytes.
        let s = "👋hello";
        assert_eq!(clamp_utf16_offset_to_byte(s, 0), 0);
        // Offset 1: lands inside the surrogate pair; clamp down to codepoint start (0).
        assert_eq!(clamp_utf16_offset_to_byte(s, 1), 0);
        // Offset 2: end of the emoji, start of 'h' (byte 4).
        assert_eq!(clamp_utf16_offset_to_byte(s, 2), 4);
        // Offset 3: end of 'h' (byte 5).
        assert_eq!(clamp_utf16_offset_to_byte(s, 3), 5);
    }

    #[test]
    fn clamp_offset_in_multi_byte_utf8_but_single_utf16() {
        // "café" — 'é' is 2 UTF-8 bytes but 1 UTF-16 unit.
        let s = "café";
        assert_eq!(clamp_utf16_offset_to_byte(s, 0), 0);
        assert_eq!(clamp_utf16_offset_to_byte(s, 1), 1);
        assert_eq!(clamp_utf16_offset_to_byte(s, 2), 2);
        assert_eq!(clamp_utf16_offset_to_byte(s, 3), 3); // start of 'é'
        assert_eq!(clamp_utf16_offset_to_byte(s, 4), 5); // end of 'é', byte 5
    }

    /// The streamed read's peak memory is one chunk, whatever the range's size: no
    /// piece handed to the sink exceeds the chunk budget plus the line that crossed it.
    #[test]
    fn streamed_read_hands_out_bounded_pieces() {
        // 20 MB of range through a 64 KiB budget.
        let line_len = 999;
        let line_count = 20_000;
        let backend = crate::file_viewer::session::ScriptedBackend::new(&"z".repeat(line_len), line_count, |_| {});
        let chunk_bytes = 64 * 1024;

        let mut max_piece = 0usize;
        let mut total = 0usize;
        let mut sink = |piece: &str| {
            max_piece = max_piece.max(piece.len());
            total += piece.len();
            Ok(())
        };
        read_range_streamed(
            &backend,
            RangeEnd::Line { line: 0, offset: 0 },
            RangeEnd::Eof,
            &AtomicBool::new(false),
            chunk_bytes,
            &mut sink,
        )
        .unwrap();

        // Every line plus its joining newline, minus the newline past the last line.
        assert_eq!(total, line_count * (line_len + 1) - 1);
        assert!(
            max_piece <= chunk_bytes + line_len + 1,
            "peak piece was {max_piece} bytes against a {chunk_bytes}-byte budget"
        );
    }

    #[test]
    fn compare_ends_orders_eof_greatest() {
        let a = RangeEnd::Line { line: 5, offset: 3 };
        let b = RangeEnd::Line { line: 5, offset: 7 };
        let c = RangeEnd::Eof;
        assert!(compare_ends(&a, &b).is_lt());
        assert!(compare_ends(&b, &a).is_gt());
        assert!(compare_ends(&a, &c).is_lt());
        assert!(compare_ends(&c, &c).is_eq());
    }
}
