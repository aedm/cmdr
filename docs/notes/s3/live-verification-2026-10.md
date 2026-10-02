# S3 live verification, 2026-10-02

Every `cmdr-s3` live cell run against all seven named presets, to check each provider's profile against what the real
service does. AWS, B2, and Wasabi had never been run before (their entries came from docs). The distilled, current
findings live in `crates/cmdr-s3/DETAILS.md` § "Verified providers"; this note keeps the per-cell evidence behind them.

## Setup

- **Runner**: `apps/desktop/test/s3-servers/live.sh`, variables from `live-env.sh` beside it (sops store through
  `secret`). Run from a private snapshot of the commit under test with its own `CARGO_TARGET_DIR`, because two sibling
  agents were editing the shared worktree at the same time.
- **Buckets** (kept; the campaign isn't over):
  - AWS: `cmdr-s3-test-58fb74`, `cmdr-s3-test-58fb74-2` (`eu-north-1`), `cmdr-s3-test-58fb74-usw2` (`us-west-2`), as the
    IAM user `claude-agent`.
  - B2: `cmdr-s3-test-58fb74`, `cmdr-s3-test-58fb74-2` (`eu-central-003`, private). Application key
    `cmdr-s3-test-58fb74` reaches both (`B2_S3_TEST_KEY_ID` / `B2_S3_TEST_SECRET_ACCESS_KEY`); key
    `cmdr-s3-test-58fb74-scoped` reaches `-2` only (`B2_S3_SCOPED_KEY_ID` / `B2_S3_SCOPED_SECRET_ACCESS_KEY`). The
    master key doesn't work on B2's S3 API.
  - Wasabi: `cmdr-s3-test-58fb74`, `cmdr-s3-test-58fb74-2` (`eu-central-1`, trial account, root key).
  - Hetzner: `cmdr-s3-test-58fb74`, `cmdr-s3-test-58fb74-2` (`nbg1`), replacing the deleted `cmdr-s3-test-d0e600`.
  - R2 `cmdr-s3-test`, GCS and Spaces from the secret store: one bucket each, unchanged.
- ❗ **HTTP/1.1 only**: `cmdr-s3` built alone (as `live.sh` builds it) has no `reqwest` `http2` feature, so these cells
  never see HTTP/2-only provider behavior. Inside the app build the feature is on (`genai` enables it) and the client
  negotiates HTTP/2: GCS reset every app request with `PROTOCOL_ERROR` because of an explicit `host` header beside
  `:authority`, which only the app-level live suite caught (fixed in 47a2bd97f).
- **Throughput is skewed**: two other live runners shared the uplink. The numbers below are for orientation only.
- **Cost**: well under the $0.50-per-provider budget. Each provider took roughly 1.3 GB of uploads plus server-side
  copies per full run; B2 and Wasabi ran a second time on three cells. Wasabi bills its ~1.5 GB for 90 days (trial).

## Outcome per provider

Every full run passed (20 cells per provider plus the sweep), apart from the two failures under "Bugs found".

### AWS (`eu-north-1`)

- Conditional writes: `If-None-Match` enforced (412, old object kept) on Put, Complete, and Copy; 200 on a free key;
  R2's copy header ignored. Matches the doc-based allowlist.
- Cut-off PUT: refused over an object (original kept) and on a free key (nothing published).
- Multipart: equal parts, larger last part, smaller last part, larger first part all land; a 4 MiB part before the last
  is `EntityTooSmall`.
- `UploadPartCopy`: ranged, whole, and a 1 MiB source as the last part all land; a wrong `x-amz-copy-source-if-match`
  is 412. Copies across buckets work (`CopyObject` and `UploadPartCopy`).
- `DeleteObjects`: three keys, 1,000 absent keys fine; 1,001 is `MalformedXML`.
- `ListMultipartUploads` lists an upload at once; abort 204, a second abort 204, a part after it `NoSuchUpload`.
- Awkward names round-trip; `encoding-type=url` echoed; NFD and NFC are two objects; `x-amz-meta-mtime` verbatim; a
  wrong crc32 is `BadDigest`.
- Flows: one PUT, `CreateNew` refused (`AlreadyExists`), overwrite, date kept, 11 and 12 MiB in 5 MiB parts, 70 MiB at
  the 64 MiB floor, copies (one request, 70 MiB in parts, 11 MiB with a tail), rename, folder move, seven-day share
  link, empty folder: all fine.
- Throughput: 64 MiB upload at 2 / 4 / 8 parts 22 / 24 / 24 MiB/s; 140 MiB copy at 4 / 8 / 16 in flight 0.96 / 0.63 /
  0.60 s, 0.92 s in 64 MiB parts.

### Backblaze B2 (`eu-central-003`)

- Conditional writes: `501 NotImplemented` on Put, Complete, and Copy, old object kept (check-then-write is right).
- Cut-off PUT: refused both ways, in two runs. Confirms the SO-based `refuses_short_body` entry live.
- ❗ `x-amz-copy-source-if-match`: a wrong pin is 412 and the current one copies (every part of the 70 MiB and 11 MiB
  copies carries it), in two runs. **Added to `enforces_copy_source_pin`.**
- Multipart: every shape lands but the 4 MiB part before the last (`EntityTooSmall`). `UploadPartCopy` every shape
  lands. Cross-bucket copies work.
- `DeleteObjects` of 1,001 keys is `InvalidRequest`. A second abort is `NoSuchUpload`.
- NFD and NFC two objects; wrong crc32 `BadDigest`; flows all fine.
- `ListBuckets` lists three buckets with the test key, one of them (`cmdr-s3-test`) outside its scope: the
  `listAllBucketNames` capability names every bucket.
- Throughput: upload 16 / 21 / 22 MiB/s; copy 5.8 / 3.1 / 2.5 s, 2.5 s in 64 MiB parts.

### Wasabi (`eu-central-1`)

- ❗ Conditional writes: `If-None-Match` IGNORED on all three (200 and overwritten). Check-then-write is right; this is
  the case the allowlist exists for.
- Cut-off PUT: refused both ways, in two runs. **Added to `refuses_short_body`.**
- `x-amz-copy-source-if-match` ignored (the part copied), in two runs: stays off the pin list.
- Multipart and `UploadPartCopy`: every shape lands but the 4 MiB part before the last. Cross-bucket copies work.
- `DeleteObjects` of 1,001 keys is accepted (200). The builder caps at 1,000 anyway.
- NFD and NFC two objects; wrong crc32 `BadDigest`; flows all fine.
- Throughput: upload 14 / 14 / 12 MiB/s; copy 1.31 / 0.86 / 0.61 s, 1.65 s in 64 MiB parts.

### Hetzner (`nbg1`)

- Same as the earlier campaign: `If-None-Match` enforced on Put only; pin ignored; cut-off PUT refused; any part sizes;
  `DeleteObjects` of 1,001 a bodyless 400; NFD and NFC two objects; wrong crc32 ignored.
- New: cross-bucket `CopyObject` and `UploadPartCopy` work between the two new buckets.
- "Other" view of Hetzner: an overwrite goes as one part; cancelled before completion, the original survives.
- Throughput: upload 28 / 23 / 25 MiB/s; copy 1.06 / 0.65 / 0.61 s.

### R2, GCS, Spaces

- Every finding matches `DETAILS.md` § "Verified providers" from the earlier campaign: R2 enforces Put / Complete / its
  copy header and the pin, refuses a larger last part, and stores keys NFC; GCS ignores every precondition, has no
  `UploadPartCopy`, and refuses `x-goog-if-generation-match` beside SigV4 headers (`ExcessHeaderValues`); Spaces
  enforces Put only and ignores the pin.
- Throughput: R2 upload 20 / 19 / 18 MiB/s, copy 4.2 / 2.5 / 1.8 s; GCS upload 8 / 10 / 16 MiB/s, copy 0.8 / 0.7 / 0.7
  s; Spaces upload 11 / 23 / 22 MiB/s, copy 0.7 / 0.5 / 0.4 s.

## Allowlist changes

- **B2 → `enforces_copy_source_pin`**: 412 on a stale pin AND a successful pinned multipart copy, in two separate runs.
  Effect: B2 multipart copies skip the HEAD before the completion.
- **Wasabi → `refuses_short_body`**: a cut-off PUT kept the original AND published nothing on a free key, in two
  separate runs. Effect: a Wasabi overwrite goes as one PUT instead of a one-part multipart upload. Only "Other" is off
  the list now.
- **No removals**: every allowlisted entry held on every provider. AWS's doc-based entries (all three conditional
  writes, the pin, short body) are now live-verified.
