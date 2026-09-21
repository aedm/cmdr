//! The row boundary rule: where the viewer breaks a file into rows.
//!
//! The viewer renders **rows**, not physical lines. A row ends at a newline or
//! after a segment, whichever comes first, so a file with no newline in it costs
//! a bounded read per fetch instead of the whole file. Nothing is dropped; a long
//! line simply occupies several rows.
//!
//! The rule is defined once, as a **boundary set**, and both `row_start` and
//! `row_end` are derived from it. A byte offset `b` is a row boundary exactly
//! when one of these holds:
//!
//! 1. `b == 0`, or
//! 2. `b` is the start of a physical line (the offset just past a newline), or
//! 3. `b` is a multiple of [`SEGMENT_BYTES`] **below EOF** **and the window
//!    `[b - SEGMENT_BYTES, b)` contains no newline**, snapped back to the nearest
//!    character start.
//!
//! Then `row_start(offset)` is the greatest boundary `<= offset`, and
//! `row_end(row_start)` is the least boundary `> row_start`, or EOF.
//!
//! ❗ **"Below EOF" in clause 3 is load-bearing.** A multiple sitting exactly on EOF
//! cannot start a row, because nothing follows it. Admitting it looks harmless and is,
//! until the file ALSO ends mid-character: the snap then drags it back below EOF and
//! invents a boundary, splitting a couple of truncated bytes off as their own row. The
//! same byte then belongs to two rows depending on which direction you read the rule
//! from, which is invariant I4.
//!
//! ❗ **Clause 3's second half is the whole design, not an optimization.** A
//! `SEGMENT_BYTES`-wide window with no newline implies a line at least that long.
//! Contrapositive: if every line is shorter than `SEGMENT_BYTES`, no multiple ever
//! qualifies, no row ever ends anywhere but at a newline, and rows are exactly
//! lines. Drop the window test and every ordinary file gains a spurious break
//! every 20 000 bytes, because a line straddling a multiple gets cut. (A line of
//! EXACTLY `SEGMENT_BYTES` is not shorter than one, so it does split: into a full
//! row plus a row holding only its newline.)
//!
//! ❌ **Do not derive `row_end` as `row_start + SEGMENT_BYTES`.** It reads like the
//! same thing and is not: after a newline at byte 100, iterating gives a row
//! `[101, 20101)` while probing at 20050 gives `row_start = 20000`, so the same
//! byte belongs to two different rows and a byte-offset seek re-renders overlapping
//! text. Both ends come from the boundary set, always.
//!
//! **Boundaries are absolute file offsets, BOM or no BOM.** `full_load.rs` scans
//! BOM-stripped indices; anchoring rows to those would put FullLoad's rows out of
//! step with ByteSeek's across a reload or a tail escalation, which swap backends
//! under a live frontend row cache.
//!
//! Architecture and the rest of the plan: `docs/specs/viewer-row-wrap.md`.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use serde::Serialize;

use super::ViewerError;
use super::encoding::{FileEncoding, NewlineScanner, decode_line};

/// The grid row boundaries sit on, and the length of every row inside a long line
/// except its first.
///
/// A row is at most just under `2 × SEGMENT_BYTES`: a newline exactly on a multiple
/// disqualifies the next one, so the row runs to the one after. Chosen for a
/// readable horizontal scroll with wrap off, and deliberately not a round power of
/// two, so a stray 16384 / 65536 elsewhere is obviously unrelated.
pub const SEGMENT_BYTES: u64 = 20_000;

/// The most bytes a single `row_start` or `row_end` call may read.
///
/// One window of this size answers everything either function needs: every
/// candidate boundary near the probe, plus the newline evidence that decides
/// clause 3 for each of them. Nothing here depends on where the physical line
/// starts or ends, which is what makes a 50 GB single-line file cheap.
pub const MAX_WINDOW_BYTES: u64 = 2 * SEGMENT_BYTES;

/// Where a file's content starts: past its BOM, if it actually has one.
///
/// ❗ `encoding.bom_bytes().len()` is NOT the answer. An encoding that CAN carry a BOM
/// doesn't mean this file does: `encoding::detect_from_head` reaches UTF-16 without one
/// through its parity heuristic, and a manual encoding switch lands a backend here too.
/// Assuming the BOM drops the file's first character from every read, and shifts one
/// backend's row 0 against another's, which the ByteSeek→LineIndex upgrade then slides
/// under a live row cache.
///
/// `head` is the file's first bytes; anything at least as long as the BOM will do.
pub fn content_start(head: &[u8], encoding: FileEncoding) -> u64 {
    let bom = encoding.bom_bytes();
    if !bom.is_empty() && head.starts_with(bom) {
        bom.len() as u64
    } else {
        0
    }
}

/// A file the rule can read windows of.
///
/// The rule never opens a file itself, so a caller hands it an in-memory slice
/// ([`SliceSource`], for `FullLoadBackend` and tests) or an open file
/// ([`FileSource`]). Tests wrap a source to count bytes, which is what makes the
/// bounded-work property structurally checkable instead of a timing guess.
pub trait RowSource {
    /// The file's size in bytes. Row boundaries never exceed it.
    fn total_bytes(&self) -> u64;

    /// Fill `buf` with bytes starting at `start`, returning how many were filled.
    /// A short fill means EOF, or a file that shrank under us.
    fn read_window(&mut self, start: u64, buf: &mut [u8]) -> Result<usize, ViewerError>;
}

