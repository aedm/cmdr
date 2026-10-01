# The S3 fixture stack

Two S3 servers in Docker, chosen because they disagree where it matters: VersityGW behaves like AWS on conditional
writes, and Garage doesn't do them at all. Why these two and not MinIO, LocalStack, or SeaweedFS:
`docs/notes/s3/library-and-fixture-audit.md` § Docker S3 test servers. The plan they serve:
`docs/specs/s3-support-plan.md`.

```bash
./start.sh            # core: VersityGW and Garage
./start.sh minimal    # VersityGW alone
./stop.sh             # releases this shell's lease; downs only at zero holders
```

`pnpm check` brings the stack up on its own (`desktop-rust-integration-tests` declares `s3/core`), so a manual
`start.sh` is for iterating by hand. `start.sh` waits for each container's HEALTHCHECK, then sends one signed
ListBuckets from the host before it says "ready".

❌ **Never pause, stop, or kill a container to simulate a server going away**: other test binaries, worktrees, and
sessions lease the same stack at the same time. Put a `cmdr_fs::testing::tcp_proxy::TcpProxy` between the client and the
fixture and cut that, the way `crates/cmdr-webdav/src/volume/connection_drop_test.rs` does.

## The servers

- **`s3-fixture-versitygw`**, port 14480: `versity/versitygw:v1.8.0` over its POSIX backend. The default target. Honours
  `If-None-Match` and `If-Match` on writes.
- **`s3-fixture-garage`**, port 14481: `dxflrs/garage:v2.4.1`, single node. Ignores every write precondition, so it's
  the server that proves Cmdr never leans on no-clobber PUTs working everywhere.

Both answer path-style requests (`http://127.0.0.1:<port>/<bucket>/<key>`) in region `us-east-1`, and both start with
two empty buckets, `cmdr-test` and `cmdr-test-2` (two, so cross-bucket copy has somewhere to go).

**Credentials, the same pair on both**, so a cell switches server by port alone. They're fixtures and public on purpose:

- Access key: `GK00000000000000000000c0de`
- Secret key: `c0de` repeated 16 times (64 hex characters)

The pair is in Garage's own key format (`GK` + 24 hex, a 64-hex secret), because `garage key import` refuses anything
else; VersityGW takes any string. The single source is `docker-compose.yml`; `start.sh` repeats it for its probe.

## How they come up

- **First-party wrapper images**, one per server (`image-versitygw/`, `image-garage/`). Each runs the upstream binary as
  PID 1 and, beside it, an entrypoint step that creates the buckets: through the S3 API for VersityGW (a bucket made by
  `mkdir` lacks the ownership xattrs the gateway writes), through the `garage` CLI for Garage (layout assign and apply,
  key import, bucket create, bucket allow). Garage's official image is `FROM scratch` with no shell, so its wrapper
  copies the binary onto `alpine:3.24.2`.
- **Healthy means bootstrapped.** Each entrypoint writes `/tmp/fixture-ready` after its last bucket, and the HEALTHCHECK
  reads it. The lease adopts a stack on health, so a gateway that answered before its buckets existed would hand a cell
  a `NoSuchBucket`.
- **Idempotent.** Every bootstrap step checks first or tolerates a repeat (VersityGW answers 409 for a bucket it already
  owns), because `restart: unless-stopped` and the volumes mean the entrypoint runs again over state that's already
  there. Verified on 2026-10-01: a re-run adopts, a `restart` re-bootstraps to healthy, and `stop.sh` + `start.sh` comes
  back with the volumes intact.
- **Two named volumes**, `s3-fixture-versitygw-data` and `s3-fixture-garage-data`. ❗ Named, ❌ never a macOS bind mount
  for VersityGW: its POSIX backend keeps object metadata in xattrs. Named also means no anonymous volume per recreation
  (the WebDAV stack once leaked 41 GB that way). Objects persist across runs, so cells must not assume an empty bucket:
  give each cell a key prefix of its own. `docker compose -p s3-fixture down -v` wipes both, and the next bring-up
  recreates the buckets.

## Ports and binding

- **14480+, this stack's own range**: WebDAV owns 13480+, SFTP 12480+, SMB's vendored consumer stack 11480+, and
  `smb2`'s own harness 10480+. The check runner exports `S3_FIXTURE_<SERVICE>_PORT` from
  `scripts/check/checks/s3_ports.go`, and the compose defaults match it (`TestS3FixturePortsMatchComposeDefaults`).
- **Loopback only.** Every mapping carries a `${S3_BIND_ADDR:-127.0.0.1}` prefix, since a writable bucket with public
  credentials shouldn't land on the LAN or the tailnet. `TestS3FixturePortsBindToLoopback` fails the run if a mapping
  loses it. Set `S3_BIND_ADDR=0.0.0.0` to reach the fixtures from a NAT'd VM or a second machine.
- **Its own lease namespace**, `/tmp/cmdr-s3.lock` and `/tmp/cmdr-s3-leases`. The model: `scripts/check/DETAILS.md` §
  "Two fixture stacks, two lease namespaces".

## What each server answered

Observed by hand with `curl --aws-sigv4` and a small Go SigV4 signer against VersityGW v1.8.0 and Garage v2.4.1, on
2026-10-01. These are the evidence later milestones build on; re-check them when bumping either image.

**Conditional writes**, the reason there are two servers:

- `PutObject` with `If-None-Match: *` on an existing key: VersityGW answers **412** `PreconditionFailed` (with
  `<Condition>If-None-Match</Condition>`) and keeps the old bytes. Garage answers **200 and overwrites**: it ignores the
  header without a word.