- `profile_test.rs` was red first for both, then green; `cost_test.rs` moved its off-list overwrite case to "Other".

## AWS region routing

`live_connect_test.rs::live_connect_aws_routes_each_bucket_to_its_region`, from an `eu-north-1` account root:

- The root lists all three buckets; `ListBuckets` names each region.
- The `us-west-2` bucket lists, takes a write, and reads it back.
- A fresh root whose first request is a stat of the far object (learned from the redirect) gets it right; so does one
  whose first request is a write (`HeadBucket` first).
- A share link to the far object fetches unsigned (200); a server-side copy from `eu-north-1` into `us-west-2` works.
- The `us-west-2` bucket opened as a place is `WrongRegion { region: Some("us-west-2") }`.

No user-visible error anywhere.

## Connect refusals

`live_connect_test.rs::live_connect_refusals`, after the 401 fix:

- **Wrong secret**: `KeysRejected` everywhere on both the bucket and the account root, except R2 (`AccessDenied` on the
  bucket, `BucketListRefused` on the root): R2 answers `ListBuckets` from its bucket-scoped key with `AccessDenied`
  whatever the secret, and the `HeadBucket` that follows has no body.
- **Wrong key id**: `KeysRejected` everywhere, both places.
- **Missing bucket**: `NoSuchBucket` everywhere but R2 (`AccessDenied`, its 403 for a bucket it doesn't know).
- **Bucket through another region's endpoint**: AWS `us-east-1` → `WrongRegion { eu-north-1 }`; Wasabi `us-east-1` →
  `WrongRegion { eu-central-1 }`; Hetzner `fsn1` → `NoSuchBucket`; Spaces `nyc3` → `NoSuchBucket`; B2 `us-west-004` →
  `KeysRejected` (B2 keys live in one region).
- **Account root through another region**: AWS and Wasabi connect; Hetzner connects and lists none; Spaces
  `BucketListRefused`; B2 `KeysRejected`.
- **B2 key scoped to `-2`**: opens `-2`, `AccessDenied` on the other bucket, `BucketListRefused` on the account root.

## Bugs found

- **Fixed, 8df7fb335**: an unknown key id on R2 answers `401` `<Code>Unauthorized</Code>` (bodyless on a HEAD), which
  the probe didn't know: a bucket place came out `NotAnS3Endpoint` and the root `Transport("Unauthorized (HTTP 401)")`.
  Any 401 is now `KeysRejected` at connect and `PermissionDenied` mid-session. TDD red first in `refusal_test.rs` and
  `errors_test.rs`; live-verified after.
- **Fixed, a78a84e65**: `live_cleanup_removes_every_leftover` swept all of `cmdr-live/`, and with three runners on R2's
  one bucket it deleted a running flow's objects mid-cell (copy `NotFound`, rename `NotFound`, share link 404). It now
  removes only objects and uploads older than three hours; the R2 flow passes alone and alongside.

## Open questions and recommendations

- ❗ **A Wasabi account root can't open a bucket in another region.** Reached through `us-east-1` it lists the
  `eu-central-1` buckets, but listing, writing, and deleting in one fail as `IoError` "PermanentRedirect (HTTP 301)".
  Wasabi answers exactly as AWS does (`301` with `x-amz-bucket-region` and `Location`,
  `400 AuthorizationHeaderMalformed` with `<Region>`; curl, 2026-10-02), so `routing.rs` would work unchanged. The fix:
  a per-provider regional endpoint in `ProviderProfile::reroute` (`s3.<region>.wasabisys.com`) and `route_each_bucket`
  gated on "the profile routes" rather than `ProviderKind::Aws`. Not changed here: `routing_is_for_aws_only` and its
  profile twin pin AWS-only as a deliberate decision. Small, a clear win.
- **R2's wrong secret reads as the ambiguous refusal.** A bucket place could tell it apart: `ListObjectsV2` with
  `max-keys=1` answers `SignatureDoesNotMatch` with a body on R2, where `HeadBucket` has none. One request more per R2
  connect. A tradeoff: better words for a typo vs a request.
- **Hetzner and Spaces answer a bucket in another location with `NoSuchBucket`**, and B2 another region with
  `KeysRejected`. True to what the servers say, but a user who picked the wrong location reads "no such bucket". A
  location hint in the words is a UX call.
- **Cost drift (reported to the sibling building the sent-vs-counted cell)**: off the pin allowlist, a multipart
  server-side copy sends one more `HeadObject` before completing (`server_copy.rs::source_unchanged`), but
  `Workload::copy_on_server` counts `2 + checks` either way.
- **A B2 key with `listAllBucketNames` lists buckets outside its scope**, so an account root shows a bucket it can't
  open. Unverified what opening it says (likely `AccessDenied`).
- **Still unverified**: a cross-bucket copy on R2, GCS, and Spaces (each key reaches one bucket).