/// A [`RowSource`] over bytes already in memory.
pub struct SliceSource<'a> {
    bytes: &'a [u8],
}

impl<'a> SliceSource<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}

impl RowSource for SliceSource<'_> {
    fn total_bytes(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn read_window(&mut self, start: u64, buf: &mut [u8]) -> Result<usize, ViewerError> {
        let start = (start as usize).min(self.bytes.len());
        let available = &self.bytes[start..];
        let filled = available.len().min(buf.len());
        buf[..filled].copy_from_slice(&available[..filled]);
        Ok(filled)
    }
}

/// A [`RowSource`] over an open file.
pub struct FileSource {
    file: File,
    total_bytes: u64,
}

impl FileSource {
    pub fn new(file: File, total_bytes: u64) -> Self {
        Self { file, total_bytes }
    }
}

impl RowSource for FileSource {
    fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    fn read_window(&mut self, start: u64, buf: &mut [u8]) -> Result<usize, ViewerError> {
        self.file.seek(SeekFrom::Start(start))?;
        let mut filled = 0usize;
        while filled < buf.len() {
            let read = self.file.read(&mut buf[filled..])?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        Ok(filled)
    }
}

/// Applies the row boundary rule to one source, reusing one window buffer.
///
/// Hold on to a ruler across a fetch: the buffer grows to [`MAX_WINDOW_BYTES`] and
/// refilling it beats reallocating it per row.
pub struct RowRuler<S: RowSource> {
    source: S,
    encoding: FileEncoding,
    /// The grid boundaries sit on. Always [`SEGMENT_BYTES`] outside tests, which
    /// use a tiny even segment so every property can be probed at every offset of
    /// dozens of crafted files instead of sampled on a handful of big ones.
    segment: u64,
    buf: Vec<u8>,
}

impl<S: RowSource> RowRuler<S> {
    pub fn new(source: S, encoding: FileEncoding) -> Self {
        Self {
            source,
            encoding,
            segment: SEGMENT_BYTES,
            buf: Vec::new(),
        }
    }

    /// The same rule on a smaller grid, for tests only.
    ///
    /// The segment must be even (UTF-16 code units are two bytes, and the grid has
    /// to stay code-unit aligned) and comfortably longer than the longest
    /// character, or clause 3's "a newline-free segment implies a long line"
    /// reasoning stops holding.
    #[cfg(test)]
    pub fn with_segment(source: S, encoding: FileEncoding, segment: u64) -> Self {
        assert!(
            segment >= 8 && segment.is_multiple_of(2),
            "segment must be even and >= 8"
        );
        Self {
            source,
            encoding,
            segment,
            buf: Vec::new(),
        }
    }

    pub fn total_bytes(&self) -> u64 {
        self.source.total_bytes()
    }

    /// The greatest row boundary `<= offset`.
    ///
    /// Reads exactly one window of at most [`MAX_WINDOW_BYTES`], whatever the
    /// offset and whatever the file size.
    ///
    /// Production seeks through [`RowRuler::row_start_detail`], which answers this plus
    /// the clause that placed the boundary. This plain form is the shape the property
    /// tests state the rule in, and is kept for them.
    #[cfg(test)]
    pub fn row_start(&mut self, offset: u64) -> Result<u64, ViewerError> {
        Ok(self.row_start_detail(offset)?.offset)
    }

    /// `RowRuler::row_start`, plus which clause put the boundary there.
    ///
    /// A forward walk needs both halves: `from_newline` is what makes a row print a
    /// line number in its gutter, and it is also the entire newline evidence the NEXT
    /// boundary needs (see [`next_row_boundary`]). So a fetch pays for one window on
    /// its first row and none on the thousands after it.
    pub fn row_start_detail(&mut self, offset: u64) -> Result<RowStart, ViewerError> {
        let total = self.source.total_bytes();
        let offset = offset.min(total);
        let segment = self.segment;
        // The window spans the multiple at or below `offset` and the one above it.
        // That holds everything any clause can need: the newline evidence for both
        // candidate multiples, and every line start at or below `offset`.
        //
        // ❗ The grid is measured from the last BYTE, not from `offset`, which differs
        // only when `offset == total`. No boundary sits at EOF, so a probe there is
        // asking which row the file's last byte belongs to; measuring from `offset`
        // would centre the window one segment too high and find no candidate at all.
        let grid_probe = if offset >= total {
            total.saturating_sub(1)
        } else {
            offset
        };
        let grid = grid_probe - grid_probe % segment;
        let window_start = grid.saturating_sub(segment);
        let window_end = (grid + segment).min(total);
        self.load_window(window_start, window_end)?;
        let window = Window::new(&self.buf, window_start, self.encoding, segment, total);

        // Clause 1 is the floor: 0 is always a boundary.
        let mut best = 0u64;
        let mut from_newline = false;
        // Clause 3: the multiple at or below `offset`, plus the one above it,
        // which a character straddling it can pull back to at or below `offset`.
        for multiple in [grid, grid + segment] {
            if let Some(boundary) = window.clause_three_boundary(multiple)
                && boundary <= offset
                && boundary > best
            {
                best = boundary;
                from_newline = false;
            }
        }
        // Clause 2: the last line start at or below `offset`. It takes a tie with a
        // multiple, because a boundary that is both is one the FILE contains: the row
        // ending there is the file's break, not ours.
        if let Some(line_start) = window.last_line_start_at_or_below(offset)
            && line_start >= best
        {
            best = line_start;
            from_newline = true;
        }
        Ok(RowStart {
            offset: best,
            from_newline,
        })
    }

