# cmdr-s3

The S3 backend for AWS, R2, B2, Wasabi, Hetzner, and any other S3-compatible server: the protocol layer plus a `Volume`
per place (a bucket, or the account root that lists them) that lists, stats, reads, and writes. The plan:
`docs/specs/s3-support-plan.md`. Decisions and gotchas: `DETAILS.md`. Fixtures: `apps/desktop/test/s3-servers/`.

## Module map

- `sigv4.rs`, `encoding.rs`, `request.rs`, `ops.rs` (one builder per S3 call), `profile.rs` (preset → endpoint,
  addressing, conditional writes), `xml/`, `error.rs` (`S3Error`), `multipart.rs`, `metadata.rs`: pure values.
- `params.rs`, `refusal.rs` (`S3ConnectError` + the probe's table), `transport.rs` (`S3Client`, the only `reqwest`
  user).
- `volume/`: `mod.rs` (connect), `query.rs` + `listing.rs` (list, stat), `streams.rs` (GET), `writes.rs` (PUT, verify),
  `multipart_upload.rs` (parts, the sweep), `upload_body.rs`, `upload_ledger.rs`, `mutation.rs` (folders, delete,
  rename), `scan.rs`, `share_link.rs`, `paths.rs`, `errors.rs`, `state.rs` + `reconnect.rs`, `volume_impl.rs`,
  `testing.rs`.

## Must-knows

- ❌ **Never classify by `<Message>`.** `<Code>` plus the status; a bodyless answer (every HEAD) by status alone.
- ❗ **`reqwest` stays in `transport.rs`**, and every request goes out through it, inside `noting`: the operations are
  the liveness detector.
- ❗ **Every request costs the user money.** ❌ No HEAD per child, no watcher, no space poll, no index.
- ❗ **A wrong secret is ambiguous**: Garage answers `AccessDenied`, so only `SignatureDoesNotMatch` /
  `InvalidAccessKeyId` are `KeysRejected`.
- ❌ **No `.timeout()` on a GET or an upload, never a buffered body** beyond one part. A 200 to a ranged GET is skipped
  locally.
- ❗ **Parse every success body**: Complete, CopyObject, UploadPartCopy, DeleteObjects can fail inside `200 OK`.
- ❗ **Keys are never trimmed**; a `.`/`..` segment is refused (`KeyError::DotSegment`).
- ❗ **Writes go to the final key** (`publishes_writes_whole`). ❌ Nothing partial is ever published: the streamed body
  reads one piece ahead and holds its last piece for a Cancel check. ❌ Don't collapse `fetched` and `handed`.
- ❗ **Conditional writes are an allowlist, ❌ never a probe**: Garage and VersityGW answer 200 to an ignored
  `If-None-Match` and overwrite. Elsewhere `CreateNew` HEADs first (again before Complete), and a HEAD after every write
  reports another writer's object as `AlreadyExists`.
- ❗ **An upload is recorded before its first part**; the sweep aborts ❌ only recorded uploads, ❌ never one in flight.
- ❗ **`delete` is one node** (`ENOTEMPTY` while keys sit under a folder); `rename` moves one small file. M6 replaces
  it.
- ❗ **No checksum headers; equal-size parts, always** (R2). Streamed bodies sign `UNSIGNED-PAYLOAD`.
- ❌ **One unattended authentication attempt, never a loop.**
- ❗ **A share link is a credential**: `cmdr_fs::volume::ShareLink`, ❌ never logged, never across IPC.
- Every dependency was already in `Cargo.lock`. Check `cargo tree -d` before adding one.
