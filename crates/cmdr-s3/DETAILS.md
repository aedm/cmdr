# cmdr-s3 details

Must-knows and the module map: `CLAUDE.md`. This file carries the decisions. Provider facts and their sources:
`docs/notes/s3/provider-research.md`; why there's no S3 library underneath:
`docs/notes/s3/library-and-fixture-audit.md`.

## Where the crate stands

Connect, browse, read, and write work: the transport, the connect probe, and a `Volume` that lists, stats, streams,
scans for a copy, uploads (one PUT or in parts), makes folders, deletes one node, and renames one small file, so copies
onto, off, and between buckets run through the app's transfer engine. Server-side copy within an account and the "can't
rename in one call" capability are the plan's M6; their builders (`UploadPartCopy`, `DeleteObjects`) already sit in
`ops.rs`, which is why `lib.rs` still carries a crate-wide `allow(dead_code)`.

## The model: one volume per place

The ACCOUNT is the endpoint plus the access key id; it owns the secret (store service `s3+<scheme>://<host>:<port>`,
scoped by the key id, so every bucket under one key shares it). A PLACE is a bucket under it, or the account root, whose
children are the buckets (`apps/desktop/src/lib/servers/DETAILS.md` § "The model", bucket = place). Each place is its
own volume with its own id (`cmdr_fs::volume::s3_volume_id(host, port, key id, bucket)`), because a pin, a tab, and a
switcher row each key on a place.

- **App paths are the account's**, `s3://<key id>@<host>:<port>/<bucket>/<key>`, and a place's root hangs under that
  prefix (`/` or `/<bucket>`). So one object has one app spelling whichever place reached it, and a bucket place refuses
  another bucket's path (`RemoteRoot`'s containment check).
- **Two places of one account each hold their own client.** Sharing one is an optimisation for later; nothing depends on
  it.
- **The account root needs `ListBuckets`.** A bucket-scoped key (R2 non-admin tokens, B2 keys without
  `listAllBucketNames`) gets a typed `BucketListRefused` there, and its way in is a bucket place.

## Connecting

`connect_s3_volume` reads the secret from the `CredentialStore` (nothing stored is `NeedsCredentials`), builds an
`S3Client` (`user_agent("Cmdr")`, 10 s connect timeout, no `read_timeout`, redirects off, plus a pool-free twin for the
silence probe), and probes. On success it records the PII-free `s3_connected` with one property, `provider` (`aws`,
`r2`, `b2`, `wasabi`, `hetzner`, `other`).

**The probe is `ListBuckets` first, then `HeadBucket` for a bucket place.** `ListBuckets` goes first even for a bucket
because its error BODY is the only thing that can tell a wrong secret from a key without rights; a HEAD has no body. The
table (`refusal.rs`, one cell per row in `refusal_test.rs`):