    /// The least row boundary `> row_start`, or EOF.
    ///
    /// `row_start` is expected to be a boundary, one `RowRuler::row_start`
    /// returned. Reads one window of at most [`MAX_WINDOW_BYTES`].
    ///
    /// ❗ This is the rule's canonical forward step, and the reference the streaming
    /// [`next_row_boundary`] is checked against on every fixture. Production reads
    /// forward instead, because a window per row would cost a 4 096-row fetch ~160 MB;
    /// ❌ don't delete this as unused, it is what keeps that walk honest.
    #[cfg(test)]
    pub fn row_end(&mut self, row_start: u64) -> Result<u64, ViewerError> {
        let total = self.source.total_bytes();
        if row_start >= total {
            return Ok(total);
        }
        let segment = self.segment;
        // The next boundary always lands below `row_start + 2 × segment`, so the
        // two multiples above `row_start` are the only clause-3 candidates. The
        // window opens a segment below the first of them, where its own newline
        // evidence begins.
        let next_multiple = row_start - row_start % segment + segment;
        let window_start = next_multiple - segment;
        let window_end = (next_multiple + segment).min(total);
        self.load_window(window_start, window_end)?;
        let window = Window::new(&self.buf, window_start, self.encoding, segment, total);

        // EOF ends the last row; anything below it wins.
        let mut best = total;
        if let Some(line_start) = window.first_line_start_above(row_start) {
            best = best.min(line_start);
        }
        for multiple in [next_multiple, next_multiple + segment] {
            if let Some(boundary) = window.clause_three_boundary(multiple)
                && boundary > row_start
            {
                best = best.min(boundary);
            }
        }
        Ok(best)
    }

    /// Read straight from the underlying source, for a caller that does its own
    /// buffering ([`RowReader`]). The ruler's own window is untouched.
    fn read_into(&mut self, start: u64, buf: &mut [u8]) -> Result<usize, ViewerError> {
        self.source.read_window(start, buf)
    }

    /// Refill `self.buf` with `[start, end)`. A short read means EOF, or a file
    /// that shrank under us; the rule then works from what it got.
    fn load_window(&mut self, start: u64, end: u64) -> Result<(), ViewerError> {
        let len = end.saturating_sub(start) as usize;
        debug_assert!(
            len as u64 <= 2 * self.segment && (self.segment != SEGMENT_BYTES || len as u64 <= MAX_WINDOW_BYTES),
            "a window may never exceed two segments"
        );
        self.buf.resize(len, 0);
        let filled = self.source.read_window(start, &mut self.buf[..len])?;
        self.buf.truncate(filled);
        Ok(())
    }
}

/// One loaded window, and the three clauses read out of it.
///
/// Every boundary question the rule asks is answerable from a window of two
/// segments, which is the whole reason a 50 GB single-line file is cheap: nothing
/// here needs to know where the physical line starts or ends.
struct Window<'a> {
    bytes: &'a [u8],
    /// Absolute offset of `bytes[0]`. Always a multiple of the segment, or 0, so
    /// the UTF-16 scan starts code-unit aligned.
    start: u64,
    /// Absolute offsets of the code units holding `U+000A`, ascending.
    newlines: Vec<u64>,
    encoding: FileEncoding,
    segment: u64,
    total_bytes: u64,
}

impl<'a> Window<'a> {
    fn new(bytes: &'a [u8], start: u64, encoding: FileEncoding, segment: u64, total_bytes: u64) -> Self {
        Self {
            bytes,
            start,
            newlines: newline_unit_starts(bytes, start, encoding),
            encoding,
            segment,
            total_bytes,
        }
    }

    /// One past the last byte actually read. Below the requested end when the file
    /// shrank under us.
    fn end(&self) -> u64 {
        self.start + self.bytes.len() as u64
    }

    /// How far past a newline its line starts, in bytes.
    ///
    /// ❗ 2 for UTF-16: the newline is a two-byte code unit, so the line starts at
    /// `unit + 2`. Stating the rule in code units rather than raw bytes is what
    /// keeps a UTF-16 row from starting mid-unit; it isn't an optimization to fold
    /// away.
    fn newline_len(&self) -> u64 {
        match self.encoding {
            FileEncoding::Utf16Le | FileEncoding::Utf16Be => 2,
            _ => 1,
        }
    }

    /// Clause 2, looking back: the greatest line start `<= limit`.
    fn last_line_start_at_or_below(&self, limit: u64) -> Option<u64> {
        self.newlines
            .iter()
            .rev()
            .map(|unit| unit + self.newline_len())
            .find(|line_start| *line_start <= limit)
    }

    /// Clause 2, looking forward: the least line start `> floor`.
    #[cfg(test)]
    fn first_line_start_above(&self, floor: u64) -> Option<u64> {
        self.newlines
            .iter()
            .map(|unit| unit + self.newline_len())
            .find(|line_start| *line_start > floor)
    }

