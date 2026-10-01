# cmdr-s3 details

Must-knows and the module map: `CLAUDE.md`. This file carries the decisions. Provider facts and their sources:
`docs/notes/s3/provider-research.md`; why there's no S3 library underneath:
`docs/notes/s3/library-and-fixture-audit.md`.

## Where the crate stands

The protocol layer exists; the transport, the `Volume`, and the app wiring don't. Everything is `pub(crate)` and nothing
outside the tests calls it yet, so `lib.rs` carries a crate-wide `allow(dead_code)` that goes once `volume/` exists. The
crate has no `index-crate-isolation` surface ceiling yet for the same reason: it exposes nothing. It is in the guarded
list (no `tauri` in its tree) from day one.

## Signing

- **Header auth for every API call.** `sigv4::sign` adds `x-amz-date`, `x-amz-content-sha256`, `Authorization`, and an
  explicit `host` (so the transport can't spell an IPv6 literal or a default port differently from what was signed). It
  signs every header in the request. The transport may add headers afterwards (`user-agent`, `content-length`); those go
  unsigned, which SigV4 allows.
- **The payload hash follows the body** (`S3Request::payload_hash`): `Body::Streamed` signs `UNSIGNED-PAYLOAD` so the
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
  `unquote_plus`). A server that encodes a plus as a literal `+` would break this; the fixture suite should list a key
  holding both.
- **Archived objects.** `StorageClass::is_archived` is true for `GLACIER` and `DEEP_ARCHIVE`. Intelligent-Tiering's
  archive tiers don't show in a listing (only HEAD's `x-amz-archive-status` says so), so a read of one surfaces as
  `InvalidObjectState` instead.

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
