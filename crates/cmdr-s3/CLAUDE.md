# cmdr-s3

The S3 backend for AWS, R2, B2, Wasabi, Hetzner, and any other S3-compatible server: the protocol layer plus a `Volume`
per place (a bucket, or the account root that lists them) that lists, stats, and reads, but doesn't write yet. The plan:
`docs/specs/s3-support-plan.md`. Decisions and gotchas: `DETAILS.md`. Fixtures: `apps/desktop/test/s3-servers/`.

## Module map

- `sigv4.rs`, `encoding.rs`, `request.rs`, `ops.rs` (one builder per S3 call), `profile.rs` (preset → endpoint,
  addressing, conditional writes), `xml/`, `error.rs` (`S3Error`), `multipart.rs`, `metadata.rs`: pure values.
- `params.rs` (`S3ConnectionParams`, `S3Provider`, the store key), `refusal.rs` (`S3ConnectError` + the probe's table),
  `transport.rs` (`S3Client`, the only `reqwest` user).
- `volume/`: `mod.rs` (connect), `query.rs` + `listing.rs` (list and stat), `streams.rs` (ranged GET), `scan.rs`,
  `share_link.rs`, `paths.rs`, `errors.rs`, `state.rs` + `reconnect.rs`, `volume_impl.rs`, `testing.rs` (fixtures,
  `testing` feature).

## Must-knows

- ❌ **Never classify by `<Message>`.** `<Code>` plus the status; a bodyless answer (every HEAD) by status alone.
- ❗ **`reqwest` stays in `transport.rs`.** Everything else sends `S3Request`s and reads `Answer`s; a transport failure
  goes back to `map_transport_error` / `classify_connect_error`.
- ❗ **Every wire-touching delegator wraps itself in `noting`**, and every request goes out through
  `S3Client::exchange`: the operations are the liveness detector (`cmdr_fs::volume::liveness`, shared with WebDAV).
- ❗ **Every request costs the user money.** ❌ No HEAD per child, no watcher, no space poll, no index.
- ❗ **A wrong secret is ambiguous on some servers**: Garage answers `AccessDenied`, so only `SignatureDoesNotMatch` /
  `InvalidAccessKeyId` are `KeysRejected`. The probe runs `ListBuckets` first because only its body can tell.
- ❗ **No writes yet, and it says so**: `is_writable` is `false` until the write path works (conformance holds it to
  that). Reads are real: `supports_export` and `supports_streaming` are `true`.
- ❌ **No `.timeout()` on a GET, never a buffered body**: `S3Client::open` bounds only the headers; the body's budget is
  per chunk, and each chunk counts as heard. A 200 to a ranged GET is skipped locally.
- ❗ **Parse every success body**: Complete, CopyObject, UploadPartCopy, DeleteObjects can fail inside `200 OK`.
- ❗ **Keys are never trimmed**; listings use `encoding-type=url` and decode `+` as a space. A `.`/`..` segment is
  refused (`KeyError::DotSegment`).
- ❗ **Conditional writes are an allowlist, ❌ never a probe**: Garage and VersityGW answer 200 to an ignored
  `If-None-Match` and overwrite.
- ❗ **No checksum headers; equal-size parts, always** (R2). Streamed bodies sign `UNSIGNED-PAYLOAD`.
- ❌ **One unattended authentication attempt, never a loop**; the store is refreshed by an attended sign-in, never
  seeded.
- ❗ **A share link is a credential**: it travels as `cmdr_fs::volume::ShareLink` (no URL in `Debug`), and ❌ nothing
  logs it. The app writes it straight to the clipboard; it never crosses IPC.
- Every dependency was already in `Cargo.lock`. Check `cargo tree -d` before adding one.