    /// Clause 3: the boundary `multiple` contributes, if it contributes one.
    ///
    /// `None` when the multiple is 0 or past EOF (clause 1 covers the first, and a
    /// boundary past the last byte would end a row nobody can render), when a
    /// newline falls in the segment before it, or when the window came up short of
    /// the evidence needed to decide. Otherwise the multiple, snapped back to the
    /// nearest character start.
    ///
    /// ❗ The newline test is what keeps ordinary files whole. A newline-free
    /// segment can only sit inside a line at least that long, so in a file whose
    /// lines are all shorter than a segment NO multiple ever qualifies and rows
    /// come out exactly equal to lines. Drop the test and every ordinary file
    /// gains a spurious break every segment, because a line straddling a multiple
    /// gets cut.
    fn clause_three_boundary(&self, multiple: u64) -> Option<u64> {
        // ❗ `>=`, not `>`. A multiple sitting exactly ON EOF cannot start a row, because
        // nothing follows it. Admitting it is harmless until the file ALSO ends
        // mid-character: the snap then drags it back below EOF and invents a boundary,
        // splitting a couple of truncated bytes off as their own row. `row_start` would
        // put the file's last byte in that invented row while the forward walk put it in
        // the row a segment earlier, which is the same byte in two rows (I4).
        if multiple == 0 || multiple >= self.total_bytes {
            return None;
        }
        let evidence_start = multiple - self.segment;
        if evidence_start < self.start || multiple > self.end() {
            // The window doesn't hold the whole preceding segment, so we can't
            // prove it is newline-free. Treat the multiple as disqualified:
            // falling back to an earlier boundary is safe, inventing one is not.
            return None;
        }
        if self
            .newlines
            .iter()
            .any(|unit| *unit >= evidence_start && *unit < multiple)
        {
            return None;
        }
        Some(snap_back_to_char_start(self.bytes, self.start, multiple, self.encoding))
    }
}

/// The greatest character start `<= pos`, decided from the bytes below `pos` alone.
///
/// `bytes` is any window holding them; `bytes_start` is its absolute offset. Free
/// rather than a [`Window`] method because the forward walk ([`RowReader`]) holds its
/// own buffer and has to snap clause 3's multiples with the same arithmetic. See
/// [`Window::clause_three_boundary`] for why only clause 3 needs this.
///
/// A boundary inside a character would make the decoder emit a replacement character
/// we invented ourselves, on both sides of the break. Reading only downward is what
/// lets a window ending exactly at `pos` still decide it, which is how the ruler fits
/// its reads in one segment pair. The snap moves at most three bytes, so it can't
/// reach the previous boundary (a qualifying multiple has a whole newline-free segment
/// behind it) and can't push a row past its length bound.
fn snap_back_to_char_start(bytes: &[u8], bytes_start: u64, pos: u64, encoding: FileEncoding) -> u64 {
    if pos <= bytes_start {
        return pos;
    }
    let idx = (pos - bytes_start) as usize;
    if idx > bytes.len() {
        return pos;
    }
    match encoding {
        FileEncoding::Utf16Le | FileEncoding::Utf16Be => {
            if idx < 2 {
                return pos;
            }
            let unit = if matches!(encoding, FileEncoding::Utf16Le) {
                u16::from_le_bytes([bytes[idx - 2], bytes[idx - 1]])
            } else {
                u16::from_be_bytes([bytes[idx - 2], bytes[idx - 1]])
            };
            // A high surrogate right below `pos` means its low half starts AT
            // `pos`: one unit back keeps the pair in one row. Capped at one
            // step, because a RUN of lone high surrogates is malformed input
            // and an unbounded walk would break the row length bound.
            if (0xD800..=0xDBFF).contains(&unit) {
                pos - 2
            } else {
                pos
            }
        }
        FileEncoding::Utf8 | FileEncoding::Utf8WithBom | FileEncoding::UsAscii => {
            // Walk back over continuation bytes (at most three), then ask
            // whether the lead byte below them covers `pos`.
            let mut back = 0usize;
            while back < 3 && idx >= back + 2 && is_utf8_continuation(bytes[idx - back - 1]) {
                back += 1;
            }
            if idx < back + 1 {
                return pos;
            }
            let lead_pos = pos - back as u64 - 1;
            let char_len = utf8_char_len(bytes[idx - back - 1]);
            if lead_pos + char_len > pos { lead_pos } else { pos }
        }
        // The Western single-byte encodings map every byte to a character, so
        // no offset can split one.
        FileEncoding::Windows1252 | FileEncoding::Iso8859_1 | FileEncoding::MacRoman => pos,
    }
}

/// Byte offsets of the code unit holding `U+000A`, absolute, ascending.
///
/// [`NewlineScanner`] reports the offset of the `0x0A` BYTE, which for UTF-16 BE is
/// the second half of the pair; the rule wants the unit's own start, because that
/// is what the segment window is measured in and what `+ newline_len` turns into a
/// line start.
fn newline_unit_starts(window: &[u8], window_start: u64, encoding: FileEncoding) -> Vec<u64> {
    let mut scanner = NewlineScanner::new(encoding, window_start);
    let big_endian = matches!(encoding, FileEncoding::Utf16Be);
    let mut out = Vec::new();
    scanner.feed(window, |offset| {
        out.push(if big_endian { offset - 1 } else { offset });
    });
    out
}

fn is_utf8_continuation(byte: u8) -> bool {
    byte & 0xC0 == 0x80
}

