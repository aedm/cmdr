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
  `temp_overwrite.rs`, `multipart_upload.rs` (parts, the sweep), `server_copy.rs`, `batch.rs` (tally, batch delete),
  `upload_body.rs`, `upload_ledger.rs`, `mutation.rs` (folders, delete, rename), `scan.rs`, `share_link.rs`, `paths.rs`,
  `errors.rs`, `state.rs` + `reconnect.rs`, `volume_impl.rs`, `testing.rs`.

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
  reads one piece ahead and holds its last piece for a Cancel check, and a cut-off PUT removes what a server kept of it,
  by its own `x-amz-meta-cmdr-write` token only. ❌ Don't collapse `fetched` and `handed`.
- ❗ **Conditional writes are an allowlist, ❌ never a probe**: Garage and VersityGW answer 200 to an ignored
  `If-None-Match` and overwrite. Elsewhere `CreateNew` HEADs first (again before Complete), and a HEAD after every write
  reports another writer's object as `AlreadyExists`.
- ❗ **An upload is recorded before its first part**, and an abort counts only once a listing confirms it; the sweep
  aborts ❌ only recorded uploads, ❌ never one in flight.
- ❗ **`delete` is one node** (`ENOTEMPTY` while keys sit under a folder). **`rename` moves one small file**; a folder
  or an object past the part floor is `RenameWork::CopyThenDelete`, which callers send through the engine.
- ❗ **An overwrite of an existing object off the `refuses_short_body` allowlist goes through a temp key**: VersityGW
  publishes a cut-off PUT, which would lose the original.
- ❗ **Server-side copy stays within one account**, matched on the concrete `S3Volume`, ❌ never a path; parts pinned to
  the source's ETag.
- ❗ **No checksum headers; equal-size parts, always** (R2). Streamed bodies sign `UNSIGNED-PAYLOAD`.
- ❌ **One unattended authentication attempt, never a loop.**
- ❗ **A share link is a credential**: `cmdr_fs::volume::ShareLink`, ❌ never logged, never across IPC.
- Every dependency was already in `Cargo.lock`. Check `cargo tree -d` before adding one.
