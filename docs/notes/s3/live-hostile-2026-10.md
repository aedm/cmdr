# S3 hostile live cells, 2026-10-02

A campaign to break `cmdr-s3` against the seven real provider accounts, at the `S3Volume` / `cmdr_fs` `Volume` level the
app drives. The cells: `crates/cmdr-s3/src/volume/live_hostile_test.rs` (names, folders, sizes, scale, share links,
metadata) and `live_hostile_failure_test.rs` (cancels, crash recovery, races), sharing `live_hostile_support.rs`. Run
them with `apps/desktop/test/s3-servers/live.sh <providers> live_hostile[_<cell>]`. Each cell collects every miss per
provider and fails once at the end. The distilled, current facts live in `crates/cmdr-s3/DETAILS.md`; this note keeps
the evidence.

## Setup

- **Accounts**: the `live-env.sh` set (R2, Hetzner `nbg1`, GCS, Spaces `fra1`, AWS `eu-north-1`, B2 `eu-central-003`,
  Wasabi `eu-central-1`). Two sibling agents ran live suites on the same accounts and uplink at the same time, so
  throughput numbers are for orientation only.
- **Snapshot runs**: siblings edited the shared worktree mid-run (twice it didn't compile), so later runs went from a
  `git archive HEAD` snapshot with its own `CARGO_TARGET_DIR` (`target/s3-probe/snap-run.sh`, not committed).
- **Cost**: per provider about 1.3 GB uploaded and 1.1 GB read back (the ~1 GiB object), plus ~~1,300 small requests.
  Egress is billed on AWS (~~$0.10) and GCS (~$0.13); the rest charge none at this size. Wasabi's big object was 300
  MiB, so its 90-day minimum bills well under a cent. Every cell cleans its own `cmdr-live/<run>/` prefix and the sweep
  found no stale leftovers.

## Bugs found and fixed

1. **A Cancel after a PUT's last piece destroyed the file it was overwriting** (`1b5c755f9`). Live on R2: the cancel
   cell's "one PUT, at its last piece, over an original" case reported `Cancelled` while the original was gone. Cause:
   `put_streamed`'s 200 ms tick still honoured a Cancel after the last piece was released, dropped the request while the
   server was committing, and the cut-off cleanup then found our own `x-amz-meta-cmdr-write` token on the published
   object and deleted it, so neither the original nor the new bytes survived (the fake reproduced exactly that). R2
   answers slowly enough to hit it; the other six answered within the tick and reported `Ok`. Fix: once the last piece
   is released, the PUT waits for its answer and reports the file it finished; an empty body checks Cancel once, before
   the request. Pinned by `late_cancel_test.rs` over a fake S3 that commits and then answers slowly (red: object gone,
   `Cancelled`).
2. **A server copy whose source shrank mid-copy surfaced as a raw `InvalidRange`** (`90f619970`). Live on Spaces, which
   ignores the copy's ETag pin: a 16 MiB source replaced by 1 MiB between parts failed the next `UploadPartCopy` with
   `416 InvalidRange`, an opaque `IoError`. Nothing was published and the upload was aborted either way; now it's
   `SourceChanged` like the pin's 412. Unit-tested in `server_copy_test.rs`.
3. **An object with a 1,024-byte key couldn't be deleted, renamed, or tallied on B2** (`865680063`). Every "what's under
   this?" listing asks for `<key>/`, one byte past S3's ceiling; B2 refuses that prefix with `400 InvalidRequest` where
   everyone else answers an empty page. Now such a prefix is answered as empty without a request
   (`listing::can_hold_keys`). Pinned by `long_key_test.rs` over `fake_s3.rs`, which refuses an overlong prefix like B2;
   verified live on B2 afterwards.

## Outcome per cell

"All six" means R2, Hetzner, GCS, Spaces, AWS, and Wasabi; B2 is listed where it differs. Every cell passed on every
provider after the fixes, except `live_hostile_sizes` on B2 (§ "Open questions").

- **`live_hostile_names_round_trip`**: 28 names (NFC and NFD `café`, emoji with a ZWJ sequence, Hebrew, Arabic, double
  spaces, leading space, trailing space, trailing dot, `...`, `100% sure`, a literal `%2e%2e` and `%20`, `+`, `#`, `?`,
  `&` `=` `;`, `\`, quotes, `<>|`, `~!*()$,@:`, `[]{}^` and a backtick, a tab, a line break, Japanese, `UPPER.TXT`
  beside `upper.txt`) plus a key of exactly 1,024 bytes, each through write, list, stat, read, rename, and delete. All
  passed everywhere, with two provider refusals: GCS refuses CR/LF in a key (raw PUT `400 InvalidObjectName`; through
  the volume a bodyless 400 from the no-overwrite HEAD, shown as `IoError "HTTP 400"`), and B2 refuses any control
  character, the tab too (`400 InvalidRequest`). Nothing lands in either case. R2 lists the NFD name as its NFC twin, as
  documented. A `..` segment resolves lexically inside the bucket (`RemoteRoot`), one climbing out of the bucket is
  refused, and a 1,025-byte key is refused by every server.
- **`live_hostile_folders`**: passed everywhere. A file beside its folder lists as `twin (file)`, reads as the file,
  renames alone, and the folder's delete refuses (`ENOTEMPTY`) without touching the file; a marker folder survives
  emptying; a folder under a file is `NotADirectory`; New File on a folder's name is `AlreadyExists`; a markerless
  20-level tree lists at both ends, tallies one file and 21 folders, is `CopyThenDelete` for the engine and
  `NotSupported` for `rename`, and vanishes with its last key.
- **`live_hostile_sizes`**: 0, 1, 5 MiB ± 1, 10 MiB ± 1, and 15 MiB + 1 (a one-byte last part) at a 5 MiB part floor,
  the 0 / floor + 1 / 2 × floor ones also of unknown length, each read back byte for byte and stat-sized; ranged reads
  across a part edge, from the last byte, and from the end (empty); a download released mid-way then read whole; then 1
  GiB (300 MiB on Wasabi) in production 64 MiB parts, generated and verified without holding it. Passed on all six.
  Big-object rates (shared link, skewed): R2 20.5 up / 33.4 down MiB/s, Spaces 15.8 / 46.3, AWS 25.7 / 48.1, GCS 15.4 /
  23.2, Wasabi 22.9 / 48.8, Hetzner 4.6 / 31.1 (Hetzner's upload overlapped a sibling's big run). B2: blocked by 403 on
  every request by the time this cell reached it (§ "Open questions"); its edge sizes were covered by the other cells'
  multipart and single-PUT paths.
- **`live_hostile_cancel_uploads`**: a Cancel before the first part, mid second part, and right before the completion of
  a 16 MiB multipart upload, and mid-body and at the last piece of a 2 MiB PUT, each to a free key and over an original.
  All cancelled cases left nothing published, the original byte for byte, no upload on the server, and no open ledger
  record, on all seven (after fix 1). The "at its last piece" case is too late by design: every provider now reports the
  finished file, read back whole.
- **`live_hostile_cancel_server_copies`**: a Cancel before the upload exists, before the first part, between parts,
  before the completion, and on a one-request copy. Passed on all seven: nothing published, the source intact, no upload
  left. GCS copies any size in one atomic `CopyObject`, so only the checkpoint before it can stop it; a later Cancel
  finishes the copy whole, as expected.
- **`live_hostile_crash_recovery`**: a child process (this test binary re-run) starts a paced 40 MiB upload under a
  state dir and is SIGKILLed after its record and a part landed; the server lists the upload (all seven, R2 included).
  The next connect with that state dir sweeps it: the upload is gone (a part sent to it is refused), nothing is
  published, the record is closed. An upload in flight in THIS process survives a second place's connect and an explicit
  sweep (0 aborted) and completes intact. Passed on all seven. FINDING (all seven): a second LIVE process sharing the
  record has its running upload aborted by the first one's sweep ("the server ended the upload before it completed"),
  because "in flight" is a per-process registry. Cmdr's `instance_lock.rs` allows one process per data dir, so the app
  can't reach this; documented in `DETAILS.md` § "Unfinished uploads".
- **`live_hostile_races`**: two `CreateNew`s on one key at once (one PUT, three rounds; in parts, one round): always
  exactly one winner whose bytes are stored, the loser `AlreadyExists`, on the header providers (R2, Hetzner, Spaces,
  AWS) every time. On check-then-write providers the documented blind window showed in one round of three: GCS, B2, and
  Wasabi each once reported BOTH writers `Ok`, the stored bytes being one writer's, so the other's write was silently
  replaced. A source replaced mid-copy is `SourceChanged` with nothing published on all seven (GCS: n/a, one atomic
  copy). Deleting a folder's listed files while another writer adds one keeps the new file; a file written while its
  folder is deleted survives; a rename onto a taken name is `AlreadyExists` with both kept, and forced replaces it.
- **`live_hostile_scale`**: 1,150 files plus five subfolders of ten. Passed on all seven: two pages, 1,150 file rows and
  five folder rows with the right sizes, `tally_subtree` 1,200 files and six folders, capped at 1,000 incomplete,
  `rename_work` `CopyThenDelete`, and `delete_files` of all 1,200 in two batches with no failure, the folder then
  `NotFound`. Seeding 1,200 PUTs at 16 in flight took 3 s (Spaces, Hetzner) to 34 s (B2).
- **`live_hostile_share_links`**: links to `hash# & plus+ 100% é 🦀 ?.txt`, `a b/c=d;e.txt`, and a Hebrew name fetch
  unsigned with the bytes intact everywhere. A three-second link works fresh and fails expired: 403 on most, ❗ 400 on
  GCS and 401 on B2. A deleted object's link answers 404 everywhere.
- **`live_hostile_metadata`**: passed everywhere. A stat shows the source's mtime (`x-amz-meta-mtime`) for an empty, a
  small, and a multipart object; a listing shows the upload time; a rename, a one-request copy, and a copy in parts keep
  the mtime; a copy of a dateless object takes the source's upload time; a dateless overwrite (one PUT, and in parts)
  drops the old mtime.

## Provider quirks seen

- **Refused names**: GCS, CR and LF (`InvalidObjectName`); B2, any control character (`InvalidRequest`).
- **An overlong listing prefix**: B2 `400 InvalidRequest`; everyone else an empty page.
- **An expired presigned link**: 403, except GCS 400 and B2 401.
- **A second abort of an aborted upload**: R2 and GCS answer a success, so a second abort proves nothing; probe with a
  part instead (`upload_gone`).
- **R2's `ListMultipartUploads` listed the killed child's upload** under its prefix, in every run of the crash cell.
  `DETAILS.md` § "Verified providers" says R2 lists nothing, not even a fresh upload; that may hold only for an unscoped
  listing.
- **A Cancel landing just after a PUT's body**: R2 answered the PUT later than the 200 ms progress tick, the others
  sooner; that difference is what exposed fix 1.

## Open questions for the lead

1. **B2 went 403 on every request** late in the campaign (HEAD, PUT, GET on `cmdr-s3-test-58fb74`), after everything had
   passed on it. Probably B2's daily free transaction cap (Class B/C at a $0 cap answers 403); asked `live-providers`.
   Rerun `live.sh b2 live_hostile_sizes` once it clears.
2. **A typed refusal for names a provider won't store?** GCS and B2 refusals surface as `IoError "HTTP 400"` /
   `"InvalidRequest (HTTP 400)"`. Mapping `InvalidObjectName` (and B2's control-character case) to
   `VolumeError::InvalidName` would let the UI say "this provider doesn't allow that name". Not done: `InvalidRequest`
   is B2's catch-all, so it can't be mapped by code alone, and GCS's refusal arrives as a bodyless 400 to the
   no-overwrite HEAD. Judgment call.
3. **The check-then-write blind window is not rare under a real race**: one round in three on GCS, B2, and Wasabi. It's
   the accepted residual risk (`DETAILS.md` § "No-overwrite writes"), and two Cmdr writers racing for one key is
   unusual, but if it matters, a post-write HEAD that also compares our write token (not just the ETag) wouldn't close
   it either; only versioning would.
4. **R2's multipart listing**: worth re-verifying the "lists nothing" claim (§ "Provider quirks seen") before relying on
   it in `abort_upload`'s R2 special case.