/// Bytes in the character a lead byte starts. A malformed lead counts as 1, which
/// leaves the boundary where it was.
fn utf8_char_len(lead: u8) -> u64 {
    match lead {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

// ---------------------------------------------------------------------------
// The same boundary set, read forward
// ---------------------------------------------------------------------------

/// A row boundary, and which clause put it there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowStart {
    pub offset: u64,
    /// The boundary is the byte just past a newline, so the row starting here starts a
    /// physical line and the row BEFORE it ended at a break the file contains.
    pub from_newline: bool,
}

/// Where the next row boundary came from.
///
/// The label is what a row needs beyond the number: it decides whether the row ending
/// there `continues` (a break Cmdr made, which no copy path may turn into a newline)
/// and whether that row's text drops a trailing newline code unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextBoundary {
    /// Clause 2: the byte just past a newline. The row it closes ended at the file's
    /// own break, so its text stops one code unit short of it.
    LineStart(u64),
    /// Clause 3: a segment multiple, already snapped to a character start. The row it
    /// closes is one Cmdr ended itself, and keeps every byte as text.
    Segment(u64),
    /// EOF closes the last row.
    Eof(u64),
}

impl NextBoundary {
    pub fn offset(self) -> u64 {
        match self {
            Self::LineStart(b) | Self::Segment(b) | Self::Eof(b) => b,
        }
    }
}

/// The least row boundary greater than `boundary`, read forward.
///
/// ❗ The same boundary set as [`RowRuler`] approached from the other side, ❌ not a
/// second rule. `row_start` / `row_end` answer one probe with one bounded window,
/// which is what a seek wants and costs a two-segment read per row; a fetch walks
/// thousands of rows in sequence and reads the same boundaries out of the newline
/// stream it is already holding. `rows_test` asserts the two agree on every fixture at
/// every offset, so there is still only one definition to be wrong about.
///
/// `prev_newline` is the last newline code-unit START strictly below `boundary`,
/// `next_newline` the first at or above it. A caller may pass `None` for
/// `prev_newline` whenever it knows the segment below `boundary` holds no newline,
/// which is exactly what a clause-3 boundary proves.
///
/// `snap` applies clause 3's character snap, because the caller is the one holding the
/// bytes. A snapped multiple landing at or below `boundary` is discarded rather than
/// returned as a zero-length row, the same rejection `RowRuler::row_end` makes.
pub fn next_row_boundary<F: Fn(u64) -> u64>(
    boundary: u64,
    prev_newline: Option<u64>,
    next_newline: Option<u64>,
    newline_len: u64,
    segment: u64,
    total_bytes: u64,
    snap: F,
) -> NextBoundary {
    let mut best = NextBoundary::Eof(total_bytes);
    // Clause 3. Only two multiples can matter: the first above `boundary`, and the one
    // after it for when a newline inside the first one's segment disqualifies it.
    let first = boundary - boundary % segment + segment;
    for multiple in [first, first + segment] {
        if multiple > total_bytes || multiple >= best.offset() {
            break;
        }
        let evidence_start = multiple - segment;
        let clear =
            prev_newline.is_none_or(|unit| unit < evidence_start) && next_newline.is_none_or(|unit| unit >= multiple);
        if !clear {
            continue;
        }
        let snapped = snap(multiple);
        if snapped > boundary {
            best = NextBoundary::Segment(snapped);
            break;
        }
    }
    // Clause 2, which takes a tie: a boundary that is both a line start and a multiple
    // is the file's break, so the row ending there carries no marker.
    if let Some(unit) = next_newline {
        let line_start = unit + newline_len;
        if line_start > boundary && line_start <= best.offset() {
            best = NextBoundary::LineStart(line_start);
        }
    }
    best
}

/// One row's place in the file, before anything is decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowSpan {
    /// Absolute offset of the row's first byte.
    pub start: u64,
    /// Absolute offset of the next row's first byte: past the newline this row ended
    /// at, when it ended at one.
    pub end: u64,
    /// Absolute offset just past the row's last byte of TEXT: `end` less the newline
    /// code unit when the row ended at one, and `end` otherwise.
    pub text_end: u64,
    /// Cmdr ended this row at a segment boundary rather than at a newline or EOF.
    /// ❗ Nothing may join such a row to the next one with a newline.
    pub continues: bool,
    /// This row starts a physical line, so it is the one that prints a line number.
    pub starts_line: bool,
}

impl RowSpan {
    /// Bytes of text this row carries, newline excluded.
    pub fn text_bytes(&self) -> u64 {
        self.text_end - self.start
    }
}

/// How many bytes a [`RowReader`] pulls per refill. Big enough that a file of short
/// rows refills rarely; the compaction that precedes each refill moves every byte at
/// most once, so the walk stays linear whatever the row length.
const READ_CHUNK_BYTES: usize = 64 * 1024;

