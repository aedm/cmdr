# What `inspect_file` still owes

Ask Cmdr's `inspect_file` ships as the tool that reads inside a file: up to 200 paths per call, a text window in any
encoding the viewer decodes, `find` across text and PDFs, PDF text by page with title and author, one level of an
archive's entries and any file inside one, and a photo's EXIF with GPS, with every cut visible in the row. The code is
`apps/desktop/src-tauri/src/agent/tools/read/inspect/`; the account is
`apps/desktop/src-tauri/src/agent/tools/DETAILS.md` § "Reading a file the way the viewer does", and the seams it rides
are in `apps/desktop/src-tauri/src/file_viewer/DETAILS.md` § "Headless reads" and
`apps/desktop/src-tauri/src/crash_reporter/DETAILS.md` § "The one exemption". Six items are open. Paths below are under
`apps/desktop/src-tauri/src/`.

## 1. `find` skips archive entry names

**Problem**: `find` applies to text and PDF rows only, so "which of these zips has a `README` inside?" is one call per
zip, each listing up to `MAX_ARCHIVE_ENTRIES` (200, `agent/tools/read/inspect/archive.rs`) entries that the model then
scans itself. There's also no paging past the first 200 entries, and no recursive name search.

**Impact**: wasted turns and tokens on archive questions, and a silent ceiling on big archives.

**Solution**: apply the shared `Matcher` to entry names at the top level (`ArchiveVolume::index()` in
`crates/cmdr-archive/src/volume.rs` already holds every name), plus a `pageOffset`-style paging of an archive's
children. Recursion needs a walk budget under the same per-path deadline.

**Size**: S for the top level; M with recursion and paging. Blocked on a real question that needs it.

## 2. Files on direct SMB, MTP, or SFTP volumes answer `unsupportedVolume`

**Problem**: the reader needs a local byte path (`std::fs`), so a path on a volume without `supports_local_fs_access()`
gets a typed `UnsupportedVolume` row (`agent/tools/read/inspect/mod.rs`) instead of being read through its `Volume`
stream. The file viewer has the same limit.

**Impact**: the agent can't read anything on a directly connected NAS share or phone.

**Solution**: a `Volume`-backed byte source in `file_viewer/headless.rs`. The agent inherits it the day the viewer gains
one; nothing to build in `inspect/` itself.

**Size**: L (it's the viewer's work). Blocked on the viewer growing a `Volume` seam.

## 3. Password-protected PDFs and archives stay closed

**Problem**: `textUnavailable: encrypted` and `unreadable { encrypted }` are the whole answer; the tool has no password
path. The viewer prompts the user; the agent can't, and a password typed into a chat would ride the transcript to the
provider.

**Impact**: the agent can't help with any protected file.

**Solution**: a design question before any code: where a password would be asked (a native prompt, never the chat), and
how it stays out of the thread and the provider's view.

**Size**: M. Blocked on a user asking about a protected file and finding the honest refusal insufficient; David decision
on the prompt surface.

## 4. Scanned PDFs have no OCR

**Problem**: a `hasTextLayer: false` row is the honest answer today (`agent/tools/read/inspect/pdf.rs`); `media_index`
skips PDFs, so there's no recognized text to hand over. The system prompt tells the model it's a scan, not an empty
document.

**Impact**: scanned invoices and letters, a common case in a Downloads folder, are opaque to the agent.

**Solution**: an OCR pass over rendered pages (Vision, as the media index uses for photos), which needs a PDF rasterizer
Cmdr doesn't ship.

**Size**: L. Blocked on demand, weighed against the rasterizer dependency.

## 5. An unsupported archive codec met at extract time reads as `corrupt`

**Problem**: detection and listing say `unsupported` for a codec the archive layer can't serve, but a file INSIDE such
an archive is refused at extraction, where the cause folds into `ViewerError::Archive { message }`
(`file_viewer/mod.rs`), which carries no kind. `agent/tools/read/inspect/mod.rs` maps every such error to
`UnreadableReason::Corrupt`.

**Impact**: the agent tells the user a healthy archive is damaged.

**Solution**: a typed kind on `ViewerError::Archive` (the archive layer already has `NotSupported`), mapped to
`Unsupported` in `inspect/`.

**Size**: S. Not blocked; waits on a report of a "damaged" archive that opens fine elsewhere, or on someone passing
through.

## 6. A photo's GPS location has no gate

**Problem**: `exif_facts` (`agent/tools/read/inspect/exif.rs`) always includes GPS coordinates when a photo has them.
The consent copy names it ("including where it was taken"), and that's the whole control.

**Impact**: a user who's fine sharing photo metadata but not their home coordinates has no way to say so.

**Solution**: a setting, or an on-request schema parameter (`exif: true`), so location stays out unless asked for. The
parameter is one branch in `exif_facts`; a setting also needs a settings row and copy. Either is a change to what
egresses, so check whether it needs a `CONSENT_COPY_VERSION` bump (`agent/DETAILS.md`, invariant 8).

**Size**: S. David decision: is consent-copy disclosure enough?
