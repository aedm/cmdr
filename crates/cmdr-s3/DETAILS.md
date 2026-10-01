# cmdr-s3 details

Must-knows and the module map: `CLAUDE.md`. This file carries the decisions. Provider facts and their sources:
`docs/notes/s3/provider-research.md`; why there's no S3 library underneath:
`docs/notes/s3/library-and-fixture-audit.md`.

## Where the crate stands

Connect, browse, and read work: the transport, the connect probe, and a `Volume` that lists, stats, streams, and scans
for a copy, so S3 → anywhere copies run through the app's transfer engine. Writes and copies within an account are later
milestones of the plan, and their builders already sit in `ops.rs`, which is why `lib.rs` still carries a crate-wide
`allow(dead_code)`; it goes once writes call them.

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
lane through `package(cmdr-s3)`): every `integration_test.rs` and `read_test.rs` cell runs against BOTH fixtures,
because they disagree on a wrong secret; `conformance_test.rs` holds the promises a place that reads but doesn't write
can keep (`is_writable` and `supports_export` match what the methods do, `NotFound` names the path, the copy scan stops
when told and asks inside the walk); the app's
`file_system/write_operations/backend_suites/s3_transfer_integration_test.rs` copies off a bucket through the transfer
engine, byte for byte; `connection_drop_test.rs` cuts a `TcpProxy` in front of VersityGW, ❌ never the container.
Seeding goes through `volume::testing::seed`, this crate's own builders, because the volume doesn't write yet. The
1,005-key paging prefix (`cmdr-test-paging-1005/`) and the 65 MiB object (`cmdr-test-large-65mib/blob.bin`, `seed_once`)
are seeded once per fixture and kept; every other cell works under a `scratch_prefix` of its own, since the stack's
objects persist across runs.

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

## No-overwrite writes

`ops::put_object`, `complete_multipart_upload`, and `copy_object` take `Overwrite::{Replace, Refuse}` and return
`Built { request, check_first }`. `check_first` is `true` exactly when `Refuse` was asked and the profile has no header
for that operation right now; the caller HEADs the destination, then writes, and tells the user plainly if a clash shows
up afterwards. Making the flag part of the return type is what keeps the check from being forgotten.

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
IDs Cmdr recorded locally when it started each upload (the operation log is the natural home), or abort every upload
older than some age, which would also abort other tools' uploads.

## Metadata

The source file's mtime goes in `x-amz-meta-mtime` in rclone's format: Unix seconds as a fixed-point decimal with up to
nine fractional digits and trailing zeros dropped (`1354040105.123456789`), negative before the epoch. That's
`swift.TimeToFloatString` from `ncw/swift`'s `meta.go`, which rclone's S3 backend uses for its `mtime` key (read on
2026-10-01). `parse_mtime` reads it back the way rclone does, cutting a fraction past nine digits and padding a shorter
one, and refuses anything that isn't digits, one optional `-`, and one optional `.`.