/// A forward walk over a file's rows.
///
/// Holds one bounded buffer (a refill chunk plus at most two segments), so walking a
/// 50 GB single-line file costs what walking a 50 KB one costs. ❗ Nothing here may
/// grow with a physical line's length: that is invariant I2, and a `memchr` "just to
/// find where this line ends" is how it gets lost.
pub struct RowReader<S: RowSource> {
    ruler: RowRuler<S>,
    encoding: FileEncoding,
    segment: u64,
    total_bytes: u64,
    /// The file's first content byte, past any BOM.
    ///
    /// The row GRID is absolute (`docs/specs/viewer-row-wrap.md`: a BOM must not shift
    /// one backend's rows against another's), but a file's first row starts where its
    /// text starts. That is what makes all three backends agree on row 0, and what
    /// stops ByteSeek handing the user a selectable `U+FEFF`.
    content_start: u64,
    buf: Vec<u8>,
    /// Absolute offset of `buf[0]`.
    buf_start: u64,
    /// Newline code-unit starts inside `buf`, ascending, those below the cursor
    /// already dropped.
    newlines: VecDeque<u64>,
    scanner: NewlineScanner,
    /// Absolute offset of the next row's first byte.
    cursor: u64,
    /// The last newline code unit strictly below `cursor`, where knowing it can change
    /// an answer. `None` also covers "the segment below the cursor holds no newline",
    /// which is what a clause-3 boundary proves.
    prev_newline: Option<u64>,
    /// The row at `cursor` starts a physical line.
    at_line_start: bool,
    /// `buf` reaches EOF; no refill can add to it.
    filled_to_eof: bool,
    /// The walk has produced the file's last row.
    finished: bool,
    /// Where in `buf` the row [`RowReader::next_span`] last returned starts, so its
    /// text can still be decoded until the following call.
    last_row_at: usize,
}

impl<S: RowSource> RowReader<S> {
    pub fn new(source: S, encoding: FileEncoding, content_start: u64) -> Self {
        Self::with_ruler(RowRuler::new(source, encoding), encoding, SEGMENT_BYTES, content_start)
    }

    /// The same walk on a smaller grid, for tests only. See [`RowRuler::with_segment`].
    #[cfg(test)]
    pub fn with_segment(source: S, encoding: FileEncoding, content_start: u64, segment: u64) -> Self {
        Self::with_ruler(
            RowRuler::with_segment(source, encoding, segment),
            encoding,
            segment,
            content_start,
        )
    }

    fn with_ruler(ruler: RowRuler<S>, encoding: FileEncoding, segment: u64, content_start: u64) -> Self {
        let total_bytes = ruler.total_bytes();
        let content_start = content_start.min(total_bytes);
        Self {
            ruler,
            encoding,
            segment,
            total_bytes,
            content_start,
            buf: Vec::new(),
            buf_start: content_start,
            newlines: VecDeque::new(),
            scanner: NewlineScanner::new(encoding, content_start),
            cursor: content_start,
            prev_newline: None,
            at_line_start: true,
            filled_to_eof: false,
            finished: false,
            last_row_at: 0,
        }
    }

    /// Whether [`RowReader::next_span`] would return `None`: the walk has no row left.
    ///
    /// ❗ ❌ Not `cursor == total_bytes`. On a file ending in a newline that is true one
    /// row BEFORE the end, because the final empty row starts at that same offset. A
    /// caller using the offset as its tell either stops a row early (losing the file's
    /// last newline) or, having served that row, asks once more and is handed it again.
    pub fn at_end(&self) -> bool {
        self.finished || (self.cursor >= self.total_bytes && !self.at_line_start)
    }

    /// Absolute offset of the next row's first byte.
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// How far past a newline its line starts. 2 for UTF-16: the newline is a two-byte
    /// code unit. Stating the rule in code units is what keeps a UTF-16 row from
    /// starting mid-unit; it isn't an optimization to fold away.
    fn newline_len(&self) -> u64 {
        match self.encoding {
            FileEncoding::Utf16Le | FileEncoding::Utf16Be => 2,
            _ => 1,
        }
    }

    /// Put the walk on the row containing `offset`, and return that row's start.
    ///
    /// One bounded [`RowRuler::row_start_detail`] window, whatever the file's size or
    /// its longest line: this is the seek half of invariant I1.
    pub fn seek(&mut self, offset: u64) -> Result<u64, ViewerError> {
        let probe = offset.clamp(self.content_start, self.total_bytes);
        let found = self.ruler.row_start_detail(probe)?;
        let start = found.offset.max(self.content_start);
        // A clause-3 boundary has a proven newline-free segment behind it, and the
        // file's first row has nothing behind it at all; in both cases no newline below
        // the cursor can change a later answer.
        let prev_newline = if found.from_newline && start > self.content_start {
            Some(start - self.newline_len())
        } else {
            None
        };
        let starts_line = found.from_newline || start == self.content_start;
        self.reposition(start, starts_line, prev_newline);
        Ok(start)
    }

    fn reposition(&mut self, start: u64, starts_line: bool, prev_newline: Option<u64>) {
        self.buf.clear();
        self.buf_start = start;
        self.newlines.clear();
        self.scanner = NewlineScanner::new(self.encoding, start);
        self.cursor = start;
        self.prev_newline = prev_newline;
        self.at_line_start = starts_line;
        self.filled_to_eof = false;
        self.finished = false;
        self.last_row_at = 0;
    }

