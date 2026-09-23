# SMB paths carry the server's own bytes

**Problem**: Cmdr composes every SMB path to NFC before it reaches the wire, so a file whose name the server stores in
NFD is visible in the pane, correctly sized, and impossible to open, copy, rename, or delete. Reported as `ERR-VETBX`
(app 0.46.1, macOS 27.0, 2026-09-22).

**Decision**: a path that came from the server keeps the server's exact bytes, end to end. A path that came from
anywhere else is resolved against a real listing once, at the moment a directory is opened, and everything derived from
that listing is exact thereafter. Two related defects ride along: a scan failure that names no path, and a write that
can plant a second entry indistinguishable from one already there.

Status: approved 2026-09-22. M0, M1, M2, and M3 done 2026-09-23 (byte-faithful paths and watcher keys; foreign paths
resolve at the pane's directory open, on the kernel-mount upgrade, and for files dragged in or pasted from Finder,
look-alikes refused; the cursor finds a look-alike name; a failed scan fails its operation with a typed, named error;
one shared fold key, `cmdr_fs::name_fold`); M4 and M5 not started.

## What is actually true about SMB names

An SMB filename is an opaque byte string. The server does no Unicode folding and no case folding on this share, so a
lookup succeeds only on an exact match. Measured against `naspi` over Tailscale with the `smb2` CLI (2026-09-22), on
`takeout-2026-09/extracted/Takeout/Google Photos/2026-04-25 Török Anna fotók @ Törökbálint/retusált -185.jpg`:

| directory spelling | filename spelling | server answer                  |
| ------------------ | ----------------- | ------------------------------ |
| NFC                | NFD               | 566,431 bytes                  |
| NFC                | NFC               | `STATUS_OBJECT_NAME_NOT_FOUND` |
| NFD                | NFD               | `STATUS_OBJECT_PATH_NOT_FOUND` |
| NFD                | NFC               | `STATUS_OBJECT_PATH_NOT_FOUND` |
| NFC                | `.JPG`            | `STATUS_OBJECT_NAME_NOT_FOUND` |
| `GOOGLE PHOTOS`    | NFD               | `STATUS_OBJECT_PATH_NOT_FOUND` |

So one directory legitimately holds its own name in NFC and its children's names in NFD, and case is significant too.
Any rule of the form "SMB servers use form X" is false; the only correct rule is "use the bytes the server gave you".

Scale on David's NAS, from a recursive listing of `Takeout/Google Photos` (497,120 entries, 2026-09-22): all 235
directory names are NFC or ASCII, 712 file names across six albums are NFD, and there are zero names that collide modulo
normalization and case. So listings never break and only leaf lookups do, and the wrong-file hazard below is latent
rather than live.

## Why it fails today

- `crates/cmdr-smb/src/volume/paths.rs:43,48`: `to_smb_path` runs `.nfc()` over the whole path. Every session-touching
  call goes through it, so every wire request is composed.
- `crates/cmdr-smb/src/volume/query.rs:26-27`: `list_directory_impl` builds the display path from the composed directory
  path and then appends each entry's raw server bytes. So every Cmdr path has NFC ancestors and an exact leaf, and
  `to_smb_path` re-composes the leaf on the way back out.
- The listing, the scan preview, and the size all come from that listing or from the `authoritative_listing` oracle, so
  they succeed. Only the `Create` goes to the server, which is why the file looks perfectly healthy until it is touched.
  `ERR-VETBX`'s log shows exactly this: `oracle hit for parent (327 cached entries)`, then `open_read_stream_with_hint`
  with an all-NFC path, then `STATUS_OBJECT_NAME_NOT_FOUND during Create`.

The macOS kernel mount fails the same way one layer down: it decomposes on `readdir` and recomposes on lookup, so Finder
cannot open these files either, and Cmdr's own local-filesystem path over `/Volumes/naspi` returned a plain `ENOENT`.
Commander One drops the NFD entries from the listing outright. Cmdr is the only one of the three that can already see
the files, which is why it is the only one that can be fixed here.