- `ListBuckets` 2xx with a `ListAllMyBucketsResult`: the keys work; a bucket place goes on to `HeadBucket`.
- `SignatureDoesNotMatch` / `InvalidAccessKeyId`: `KeysRejected`, whatever the place.
- `RequestTimeTooSkewed`: `ClockSkewed` (this Mac's clock is off by more than 15 minutes).
- 5xx or a throttle: `Transport`.
- No S3 `<Error>` body (an HTML page, a bare 404): `NotAnS3Endpoint`.
- `AccessDenied` (or any other S3 error) on the account root: `BucketListRefused`; on a bucket place: fall back to
  `HeadBucket`, which a bucket-scoped key passes.
- `HeadBucket` 404: `NoSuchBucket`. 403: `AccessDenied` (can't tell a wrong key from no rights). A redirect, or any
  answer carrying `x-amz-bucket-region`: `WrongRegion { region }`.
- Transport failures: `TimedOut`, `CertificateUntrusted` (an `InvalidData` `io::Error` in the source chain), or
  `Unreachable`.

**Gotcha: Garage answers a wrong secret with `AccessDenied`** (VersityGW with `SignatureDoesNotMatch`; fixture README).
So `AccessDenied` is never `KeysRejected`, and the words for `BucketListRefused` and `AccessDenied` ask about both the
keys and the rights. `integration_test.rs` pins both servers' answers.

## Listing and stat

- **The account root**: `ListBuckets`, every page (AWS paginates past 10,000), buckets as folders carrying their
  creation date as `created_at`.
- **A folder**: `ListObjectsV2` with `prefix=<key>/` and `delimiter=/`, every page, `on_progress` after each page with
  the running tally (never per entry), cancel checked between pages. `listing.rs` turns a page into children:
  `CommonPrefixes` are folders; the folder's own marker (the key `<key>/` itself) is left out; a key with a `/` past the
  prefix (a child's marker, or a server that ignored the delimiter) names a folder, once; an empty, `.`, or `..` name is
  left out (unaddressable). ❗ **An object and a folder of one name keep the folder**: S3 allows `notes` beside
  `notes/…`, but one name in a pane is one path, and two entries on one path break everything keyed on it.
- **A missing folder is `NotFound`**: S3 has no folders, so a prefix with no keys at all (its marker included) doesn't
  exist, and a listing that saw nothing says so rather than showing an empty folder.
- **`get_metadata`**: the account root without a request; a bucket by `HeadBucket`; a key by `HeadObject`, and when that
  finds no object, one `ListObjectsV2` capped at one key under `<key>/` decides folder or `NotFound`.
- **Dates**: a HEAD's `x-amz-meta-mtime` (rclone's key and format, the source file's own mtime) wins over
  `Last-Modified` (the upload time). ❗ **A listing shows `LastModified`**: `ListObjectsV2` carries no user metadata,
  and a HEAD per child to fetch it would cost a request per file. So a file Cmdr or rclone uploaded shows its upload
  time in the pane and its own mtime in Get info. Decision: cost over consistency, because every request is billed.
- **Errors** (`src/volume/errors.rs`): not found (`NoSuchKey`, `NoSuchBucket`, a bodyless 404) is `NotFound(path)`; a
  refusal (`AccessDenied`, keys that stopped working, a bodyless 403) is `PermissionDenied { path }`; an archived object
  (`InvalidObjectState`) is `ColdStorage(path)`; `NotImplemented` / 405 is `NotSupported`; the rest is `IoError`
  carrying `<Code> (HTTP nnn)` for the logs.
- **Space**: `NotSupported`, and no poll interval. S3 has no capacity, and "bytes used" is a listing of every key.

## Reading

`streams.rs`, modelled on `crates/cmdr-webdav/src/volume/streams.rs`:

- **One GET per stream, pulled a chunk at a time** through `S3Client::open`, which returns the answer with its body
  still on the wire (`Opened`). ❌ No `.timeout()` on the request: only the headers wait is bounded (`QUERY_BUDGET`),
  and the body gets `REQUEST_BUDGET` of idle time per chunk, never a total, so a multi-GB download has no ceiling. Every
  chunk counts as `heard` for the silence watch. Peak memory per stream is one socket read.
- **A read from an offset asks `Range: bytes=<offset>-`.** A 206 names the full length in `Content-Range`, so a resumed
  stream's `total_size` stays the whole object. ❗ A 200 to a ranged GET means the server ignored `Range`; the stream
  skips `offset` bytes locally (`judge_get`, unit-tested). A 416 (at or past the end) is an empty read. Both fixtures
  answer 206 with the exact window (verified on VersityGW v1.8.0 and Garage v2.4.1, `read_test.rs`, 2026-10-01).
- **`read_range`** asks for exactly `[offset, offset + len)` and drops the response once the window is full, so a server
  that ignored the range doesn't stream the rest of the object. It backs remote-archive browsing.
- **A refused GET reads its `<Error>` body** (bounded by `REQUEST_BUDGET`) and goes through `map_s3_error`. The account
  root and a bucket's top answer `IsADirectory` without a request.
- **The copy scan** (`scan.rs`) hands `cmdr_fs::volume::scan_walk` the backend's own stat and listing: one listing per
  folder. A recursive `ListObjectsV2` (no delimiter, 1,000 keys per request whatever the nesting) would bill fewer
  requests for a deep tree; it's not done because the copy that follows lists each folder again anyway.

## Share links

"Copy share link" is `Volume::share_link` (`share_link.rs`): a presigned GET (`S3Client::share_link`, over
`ops::share_link`) for one key, signed offline with the account's keys, so it's free, instant, and sends nothing (❌ no
`noting`). The expiry is a `ShareLinkExpiry` (one hour, one day, or seven days, S3's SigV4 ceiling). The account root
and a bucket's top answer `IsADirectory`; a key that's really a prefix gets a link that answers 404, since telling the
two apart would cost a request and the UI only offers it on a file row. No live client means `DeviceDisconnected`: the
credentials live on the client. Both fixtures serve the link to a plain unsigned `reqwest::get` (`read_test.rs`,
verified on VersityGW v1.8.0 and Garage v2.4.1, 2026-10-01). The app's `copy_share_link` command writes it to the
clipboard in Rust and returns only the outcome, so the URL never reaches IPC or a frontend log.

## Connection state and reconnect

The WebDAV model, nearly line for line (`crates/cmdr-webdav/DETAILS.md` § "The reconnect model" and § "Silent or slow"
have the reasoning): `Connected | Disconnected | NeedsCredentials`, transitions only through `emit_if_changed`, the
first `DeviceDisconnected` flips the state once and starts a 2/5/15/30/60/120 s backoff when "reconnect automatically"
is on, one unattended probe out of the store, and a refusal (`KeysRejected`, `AccessDenied`, `BucketListRefused`)
latches `NeedsCredentials` until a person signs in. `reconnect_with_credentials(username, password)` takes the access
key id as the username and refuses any other key (`NotSupported`): another key is another account. `sign_in_prompt` is
`SignInShape::AccessKeys`. The silence watch is the shared `cmdr_fs::volume::liveness`, probing with an unsigned HEAD on
the endpoint through the pool-free client.

## Which side a test lives on

Unit cells: the refusal table, the listing rules, path splitting, the error map, the state machine, the switch, and how
a GET's answer is read (`streams_test.rs`), all without a server. Docker cells (`#[ignore]`d, run by the shared fixture
lane through `package(cmdr-s3)`): every `integration_test.rs`, `read_test.rs`, and `write_test.rs` cell runs against
BOTH fixtures, because they disagree on a wrong secret and on preconditions; `conformance_test.rs` runs every shared
`cmdr_fs::volume::conformance` assertion that applies (all but the unknown-length refusal, which is for backends that
can't take one, and the link one: S3 has no links), on the check-then-write path on both servers; `write_test.rs` also
proves the header path against VersityGW (`S3Volume::trust_conditional_writes`, testing only); the app's
`file_system/write_operations/backend_suites/s3_transfer_integration_test.rs` copies onto, off, and between buckets
through the transfer engine, byte for byte, plus the shared network scenarios (cancel, an answered Overwrite in place,
awkward names); `connection_drop_test.rs` cuts a `TcpProxy` in front of VersityGW, ❌ never the container. Seeding goes
through `volume::testing::seed`, this crate's own builders, so a cell about the write path never seeds through the code
it tests. Multipart cells cut 5 MiB parts (`S3Volume::set_part_floor`, testing only) except one per fixture at the
production 64 MiB. The 1,005-key paging prefix (`cmdr-test-paging-1005/`) and the 65 MiB object
(`cmdr-test-large-65mib/blob.bin`, `seed_once`) are seeded once per fixture and kept; every other cell works under a
`scratch_prefix` of its own, since the stack's objects persist across runs.

## The public surface is capped

Root re-exports: 7 items (`S3ConnectionParams`, `S3Provider`, `InvalidProvider`, `S3ConnectError`, `S3Volume`,
`UnattendedReconnect`, `connect_s3_volume`) plus `pub mod volume`, which the check counts as an eighth. Public modules:
1 (`volume`), plus `volume::testing` under the `testing` feature. `index-crate-isolation` pins it at exactly 8 / 1 / 8
(measured 2026-10-01): `cmdr-webdav`'s shape plus the provider preset the host maps its saved entry onto, and the
refusal for a preset that can't make an endpoint.

## Signing

- **Header auth for every API call.** `sigv4::sign` adds `x-amz-date`, `x-amz-content-sha256`, `Authorization`, and an
  explicit `host` (so the transport can't spell an IPv6 literal or a default port differently from what was signed). It
  signs every header in the request. The transport may add headers afterwards (`user-agent`, `content-length`); those go
  unsigned, which SigV4 allows.
- **The payload hash follows the body** (`PayloadHash::for_body`): `Body::Streamed` signs `UNSIGNED-PAYLOAD` so the
  bytes are read once; `Body::Bytes` (the XML bodies) and `Body::Empty` sign their real SHA-256. The spec said
  `UNSIGNED-PAYLOAD` everywhere. Hashing what's already in memory costs nothing, and it's what a strict server (or one
  on plain `http://`) expects.
- **Query auth only for share links** (`sigv4::presign`, `ops::share_link`): signs `host` alone, expiry 1 s to 604,800 s
  (seven days, S3's SigV4 ceiling). A signature in a URL ends up in every log that prints the URL, `reqwest::Error`'s
  `Display` included, so API calls never use it.
- **Encoding** (`encoding.rs`): RFC 3986 unreserved bytes pass, everything else is uppercase `%XX`, a space is `%20`.
  Keys are encoded per segment and only once (S3's rule; other AWS services encode twice), so the wire path IS the
  canonical path. Query pairs are sorted by encoded name then value; a valueless parameter is signed `uploads=` and sent
  bare (`?uploads`), the way the AWS SDKs do it.
- **Verified against AWS's published vectors** (`sigv4_test.rs`): GET Object, PUT Object, GET lifecycle, List Objects,
  and the presigned GET, byte for byte (cross-checked against `s3s-project/s3s` and `durch/rust-s3` on 2026-10-01; the
  AWS pages now redirect).

**Gotcha: a dot segment can't travel.** The `url` crate (and so `reqwest`) resolves `.` and `..` path segments, and
WHATWG counts `%2E%2E` as `..` too, so a key like `a/../b` would address `b`. `encode_key` refuses it with
`KeyError::DotSegment`; a delete must never land on a different object. Such keys exist only if another tool wrote them;
they can't be reached over HTTP through this stack.

## Providers

`ProviderProfile::from_preset` turns the connect form's preset into everything a request needs:

- **AWS**: `s3.<region>.amazonaws.com`, virtual-hosted. Put, Complete, and Copy all take `If-None-Match: *`.
- **R2**: `<account>.r2.cloudflarestorage.com`, region `auto`, path style. Put takes `If-None-Match`; Complete takes
  nothing; Copy takes `cf-copy-destination-if-none-match`. Keys composed NFC before they leave (`nfc_keys`), because R2
  stores them NFC and an NFD key would otherwise collide with its twin while our own comparisons said they differ.
  Jurisdictional endpoints (EU, FedRAMP) aren't offered yet.
- **B2**: `s3.<region>.backblazeb2.com`, path style. No conditional writes (501, per corroboration only).
- **Wasabi**: `s3.<region>.wasabisys.com`, path style (Wasabi's recommendation). Check-then-write (undocumented).
- **Hetzner**: `<location>.your-objectstorage.com`, region = location, path style. Check-then-write (undocumented).
  `cross_bucket_copy` is false: Hetzner's `CopyObject` works within one bucket only, and the builders refuse a
  cross-bucket copy with `BuildError::CrossBucketCopy` before sending it.
- **Other**: the given `http(s)://host[:port]` (nothing after it), region default `us-east-1`, the path-style toggle as
  given except that an IP endpoint is always path style. Check-then-write.

**Addressing.** Path style everywhere but AWS: one host for every bucket means one connection pool and one TLS
certificate. On AWS (where path style is deprecated, no date set) a bucket that isn't a plain DNS label (3–63 of
`a–z 0–9 -`, no dots) still goes by path: a dot breaks the `*.s3.<region>.amazonaws.com` wildcard certificate.

**Host parts are validated.** A region, location, or account ID must be `a–z 0–9 -`, so a typed `x.evil.com/` can't
redirect requests (and the signature) to another host.

**Conditional writes are an allowlist, ❌ never a probe.** A server can ignore `If-None-Match: *` and answer 200 while
overwriting: Garage does on Put, Complete, and Copy, VersityGW on Copy (`apps/desktop/test/s3-servers/README.md`,
observed 2026-10-01). A success proves nothing, so only an operation the provider documents enforcing carries a header
(AWS's three, R2's Put and its Copy header); every other cell is `CheckThenWrite`. The allowlist rests on provider docs
until M8 verifies each entry on a real account. A `501 NotImplemented` (`S3Error::is_not_implemented`) on an allowlisted
operation means the caller should call `ProviderProfile::downgrade(op)`: that one operation becomes check-then-write for
the session, and the first call logs. The cells are atomics because one profile serves every concurrent operation.

## Writing

**Every write goes to its final key, and the transfer engine stages nothing here.** S3 publishes an object only when its
PUT or `CompleteMultipartUpload` finishes, and a replaced object stays readable until then, so the volume answers
`publishes_writes_whole` and the engine writes final keys and takes a file→file Overwrite in place
(`apps/desktop/src-tauri/src/file_system/write_operations/transfer/volume/DETAILS.md` § "Whole-publish destinations").
Decision/Why: a `.cmdr-tmp-*` temp would cost a landing rename, which on S3 is a server-side copy plus a delete (twice
the requests, a single copy fails past 5 GB), and buys nothing the protocol doesn't already give. `write_is_single_shot`
stays `false`: a request is open while the source drains.

- **Shape** (`writes.rs::shape_for`): one streamed PUT when `plan_parts` gives a single part (so up to 69 MiB, a 64 MiB
  part plus a folded tail), a multipart upload otherwise. A stream of unknown length goes in parts of the floor size (64
  MiB, so at most 625 GiB), and one that ends inside its first part goes out as one PUT from the buffer.
- **The streamed PUT** sends `Content-Length` and reads one piece ahead (`upload_body.rs`), the WebDAV body's design
  with one more guard: a source that proves longer than promised fails the body BEFORE its last promised byte goes out,
  so S3 never stores a truncated prefix (WebDAV removes its truncated file afterwards; on S3 that file would already be
  the user's). ❗ The last piece also waits for a go-ahead from the upload, which asks the progress callback, where a
  Cancel arrives: with only the 200 ms tick, a cancel landing between ticks lost to a fast finish and published the
  object (found by the shared `a_cancelled_upload_leaves_nothing_behind` scenario). A cancel after the last piece went
  out is too late to stop the publish, and the write reports the file it finished.
- ❗ **A cut-off PUT is cleaned up after**, because not every server keeps S3's promise to publish nothing short of
  `Content-Length`: VersityGW stores whatever arrived before the connection dropped (fixture README). Every PUT carries
  a token of its own (`x-amz-meta-cmdr-write`, `metadata::write_token`), and a PUT that was cancelled or cut off HEADs
  its key (again after 150 and 300 ms, since the server stores the body only once it notices the drop) and deletes the
  object ONLY when it carries that token (`writes.rs::remove_cut_off_put`): anything else there is the original or
  another writer's. ❗ Residual risk on such a server: an in-place `CreateOrReplace` that's cut off has already lost the
  original to the server's truncated publish, and a crash mid-PUT leaves the truncated object with nothing to clean it.
  AWS, R2, B2, and Garage refuse a short body; which other providers keep the promise is for M8 to confirm. The token is
  visible as user metadata and harmless to other tools.
- **Multipart** (`multipart_upload.rs`): up to `UPLOAD_CONCURRENCY` (4) parts in flight, and a part is read from the
  source only when a slot is free, so at most four part buffers exist (256 MiB at the floor). Parts are buffered at all
  because a failed one is sent again after 1, 2, then 4 s on a throttle (`SlowDown`, 503, 429), a server fault, or a
  transport failure. The source is read between pieces with progress and cancel still answered, and ❌ a `next_chunk` is
  never dropped half-read. A known length is a promise: a part that comes up short, or bytes left after the last part,
  fail the upload. Cancel is checked once more right before `CompleteMultipartUpload`, which is what publishes.
- **Verification**: a HEAD after every write (`verify_landing`, `judge_landing`) compares the size and the ETag with
  what the write answered. It costs one cheap request per file and feeds the pane patch that follows (`take_written`),
  so `notify_mutation` doesn't pay a second one. ETags aren't compared with an MD5 of the bytes: under SSE-KMS and for
  multipart they aren't one.
- **Throttling on a single PUT isn't retried here**: its body is the source stream, which can't be read twice, and the
  engine's per-file retry runs only on transport errors. A typed "busy, try again" `VolumeError` the engine retries is a
  candidate for M8's friendly errors.

## No-overwrite writes

`ops::put_object`, `complete_multipart_upload`, and `copy_object` take `Overwrite::{Replace, Refuse}` and return
`Built { request, check_first }`. `check_first` is `true` exactly when `Refuse` was asked and the profile has no header
for that operation right now. Making the flag part of the return type is what keeps the check from being forgotten.

- **The header path** (AWS's Put and Complete, R2's Put): the server refuses atomically, 412 → `AlreadyExists`
  (`map_s3_error`, since the only precondition Cmdr sends is a no-overwrite one). A 501 downgrades that operation for
  the session; a refused Complete is sent again the other way (the parts are still there), a refused PUT fails that
  file.
- **Check-then-write** (everything else, both fixtures included): a HEAD before the write, again right before
  `CompleteMultipartUpload` (catching a writer that took the name during a long upload), and the HEAD after the write:
  another writer's ETag there under `CreateNew` is `AlreadyExists`, their object kept, ours gone. ❗ One window stays
  blind: a writer whose object lands between our last check and our own write's completion is overwritten by ours, and
  nothing short of bucket versioning can see it. That's the residual risk the product decision accepts ("tell the user
  plainly when we notice").
- **Garage ends an upload when another write replaces its object** (`NoSuchUpload` on the next part or the completion;
  `apps/desktop/test/s3-servers/README.md`). Under `CreateNew` that's read as the name being taken, after a HEAD
  confirms it, ❌ never as the destination "not found".
- **`create_file`** (New File) also refuses a name a FOLDER holds: an object beside a same-named prefix would hide under
  the folder in every listing. `write_from_stream` leaves that check to the engine's destination pre-check, which reads
  the listing anyway, to save a request per file.

## Unfinished uploads

S3 keeps an unfinished multipart upload's parts forever, invisible in every listing and billed. Cancel and every failure
abort it on the spot; what an abort can't reach (a crash, a dropped future, a server gone mid-abort) is swept later.

- **The record** (`upload_ledger.rs`): `<state dir>/unfinished-uploads` under `VolumeHost::state_dir("s3")`, one line
  per event (`+` when `CreateMultipartUpload` answers, `-` once completed or aborted), every field percent-encoded,
  rewritten to the open records whenever a sweep reads it. A process-wide registry marks uploads running in THIS
  process, and a guard marks a dropped upload abandoned. Without a state directory (a test host) the record lives for
  the session.
- **Decision/Why not the operation log**: the operation log is the durable journal of what happened to the USER's files,
  for undo and search, and its rows are paths a rollback can act on. An unfinished upload is protocol state that only
  this crate can act on (`AbortMultipartUpload`), keyed by an account and an upload ID; putting it there would mean a
  schema migration, a new row kind no rollback understands, and the app reaching into S3 vocabulary. A file this crate
  owns, in a directory the host hands every backend, keeps it where the knowledge is.
- **An abort is confirmed by listing** (`abort_upload`): a part request cut off just before an abort can land after it
  and bring the upload back (VersityGW does; AWS documents the race), so each round aborts and then lists the key's
  uploads, up to four rounds 200 ms apart, and only a listing without the upload ID forgets the record.
- **The sweep** (`S3VolumeInner::sweep_unfinished_uploads`) runs in the background at every connect and after a
  reconnect, and aborts the account's open records that no task in this process is running. A record the server confirms
  gone (aborted now or already) is forgotten; any other answer keeps it for the next connect. ❌ It never aborts an
  upload ID it didn't record, and never lists the server's uploads to decide: another tool's upload may be live.

## Folders, delete, and rename

`mutation.rs`. A folder is a prefix: it exists when it has a zero-byte `name/` marker OR any key under it, and ❗ a
folder wins over an object of the same name (`NameHolds`), the listing's rule.

- **`create_directory`** writes the marker, refusing a taken name (`AlreadyExists`), a missing parent (`NotFound`), and
  a FILE holding the parent's name (`NotADirectory`: a marker under it would turn that file into a folder in every
  listing). That's `mkdir`'s contract, so the shared `cmdr_fs::volume::mkdir_all` walk runs unchanged and refuses a file
  in the way at any depth; every level it creates gets a marker, so a `mkdir -p` folder survives emptying.
- **`delete`** reads one listing of `name/` capped at two keys (`listing::folder_contents`): anything but the marker is
  `ENOTEMPTY`, the marker alone deletes the marker, nothing at all falls back to a HEAD and deletes the object (or
  answers `NotFound`). A LIST per delete is a class-A request; the batch path (`DeleteObjects`, 1,000 keys) has no trait
  hook yet and arrives with M6's move engine.
- **`rename`** moves one file of up to 64 MiB (`RENAME_BY_COPY_LIMIT`): `CopyObject` keeping the metadata (so the mtime
  survives), a HEAD proving the copy is ours, then the source's delete, so the worst a failure leaves is two copies.
  `force: false` refuses a taken name first. A folder, a bigger file, or a cross-bucket copy on Hetzner answers
  `NotSupported`, so nothing triggers a copy per object by accident. ❗ M6 replaces this with the typed "can't rename in
  one call" capability and routing through the transfer engine.

## Responses

- **The element tree** (`xml/mod.rs`): bodies are small, so each is read into a tree first, matched by local name (AWS
  uses a default namespace, some servers none). Text is kept untrimmed because keys may begin or end with spaces;
  `Element::value` trims for numbers, dates, and tokens. Entities resolve through `GeneralRef` (quick-xml 0.41 splits
  them out of text nodes; `crates/cmdr-webdav/src/propfind.rs` has the evidence). A 32-level depth cap keeps a hostile
  body from building a tree whose recursive drop overflows the stack.
- **Every parser checks the root.** An `<Error>` root inside a 2xx is `BodyError::Embedded`, boxed to keep `Result`
  small. AWS documents this for Complete and CopyObject; UploadPartCopy and DeleteObjects get the same check because it
  costs nothing.
- **URL-encoded listings.** ListObjectsV2 and ListMultipartUploads ask for `encoding-type=url` (XML 1.0 can't carry some
  characters a key may hold), and the parser decodes only when the response echoes `<EncodingType>url</EncodingType>`.
  AWS writes a space as `+` and a plus as `%2B`, so `+` becomes a space before percent-decoding (botocore's
  `unquote_plus`). A server that encodes a plus as a literal `+` would break this; `integration_test.rs` lists keys
  holding both (`a b+c.txt`, `x + y/`) and both fixtures pass (verified on VersityGW v1.8.0 and Garage v2.4.1,
  2026-10-01).
- **Archived objects.** `StorageClass::is_archived` is true for `GLACIER` and `DEEP_ARCHIVE` (Glacier Instant Retrieval
  reads on demand, so it isn't). A listing child carries it as `FileEntry::in_cold_storage`, which the pane shows as an
  "archived" glyph; a stat also reads HEAD's `x-amz-storage-class` and `x-amz-archive-status` (`cold_from_head`), the
  only place Intelligent-Tiering's archive tiers show. A read of any of them answers `InvalidObjectState`, which
  `map_s3_error` turns into `VolumeError::ColdStorage(path)` by the code alone; the copy dialog, the listing error pane
  (an archived zip, browsed), and the viewer each word it from that typed variant. A restored object still lists as
  `GLACIER`, so it keeps the glyph and reads fine. The internals say "cold storage" because "archive" already means a
  zip here; the UI says "archived". Restore is a later milestone. Neither fixture has storage classes, so this is
  unit-tested only (`query_test.rs`, `errors_test.rs`).

## Request bodies

`CompleteMultipartUpload` carries explicit part numbers. `Delete` is always quiet (only failures come back) and carries
`Content-MD5`, which AWS still requires. Both escape CR and LF as `&#13;` / `&#10;`: a parser normalizes a literal CR or
CRLF to LF, so a key holding one would otherwise name a different object (AWS's object-key naming guide asks for exactly
this).

## Errors

`S3Error { status, code, message, request_id, region, endpoint }`. `code` is an `S3ErrorCode` variant for every code
some path acts on, `Other(String)` for the rest, and `NoBody` when there's no `<Error>` (HEAD, a proxy's HTML page). The
predicates (`is_not_found`, `is_precondition_failed`, `is_not_implemented`, `is_retryable`) use the code, falling back
to the status only for `NoBody` (and `Other`, for retryability). `region` (on `AuthorizationHeaderMalformed`) and
`endpoint` (on `PermanentRedirect`) are the routing hints a connect probe can use to find a bucket's real region.

## Multipart

`plan_parts(total)`: one part size for the whole upload, at least 64 MiB (fewer billed requests than S3's 5 MiB floor)
and at least `total / 10,000`, rounded up to a whole MiB. Past 10,000 × 5 GiB it's `TooLarge`. Equal parts are an R2
requirement (`InvalidPart` at completion otherwise); doing it everywhere also means a re-sent part covers the same
bytes. A tail under 5 MiB folds into the part before it: Garage refuses an `UploadPartCopy` source that small even as
the last part (fixture README). The fold is skipped when it would push that part past 5 GiB, which only happens near the
48.8 TiB ceiling. Whether R2 accepts a last part LARGER than the rest is for M8 to confirm; if it doesn't, the tail has
to split differently there. `PartPlan::range` gives each part's inclusive byte range for `x-amz-copy-source-range` or a
ranged read, the last one running to the end of the object.

**Spec correction: no marker can find our own unfinished uploads.** The plan says the startup sweep matches unfinished
uploads "by a Cmdr marker in the initiation metadata". `ListMultipartUploads` returns only key, upload ID, initiator,
storage class, and initiation time, and no API reads an in-progress upload's metadata. The sweep has to work from upload
IDs Cmdr recorded locally when it started each upload, or abort every upload older than some age, which would also abort
other tools' uploads. The first is what ships: § "Unfinished uploads".

## Metadata

The source file's mtime goes in `x-amz-meta-mtime` in rclone's format: Unix seconds as a fixed-point decimal with up to
nine fractional digits and trailing zeros dropped (`1354040105.123456789`), negative before the epoch. That's
`swift.TimeToFloatString` from `ncw/swift`'s `meta.go`, which rclone's S3 backend uses for its `mtime` key (read on
2026-10-01). `parse_mtime` reads it back the way rclone does, cutting a fraction past nine digits and padding a shorter
one, and refuses anything that isn't digits, one optional `-`, and one optional `.`.

A write takes the date from the source stream (`VolumeReadStream::modified_at`: a local file's `stat`, another S3
object's own `x-amz-meta-mtime` or `Last-Modified`) and sets it on the PUT or on `CreateMultipartUpload`, never on the
parts. A source with no date writes none. A rename copies the metadata with the object.