    /// The next row, or `None` once the file's last one has been produced.
    ///
    /// The row's bytes stay in the buffer until the NEXT call, which is exactly how
    /// long [`RowReader::last_text`] stays valid.
    pub fn next_span(&mut self) -> Result<Option<RowSpan>, ViewerError> {
        if self.finished {
            return Ok(None);
        }
        let start = self.cursor;
        if start >= self.total_bytes {
            self.finished = true;
            // A file that ends with a newline has one more row after it: the empty one
            // the cursor is sitting on. An empty file has exactly that row and nothing
            // else. Both match what `FullLoadBackend` has always produced, and taking
            // that answer for all three backends is what makes a whole-file copy carry
            // the file's final newline whatever the file's size.
            if self.at_line_start {
                self.last_row_at = self.buf.len();
                return Ok(Some(RowSpan {
                    start,
                    end: start,
                    text_end: start,
                    continues: false,
                    starts_line: true,
                }));
            }
            return Ok(None);
        }

        // Two segments past the cursor is every byte any clause can need: the furthest
        // a boundary can land is `cursor + 2 × segment`.
        self.fill_to(start + 2 * self.segment)?;
        while self.newlines.front().is_some_and(|unit| *unit < start) {
            self.newlines.pop_front();
        }
        let next_newline = self.newlines.front().copied();

        let (buf, buf_start, encoding) = (&self.buf, self.buf_start, self.encoding);
        let boundary = next_row_boundary(
            start,
            self.prev_newline,
            next_newline,
            self.newline_len(),
            self.segment,
            self.total_bytes,
            |multiple| snap_back_to_char_start(buf, buf_start, multiple, encoding),
        );

        let end = boundary.offset();
        let (text_end, continues) = match boundary {
            NextBoundary::LineStart(b) => (b - self.newline_len(), false),
            NextBoundary::Segment(b) => (b, true),
            NextBoundary::Eof(b) => (b, false),
        };
        let span = RowSpan {
            start,
            end,
            text_end,
            continues,
            starts_line: self.at_line_start,
        };

        self.last_row_at = (start - self.buf_start) as usize;
        self.cursor = end;
        self.at_line_start = matches!(boundary, NextBoundary::LineStart(_));
        self.prev_newline = match boundary {
            NextBoundary::LineStart(b) => Some(b - self.newline_len()),
            _ => None,
        };
        Ok(Some(span))
    }

    /// Decoded text of the row [`RowReader::next_span`] last returned. Valid until the
    /// next call to it.
    pub fn last_text(&self, span: &RowSpan) -> String {
        let hi = (self.last_row_at + span.text_bytes() as usize).min(self.buf.len());
        decode_line(&self.buf[self.last_row_at.min(hi)..hi], self.encoding)
    }

    /// The next row with its text: what a fetch and a search both want.
    pub fn next_row(&mut self) -> Result<Option<(RowSpan, String)>, ViewerError> {
        let Some(span) = self.next_span()? else {
            return Ok(None);
        };
        let text = self.last_text(&span);
        Ok(Some((span, text)))
    }