## The constraint this must not break

The `.nfc()` exists because a path that came from macOS is NFD while most servers store NFC, and without it accented
paths failed with `STATUS_OBJECT_PATH_NOT_FOUND`. That case is real and still has to work. Removing the fold without
putting a resolve at the seam where foreign paths enter would trade one broken share for another.

## The design

**1. Server-derived paths are never normalized.** Drop `.nfc()` from `to_smb_path`. A path that came out of a listing is
already exact in every component, so it needs no work and costs no round trip.

**2. Foreign paths resolve once, where a directory is opened.** Every foreign path becomes real by being listed: a typed
or pasted path, a restored tab or favorite, a drag-in from Finder, an MCP `nav_to_path`, and the pane path carried
across a kernel-mount to direct-connection upgrade all end in "show me this directory". So the resolve belongs on that
one seam, not scattered across the UI entry points that feed it.

On `STATUS_OBJECT_PATH_NOT_FOUND` or `STATUS_OBJECT_NAME_NOT_FOUND`, walk down from the deepest component that does
open, list each ambiguous component's parent, match under NFC-and-case folding, substitute the server's exact bytes, and
retry. Correct the pane's own path to the resolved spelling, so children inherit exact bytes. Cache the correction per
directory for the session.

The error code localizes the work: `NAME_NOT_FOUND` means the ancestors resolved and only the leaf is wrong, which is
one listing. Only `PATH_NOT_FOUND` walks.

**3. A direct action on a foreign leaf resolves against its parent.** Opening a file by a path nobody navigated to
(open-with, a drag, an MCP call naming a file) resolves the leaf against the parent's listing, preferring the
`authoritative_listing` oracle, which is free when a pane is already showing that directory.

**4. Ambiguity is never guessed.** When folding matches two entries, the operation fails and says so rather than picking
one. A rename, a delete, or an overwrite that picks the wrong twin is data loss, which principle one forbids.

### What stays folded, and why

- `SmbConnectionParams::new` (`crates/cmdr-smb/src/volume/mod.rs:554-558`): server and share names go to TreeConnect, a
  different namespace from the filesystem, and the fold is what keeps one share from connecting twice.
- `MountAnchor::new` (`crates/cmdr-smb/src/volume/mod.rs:154-158`): the anchor is compared against share-relative paths,
  so it has to be folded the same way the rest of the anchor machinery is. If step one makes share-relative paths
  byte-faithful, this fold has to move with them; settle it in M1, not by assumption.
- `smb_volume_id` (`crates/cmdr-fs/src/volume/ids.rs:181-184`): an identity key, deliberately lossy, so one share is one
  volume however it was spelled. Leave it alone.

### A third defect the same change exposes

`crates/cmdr-smb/src/volume/watcher.rs:51-57` builds its cache key as `mount_path` joined to the NFD fold of the
share-relative event path, while `list_directory_impl` keys the same directory in NFC, and
`ListingPath`/`find_listings_for_path_on_volume` (`apps/desktop/src-tauri/src/file_system/listing/cached_listing.rs:56`,
`apps/desktop/src-tauri/src/file_system/listing/caching.rs:78-103`) compare byte-exactly, because SMB does not override
`Volume::listing_path`. On the evidence that is a live miss for every accented directory on a direct SMB volume: an
outside change never invalidates the open pane. That is the `ERR-QW42X` / `ERR-46A6B` failure shape the `ListingPath`
doc comment already warns about, so it is data-safety work, not tidying. ❗ Confirm it with a test before building on
it; it is inferred from the code, not yet observed at runtime.

Once paths are byte-faithful, the watcher's fold goes away with the rest and both sides key on server bytes.

## Milestone plan

1. **M0, pin the behavior down.** A test against the Docker SMB fixture (`crates/cmdr-smb/src/volume/testing`) holding
   an NFC directory whose children are NFD, asserting that listing, read, copy, rename, and delete all work. A second
   test for the watcher-key mismatch above, to confirm or refute it before M4 assumes it. Red first.
