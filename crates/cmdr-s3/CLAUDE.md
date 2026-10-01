# cmdr-s3

The S3 backend for AWS, R2, B2, Wasabi, Hetzner, and any other S3-compatible server. Today it's the protocol layer only:
signing, request building, XML, typed errors, and provider profiles, all pure values with no network and no `Volume`
yet. The plan: `docs/specs/s3-support-plan.md`. Decisions and gotchas: `DETAILS.md`.

## Module map

- `sigv4.rs`: SigV4 header auth and share-link presigning. `encoding.rs`: S3's percent-encoding and key encoding.
- `request.rs`: `S3Request` (unsigned) and `SignedRequest`, as plain values. `ops.rs`: one builder per S3 call.
- `profile.rs`: preset → endpoint, region, addressing, conditional-write support, and the session downgrade.
- `xml/`: the element tree (`mod.rs`), response parsers (`parse.rs`), request bodies (`build.rs`). `error.rs`:
  `S3Error`.
- `multipart.rs`: the part plan. `metadata.rs`: rclone's `x-amz-meta-mtime` format.

## Must-knows

- ❌ **Never classify by `<Message>`.** `S3Error` comes from `<Code>` plus the status; a bodyless answer classifies by
  status alone.
- ❗ **Parse every success body.** Complete, CopyObject, UploadPartCopy, and DeleteObjects can fail inside `200 OK`;
  each parser checks the root and returns `BodyError::Embedded`.
- ❗ **Keys are never trimmed**, and a listing asks for `encoding-type=url` and decodes `+` as a space (AWS's spelling).
- ❗ **A key with a `.` or `..` segment is refused** (`KeyError::DotSegment`): every URL parser resolves it, so `a/../b`
  would hit `b`.
- ❗ **Conditional writes are an allowlist, ❌ never a probe**: Garage and VersityGW answer 200 to an ignored
  `If-None-Match` and overwrite. A no-overwrite write returns `Built { check_first }`; when it's `true`, HEAD first.
- ❗ **No checksum headers.** Wasabi rejects `CRC64NVME`; integrity is `Content-MD5` on DeleteObjects plus size and ETag
  checks.
- ❗ **Equal-size parts, always** (`plan_parts`): R2 refuses anything else at completion. A tail under 5 MiB folds into
  the part before it (Garage refuses a smaller `UploadPartCopy` source).
- ❗ **Streamed bodies sign `UNSIGNED-PAYLOAD`; in-memory ones sign their hash.** Header auth only; query auth is for
  share links, because a signed URL leaks into every log line that prints it.
- `reqwest` isn't a dependency yet. When the transport lands, confine it the way `crates/cmdr-webdav/CLAUDE.md` does.
- Every dependency was already in `Cargo.lock` at these versions and features. Check `cargo tree -d` before adding one.