    /// Make sure the buffer reaches `want` (or EOF), dropping what the walk has passed.
    ///
    /// Compaction happens only here, with the cursor on a row start, so a row the
    /// caller still holds is never moved out from under it.
    fn fill_to(&mut self, want: u64) -> Result<(), ViewerError> {
        let target = want.min(self.total_bytes);
        while !self.filled_to_eof && self.buf_start + self.buf.len() as u64 <= target {
            let passed = (self.cursor - self.buf_start) as usize;
            if passed > 0 {
                self.buf.drain(..passed);
                self.buf_start += passed as u64;
            }
            let read_at = self.buf_start + self.buf.len() as u64;
            let filled = self.buf.len();
            self.buf.resize(filled + READ_CHUNK_BYTES, 0);
            let got = self.ruler.read_into(read_at, &mut self.buf[filled..])?;
            self.buf.truncate(filled + got);
            if got == 0 {
                self.filled_to_eof = true;
                break;
            }
            let big_endian = matches!(self.encoding, FileEncoding::Utf16Be);
            let sink = &mut self.newlines;
            self.scanner.feed(&self.buf[filled..], |offset| {
                // `NewlineScanner` reports the `0x0A` BYTE; UTF-16 BE holds it in the
                // second half of the pair, and the rule counts unit starts.
                sink.push_back(if big_endian { offset - 1 } else { offset });
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// What a fetch hands back
// ---------------------------------------------------------------------------

/// The most text one `viewer_get_lines` answer may carry (2 MiB).
///
/// A fetch that hits it returns FEWER ROWS, never a shortened one: invariant I3 says
/// nothing is silently truncated. It keeps a wrap-on viewport from handing the
/// offscreen height measurer megabytes of text, and it is why a chunk has to SAY it
/// stopped early ([`ChunkEnd`]) rather than leave a caller guessing from the row count.
pub const CHUNK_BUDGET_BYTES: u64 = 2 * 1024 * 1024;

/// One row, as a backend serves it.
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ViewerRow {
    pub text: String,
    /// Absolute offset of the row's first byte.
    pub byte_offset: u64,
    /// Cmdr ended this row at a segment boundary, not at a newline the file contains.
    /// The frontend marks it; ❗ ❌ no copy, save, or search path may join it to the
    /// next row with a newline.
    pub continues: bool,
    /// The 0-based physical line this row starts, or `None` on a continuation row (the
    /// gutter prints nothing there, the usual editor convention). An estimate on
    /// `ByteSeekBackend`, which has no line index; exact on the other two.
    pub line_number: Option<usize>,
}

/// Why a chunk holds the rows it holds.
///
/// ❗ A caller walking a file chunk by chunk steers by THIS, ❌ never by "fewer rows
/// than I asked for": [`CHUNK_BUDGET_BYTES`] makes a short chunk ordinary, and reading
/// one as EOF silently truncates a copy or a save.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ChunkEnd {
    /// Every row asked for was served. More may follow at `end_byte_offset`.
    CountReached,
    /// The chunk reached [`CHUNK_BUDGET_BYTES`] first. More follow at
    /// `end_byte_offset`; ask again from there.
    BudgetReached,
    /// The chunk reached the end of the file. There is nothing past it.
    EndOfFile,
}

/// How many rows a file has, and whether that is a count or an estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "kind", content = "rows", rename_all = "camelCase")]
pub enum TotalRows {
    /// Counted: `FullLoadBackend` holds the file, or `LineIndexBackend` scanned it.
    Exact(usize),
    /// Derived from the file's size. `ByteSeekBackend` opens without a scan, so it
    /// divides by the bytes-per-row it sampled at open. That sample makes the number
    /// EXACT on a file with no newline in it (every row is a whole segment) and an
    /// estimate on anything else, exactly as its line count was an estimate before.
    Estimated(usize),
}

impl TotalRows {
    pub fn rows(self) -> usize {
        match self {
            Self::Exact(n) | Self::Estimated(n) => n,
        }
    }

    pub fn is_exact(self) -> bool {
        matches!(self, Self::Exact(_))
    }
}

/// What [`collect_rows`] produced.
pub struct CollectedRows {
    pub rows: Vec<ViewerRow>,
    pub end: ChunkEnd,
    /// Absolute offset just past the last row served. A caller asks for its next chunk
    /// from here; ❗ ❌ never from decoded string lengths, which are UTF-8 even when the
    /// file is not.
    pub end_byte_offset: u64,
}

/// Walk `reader` from wherever it sits, taking at most `count` rows and at most
/// [`CHUNK_BUDGET_BYTES`] of text.
///
/// `first_line` numbers the first row that starts a line; every line-starting row after
/// it takes the next number. `None` means the backend has no line numbers to give.
pub fn collect_rows<S: RowSource>(
    reader: &mut RowReader<S>,
    first_line: Option<usize>,
    count: usize,
) -> Result<CollectedRows, ViewerError> {
    let mut rows = Vec::new();
    let mut taken = 0u64;
    let mut end = ChunkEnd::CountReached;
    let mut end_byte_offset = reader.cursor();
    let mut next_line = first_line;

    while rows.len() < count {
        let Some((span, text)) = reader.next_row()? else {
            end = ChunkEnd::EndOfFile;
            break;
        };
        let line_number = if span.starts_line {
            let n = next_line;
            next_line = next_line.map(|n| n + 1);
            n
        } else {
            None
        };
        rows.push(ViewerRow {
            text,
            byte_offset: span.start,
            continues: span.continues,
            line_number,
        });
        taken += span.text_bytes();
        end_byte_offset = span.end;
        // Tested after the row lands, so a row is never cut in half to fit; a chunk
        // overshoots the budget by at most one row (under 40 KB) instead.
        if taken >= CHUNK_BUDGET_BYTES && rows.len() < count {
            end = ChunkEnd::BudgetReached;
            break;
        }
    }
    // ❗ A chunk that ran out of ROWS at the same moment it ran out of FILE is still at
    // the end, and has to say so. Reporting `CountReached` there sends the caller back
    // for one more chunk from `end_byte_offset`, and every streaming seek clamps that
    // onto the last row, so a copy or a save carries it twice (invariant I3).
    if reader.at_end() {
        end = ChunkEnd::EndOfFile;
    }
    Ok(CollectedRows {
        rows,
        end,
        end_byte_offset,
    })
}

/// Scan every row of a file with `matcher`, reporting matches by row and by column
/// within that row.
///
/// ❗ One implementation for both streaming backends. They used to carry a copy each of
/// the same `memchr(b'\n')` loop, which rebuilt `leftover + chunk` on every newline-free
/// chunk (about 1.4 TB of `memcpy` on a 300 MB line) and framed UTF-16 as if it were
/// ASCII, so ⌘F in a UTF-16 file found nothing at all. The walk is linear,
/// encoding-aware, and bounds a match's column by the row holding it.
///
/// Cancellation is checked per row and, inside `scan_line_with_matcher`, per match.
pub fn search_rows<S: RowSource>(
    reader: &mut RowReader<S>,
    matcher: &super::Matcher,
    cancel: &std::sync::atomic::AtomicBool,
    results: &std::sync::Mutex<Vec<super::SearchMatch>>,
    progress: &std::sync::Mutex<u64>,
) -> Result<u64, ViewerError> {
    use std::sync::atomic::Ordering;

    use crate::ignore_poison::IgnorePoison;

    use super::search_matcher::{LineScan, scan_line_with_matcher};

    let mut row_number = 0usize;
    let mut scanned = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some((span, text)) = reader.next_row()? else { break };
        match scan_line_with_matcher(matcher, &text, row_number, span.start, cancel, results) {
            LineScan::HitLimit | LineScan::Cancelled => {
                scanned = span.end;
                break;
            }
            LineScan::Done => {}
        }
        scanned = span.end;
        row_number += 1;
        // Progress per row would lock 15 000 times on the reported file; once a segment
        // is often enough for a progress bar and cheap enough to ignore.
        if row_number.is_multiple_of(64) {
            *progress.lock_ignore_poison() = scanned;
        }
    }
    *progress.lock_ignore_poison() = scanned;
    Ok(scanned)
}