2. **M1, byte-faithful paths.** Drop `.nfc()` from `to_smb_path`, settle the `MountAnchor` question, and fix the fallout
   in `paths_test.rs` and the anchored-mount suites. M0's read and copy tests go green; the foreign-path tests stay red.
3. **M2, the resolve.** Per-component resolution with the oracle first, a real listing second, folding under NFC and
   case, explicit on ambiguity, cached per directory for the session. Wire it to the open-a-directory seam and the
   direct-leaf seam. M0 goes fully green.
4. **M3, the scan error keeps its path.** `ScanOutcome::Error(String)`
   (`apps/desktop/src-tauri/src/file_system/write_operations/scan_cache.rs:60-67`) carries the path it failed on, and
   `scan_bridge.rs:114-124` stops constructing `IoError { path: String::new(), .. }`. This is what turned `ERR-VETBX`'s
   first attempt into `Copy error: Path: ; Error: No such file or directory`, a message that names nothing. Small and
   independent of M1 and M2.
5. **M4, the duplicate-on-write guard.** `Volume::scan_for_conflicts` and the source-side matcher
   (`apps/desktop/src-tauri/src/file_system/write_operations/conflict_preflight.rs:371-386`) match names byte-exactly
   through a `HashMap<&str, _>`. Add a second, folded pass: a destination entry that matches only under folding is a
   conflict of its own kind, surfaced as such, never silently written beside. M1 removes the mechanism that creates
   these twins; this is the guard for shares that already hold one.
6. **M5, the docs.** `crates/cmdr-smb/CLAUDE.md` and `DETAILS.md` both currently assert that SMB servers return NFC
   names, in the backend must-knows, the watcher section, and the anchored-mount section. Replace the claim with the
   rule, and evidence-anchor it.

M1 alone fixes David's 712 files. M2 is what makes a typed or restored path work, and is the reason to do this rather
than a retry ladder.

## Rejected alternatives

- **A normalization retry ladder** (try as-given, then all-NFC, then all-NFD). Cheap, roughly 4 ms per miss on a warm
  session, and it fixes the reported case. It cannot fix a foreign path: a path with _n_ accented components has 2ⁿ
  possible spellings on the server and the ladder tries three of them, none of them mixed, so an all-NFD path needing
  `(NFC directory, NFD leaf)` never resolves. It also cannot see case, and on a share holding twins it opens whichever
  spelling it reaches first, which for a delete or an overwrite is a wrong-file write.
- **Listing-resolve alone, with the fold left in place.** Correct, but it pays a miss on every accented path including
  the ones that were already exact, and its worst case is a real listing of a directory nobody has open. Measured on
  `naspi` over Tailscale (2026-09-22, NAS CPU busy with an Immich rescan): 0.4 s marginal for the 327-entry album and
  **17.2 s** for `Photos from 2016` at 58,440 entries. Making server-derived paths exact removes that cost from the
  common case entirely.

## Open questions

1. **Ambiguity copy.** What a folded-only conflict says in the transfer dialog, and what a two-way folded match says
   when an operation refuses. Both are user-facing strings, so they are David's to approve.
2. **Resolve cache lifetime.** Per session, or invalidated by the directory's own watcher events. The second is more
   correct and needs the M0 watcher question answered first.
3. **Whether a foreign path that resolves should rewrite what is persisted** (a saved tab, a favorite), so the cost is
   paid once ever rather than once per launch.

## Evidence

- `ERR-VETBX`, the bundle and the unredacted local log for 2026-09-22 16:25:04 to 16:25:57.
- GitHub `vdavid/cmdr-reports` issue 23, which also carries the Finder and Commander One behavior.
- The `smb2` CLI matrices above, re-runnable per `~/Dropbox/obsidian/agents/tooling/smb2-cli.md`.