- `PutObject` with `If-None-Match: *` on a new key: 200 on both.
- `PutObject` with a wrong `If-Match` ETag: VersityGW 412, Garage 200 and overwrites. With the right ETag: 200 on both.
- `CompleteMultipartUpload` with `If-None-Match: *` over an existing key: VersityGW 412 and keeps the old object; Garage
  200 and replaces it.
- `CopyObject` with `If-None-Match: *` over an existing key: **200 and overwrites on both**. So even VersityGW honours
  the precondition on PUT and multipart completion only, never on a server-side copy.

**Everything else:**

- ListBuckets, PUT, `GET` with `Range: bytes=5-9` (206 with `Content-Range: bytes 5-9/20`), `GET` with a range past the
  end (416), `HEAD` on a missing key (404), and a missing bucket (404 `NoSuchBucket`): the same on both.
- Multipart create, upload part, and complete: work on both. The completed ETag is `"<md5>-<part count>"` on both.
- `UploadPartCopy`, whole source and ranged (`x-amz-copy-source-range`): work on both with a source of 5 MiB or more. ❗
  **Garage refuses a copy source under 5 MiB even as the last part** (400 `InvalidRequest`, "Source object is too small
  (minimum part size is 5Mb)"); VersityGW accepts it, as AWS does for a last part. A multipart copy has to upload a
  short tail rather than copy it, at least on Garage.
- `CopyObject` across buckets (`cmdr-test` to `cmdr-test-2`): 200 on both.
- ❗ **A PUT cut off mid-body (the client closes the connection before `Content-Length`): VersityGW stores the bytes
  that arrived as the object**, under its name, which S3 never does; Garage keeps nothing. Deterministic on VersityGW
  v1.8.0 (`cmdr-s3`'s `write_test.rs::a_cancelled_single_put_publishes_nothing`, a source stalled after 1 MiB of 3:
  1,048,576 bytes stored), 2026-10-01.
- ❗ **An aborted multipart upload can come back on VersityGW** when an `UploadPart` cut off a moment before the abort
  lands after it: `ListMultipartUploads` shows the upload again (7 of 20 runs of `…a_cancel_mid_multipart…`,
  2026-10-01). AWS documents the same race and says to abort again until the parts are gone.
- A `PutObject` to a key while a multipart upload of that key is in flight: ❗ **Garage ends the upload** (the next
  `UploadPart` or `CompleteMultipartUpload` answers 404 `NoSuchUpload`); VersityGW keeps it, and completing it replaces
  the object just put. Observed through `cmdr-s3`'s `write_test.rs` (`…catches_a_writer_mid_upload`, both servers) on
  2026-10-01; `crates/cmdr-s3/DETAILS.md` § "No-overwrite writes" has what Cmdr does with it.
- `DeleteObjects` with `Content-MD5`: 200 on both, and a key that never existed comes back under `<Deleted>`, as on AWS.
  Garage also adds a `<VersionId>` and `<DeleteMarkerVersionId>` to each entry on an unversioned bucket.
- A presigned `GET` (query auth, `X-Amz-Expires=300`, `UNSIGNED-PAYLOAD`): 200 on both.
- A wrong secret: VersityGW answers 403 `SignatureDoesNotMatch`, Garage 403 `AccessDenied`. A refusal classifier can't
  rely on the code alone to tell "wrong secret" from "no permission".
- Garage's `CompleteMultipartUpload` `<Location>` reads `https://cmdr-test..s3.garage.localhost/...` (a doubled dot from
  its `root_domain`). Cosmetic; nothing should parse it.

❗ **macOS's curl 8.7.1 signs `x-amz-copy-source-range` wrong**: both servers reject the request with a signature
mismatch, while the same request signed by hand passes. It's a curl bug, not a server one, so don't trust a
`curl --aws-sigv4` failure on that header as evidence about a server.

## Adding a server

1. Add the service to `docker-compose.yml`, prefixed `s3-fixture-`, on the next free port, publishing it as
   `'${S3_BIND_ADDR:-127.0.0.1}:${S3_FIXTURE_<NAME>_PORT:-<port>}:<container port>'`, with a HEALTHCHECK that passes
   only once its buckets exist.
2. Add it to the right mode in `start.sh`'s case table **and** to `modeServices` on `stacklease.S3` in
   `scripts/check/stacklease/registry.go` (`TestS3ModeServicesAgree`). Map its container port in `start.sh`'s probe.
3. Add its port to `s3ServiceHostPorts` in `scripts/check/checks/s3_ports.go`, and its key to `s3CoreServices` if it's
   in `core`.
4. If it builds an image of its own, add the context to `buildContextsRel` on `stacklease.S3`, or an edit to it never
   reaches a running container.

## Adding a cell

The shared fixture lane selects every `#[ignore]`d test in `package(cmdr-s3)`, plus app-crate cells named
`s3_integration_*` (`laneFixtures` in `scripts/check/checks/fixture-lane-coverage.go`). Gate each cell with an
`#[ignore]` reason naming `s3-servers/start.sh` or `s3-fixture`, connect through `cmdr_s3::volume::testing`, and work
under a `scratch_prefix` of your own: the objects persist across runs. Seed with `testing::seed`, which goes through the
crate's own request builders. Run a cell against BOTH servers when what it asserts could differ between them; a wrong
secret already does (above). By hand: `./start.sh`, then `cargo nextest run -p cmdr-s3 --run-ignored only`.
