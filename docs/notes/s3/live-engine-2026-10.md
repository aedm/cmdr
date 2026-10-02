# S3 transfer-engine flows on live providers, 2026-10-02

The app's transfer engine (copy, move, rename, delete, cancel, pause, rollback, conflicts) driven against the seven real
provider accounts through the same entry points the IPC commands call (`copy_between_volumes`, `start_rename_by_move`,
`start_renames`, `delete_files_start`, `cancel_write_operation`, `rollback_operation` + `execute_rollback`). The
distilled, current facts live in `crates/cmdr-s3/DETAILS.md`; this note keeps the evidence.

## Setup

- **One body, two servers.** Every S3 engine scenario takes a `cmdr_s3::volume::testing::S3Target` (a Docker fixture or
  a live account from `testing::live`). The Docker cells (`s3_integration_*` in `backend_suites/s3_transfer_*`,
  `s3_rename_integration_test.rs`, `s3_engine_integration_test.rs`) and the live cells (`s3_live_engine_test.rs`, run by
  `apps/desktop/test/s3-servers/live-engine.sh`) share them. S3 now also drives the shared semantics, safety, and
  move-drift scenarios, which only SFTP, SMB, and WebDAV ran before: 56 new fixture cells, all green on VersityGW and
  Garage.
- **Accounts**: the `live-env.sh` set (R2, Hetzner `nbg1`, GCS, Spaces `fra1`, AWS `eu-north-1`, B2 `eu-central-003`,
  Wasabi `eu-central-1`). Second buckets exist on AWS, B2, Wasabi, and Hetzner only. Two sibling agents ran live suites
  on the same accounts and uplink at the same time, so the timings are for orientation.
- **Cost**: per provider about 450 MB uploaded (the 140 MiB multipart copy, a 65 MiB download source, a 70 MiB cost-cell
  upload and its same-bucket copy, a 50 MiB cross-provider tree) and ~10,000 small requests, most of them from the
  1,005-object cells. AWS egress ~~200 MB (~~$0.02), Wasabi's 90-day minimum on ~0.5 GB well under a cent, Glacier and
  Deep Archive minimums on two 6-byte objects negligible. Every flow removed its own `cmdr-live/<run>/` prefix; one run
  killed by a timeout stranded 391 objects on GCS, removed by hand afterwards.

## Outcomes, flow by provider

"ok" means every assertion held, bytes checked by SHA-256 at both ends. Times are wall clock including seeding and
cleanup.

- **Byte path** (copy off, 65 MiB off, seeded tree off, onto with one PUT and a 140 MiB multipart carrying the mtime,
  tree onto, written tree off, cancel mid-upload, an Overwrite answer, a Skip answer in a pre-existing folder, awkward
  names both ways): ok on all seven. GCS was 0 of 10 before `47a2bd97f` (below); after it, 10 of 10, with one transient
  seeding failure in an earlier run.
- **Between buckets** (copy keeps the date, a cross-bucket copy runs on the server, a bucket-bound copy streams): ok on
  AWS, B2, Wasabi, and Hetzner. Skipped on R2, GCS, and Spaces: no second bucket. Hetzner copies across buckets on the
  server; the rename suite's comment naming it as the bucket-bound provider was wrong (Spaces is) and is fixed.
- **Renames through the engine** (a folder of 1,005 objects, a 17 MiB file by multipart copy, pause then cancel, pause
  then resume, a reviewed batch with a folder):
  - AWS, Spaces, Wasabi, GCS: ok.
  - R2: ok; the 1,005-object rename takes 168 s, past the suite's old 120 s wait (now stretched on live runs).
  - Hetzner: ok in five of six runs; once the 1,005-object rename failed with `DestinationExists` on a fresh destination
    key (`renamed/f0053.txt`). Open question 3.
  - B2: unverified, cap hit. Four of five failed with `PermissionDenied` on the source once the account's daily Class B
    cap was used up (open question 4); pause then cancel passed before the cap was reached.
- **Merges and moves** (merge under Skip, Overwrite, Rename (keep both), OverwriteSmaller; a move-merge onto that spares
  what it skipped; a folder moved onto and off; a file saved over or added mid-move off; same-bucket move-merge, folder
  move, and tree copy; a missing nested destination; a 6 MiB odd-length file; 40 files at full concurrency): 15 of 15 on
  R2, AWS, Spaces, Wasabi, Hetzner, and GCS (GCS's first run lost the network mid-cell, open question 6; the rerun was
  clean). B2: unverified, cap hit.
- **Safety, cancel, rollback, delete** (a failed merge copy or move onto the user's folder, a delete bound to a
  local-shaped preview, a recursive delete that takes exactly the selection, an unknown source type, cancel
  mid-download, cancel between multipart parts, a cut-off Overwrite keeps the original, pause and resume between parts,
  roll back a finished copy, cancel with rollback mid-tree, delete a folder of 1,005 objects):
  - R2, AWS, Spaces, Hetzner: 12 of 12.
  - Wasabi: 12 of 12 on the rerun; the first run's 1,005-object delete stopped at `f0530.txt` with `DeviceDisconnected`
    (one transport blip ends the whole delete, open question 1).
  - GCS: 11 of 11; the 1,005-object delete didn't finish inside the remaining seven minutes (614 objects gone).
  - B2: unverified, cap hit.
  - After every cancel: no object at the name, no unfinished upload on the server, no open record in the upload ledger.
    After every rollback: nothing left but the destination folder the copy itself made.
- **A name taken mid-upload** (informational, the documented blind window): another writer's file survived on R2, AWS,
  GCS, Spaces, and Hetzner. On Wasabi our upload replaced it silently with the copy reporting success: the
  check-then-write window `crates/cmdr-s3/DETAILS.md` § "No-overwrite writes" accepts. Whether Wasabi honours
  `If-None-Match` on PUT (which would close it) is for the profile owner to verify.
- **Between providers** (a 50 MiB file plus a nested tree, streamed): ok for R2 → GCS, GCS → Spaces, Spaces → AWS, AWS →
  Wasabi, Wasabi → R2, Hetzner → AWS, and AWS → Hetzner. B2: unverified, cap hit.
- **Archived objects** (AWS, `GLACIER` and `DEEP_ARCHIVE`, uploaded with the storage class): ok. The listing marks each
  `in_cold_storage`, a read answers `VolumeError::ColdStorage`, and a copy stops at once with
  `WriteOperationError::SourceInColdStorage`, nothing landing locally.
- **Requests against the estimate**: below.

## Bugs found and fixed

1. **Every GCS request failed in the app** (`47a2bd97f`). The app's `reqwest` speaks HTTP/2 (`genai` turns the feature
   on; `cmdr-s3` alone built HTTP/1.1, which is why the crate's own live cells never saw it), and `sigv4::sign` added an
   explicit `host` header beside HTTP/2's `:authority`. Google's front end resets such a stream with `PROTOCOL_ERROR`,
   so every connect answered `Unreachable`. Isolated with a plain `reqwest` probe in the same binary: a GET to
   `storage.googleapis.com` passed, the same GET with a `host` header failed the same way, and Cloudflare tolerated it
   (which is why R2 never showed it; R2 also offers HTTP/1.1 only). Fix: the host is signed as the request URL spells it
   and travels in the URL alone. Red first: `sigv4_test.rs::the_signed_request_carries_no_explicit_host_header` and
   `the_host_is_signed_as_the_url_spells_it`. The crate now declares `http2` itself (a sibling's `69b2330b5`).
2. **The estimate missed a HEAD on multipart server-side copies off the pin allowlist** (`6e9ad525e`, reported by
   live-providers from the tally). Hetzner, Spaces, Wasabi, and "Other" HEAD the source once more before completing
   (`source_unchanged`); `Workload::copy_on_server` now counts it. Red first:
   `cost_test.rs:: a_multipart_copy_off_the_pin_allowlist_heads_its_source_again`.
3. **Test infrastructure**: the gated source served the same bytes for every path (now per file, `gated_files`); live
   seeding at 32 concurrent PUTs drew `503 SlowDown` from Hetzner (now eight on a live account, a 503 sent again).

## Cost estimates

`s3_engine_integration_test.rs::requests_sent_against_the_estimate` counts every signed request by operation
(`testing::take_sent_requests`) for four operations on one bucket and compares it with `s3_costs::planned_workloads`,
the plan `estimate` prices. The fixture cells and the live cell assert the requests that move bytes and report the rest.

- **The write paths match exactly everywhere**: `PutObject`, `CreateMultipartUpload`, `UploadPart`,
  `CompleteMultipartUpload`, `CopyObject`, `UploadPartCopy`, and `GetObject`, on both fixtures and R2, AWS, Wasabi, GCS,
  Spaces, and Hetzner. GCS's same-bucket copy of a 70 MiB file is two `CopyObject`s, as estimated (no `UploadPartCopy`).
- **Not counted by the estimate** (the same shape on every provider; a folder of a 200 KB and a 70 MiB file):
  - Upload: 7 `ListObjectsV2` (estimated 0), and 2 HEADs more than estimated (1 on Wasabi).
  - Download: 3 `ListObjectsV2` and 1 `HeadObject` (estimated 0).
  - Same-bucket copy: 10 `ListObjectsV2` (estimated 0), 3 HEADs more (4 on Spaces and Hetzner).
  - These are the engine's own reads (the scan, the destination pre-check, the folder-creation checks), which the
    estimate counts only as `list_folder` for a tree it didn't scan. A LIST is a class A request at AWS (the price of a
    PUT), so for a tree of many small folders it's a real share of the bill.
- **The delete doesn't batch.** The estimate bills `DeleteObjects` 1,000 keys a request; the volume delete walker sends
  one `Volume::delete` per object, which on S3 is a capped LIST, a HEAD, and a DELETE. A folder of 1,005 objects on
  VersityGW: 1,009 `ListObjectsV2`, 1,007 `HeadObject`, 1,005 `DeleteObject` (3,021 requests, estimated about 5). Live
  wall clock for that delete: AWS 74 s, Spaces 119 s, Hetzner 139 s, Wasabi 155 s, R2 343 s, GCS about 11 minutes. The
  rename of the same folder, whose source sweep does batch (`delete_files`), takes 28 s on AWS.

## Open questions for the lead

1. **Batch the volume delete, or bill it per object?** Batching files through `Volume::delete_files` (S3's
   `DeleteObjects`) in the volume delete walker would make the estimate true, cut a 1,005-object delete from ~3,000
   requests to a handful, take minutes off R2 and GCS, and shrink the window in which one transport blip fails the whole
   delete (seen once on Wasabi). The alternative is teaching `Workload` the per-object shape. An engine change, so not
   made here.
2. **Count the engine's LISTs and HEADs in the estimate?** Or trim them: 7–10 LISTs for a two-file folder looks like
   more than the scan plus one pre-check needs.
3. **Hetzner's intermittent `DestinationExists` on a server-side copy** (once in six 1,005-object renames, not
   reproduced in five more with logging on). Two paths fit: a `CopyObject` that published and then failed to verify (a
   throttled or cut-off HEAD; Hetzner does answer `503 SlowDown` under load) makes `try_server_side_copy` fall back to
   streaming, whose `CreateNew` then refuses the name the copy itself took; or a transport error after Hetzner applied
   the copy. No data lost (the source stays), but the rename stops with a wrong reason. Candidates: retry throttled
   idempotent requests (HEAD, a 503'd `CopyObject`) in the backend, and have the fallback recognize its own landed copy.
4. **B2's daily Class B cap.** The 1,005-object cells used up the account's cap. From then on every B2 GET answered
   `403` with `<Code>AccessDenied</Code>` ("Cannot download file, download bandwidth or transaction (Class B) cap
   exceeded", read with the AWS CLI), and every HEAD a bodyless `403`. Cmdr maps both to `VolumeError::PermissionDenied`
   (a stat, a read, a delete's fallback HEAD); in a transfer that's `WriteOperationError::PermissionDenied` with
   `refusal: Unclassified` and `side: Source`, which the dialog words as a permission problem (a sibling's `16ee8a3b8`
   now adds that it may be a usage cap). LIST, PUT, and `DeleteObjects` kept working. B2's renames, merges, safety
   cells, and cross-provider copies are unverified until a rerun after the reset. Where the HEADs come from, a
   1,005-object folder rename counted on VersityGW (whose "Other" profile copies the way B2's does: check-then-write, no
   conditional copy): 1,005 `CopyObject`, 3,021 `HeadObject`, 19 LISTs, 2 `DeleteObjects`, 1 PUT from the engine; the
   test's own checks add 1 HEAD and 3 LISTs. Three HEADs per object, all in `server_copy.rs::copy_whole`: the source's,
   the no-overwrite `refuse_if_taken` on the destination, and `verify_landing`. `Workload` counts 3,015 for B2 (the plan
   3,017), so the estimate matches. A HEAD is Class B on B2, so this one rename exceeds B2's free 2,500 a day.
   Recommendation (not made): one LIST of the destination prefix up front in place of the per-object no-overwrite HEAD
   (the blind window grows from milliseconds per object to the whole operation), and each source's size, ETag, and
   `LastModified` from the scan's listing instead of a HEAD for a one- request `COPY`-directive copy, keeping the verify
   HEAD: about 1,000 HEADs in place of 3,000.
5. **Small-object throughput.** The 1,005-object rename: AWS 28 s, Spaces 49 s, Hetzner 52 s, Wasabi 109 s, R2 168 s,
   GCS 234 s; 40 small files at full concurrency: AWS 6 s, R2 52 s, GCS 64 s. Per-request latency from Stockholm
   dominates, so how many requests run at once per object matters more than bandwidth here.
6. **IPv6 without a route.** On this network (ULA addresses only, no global IPv6) GCS twice failed mid-run with "tcp
   connect error: Network is unreachable (os error 51)", every later connect in that process too, while `curl` falls
   back to IPv4 every time. Users with a half-configured IPv6 might see GCS drop out. Worth checking how `hyper-util`'s
   happy eyeballs handles an address list that fails instantly.
7. **The fixture lane got heavier.** The app crate now has 92 `s3_integration_` cells. At full parallelism under a load
   average of 9 every one of them hit the 8 s cap; at `-j 2` all pass in under 8 s each. The contention re-run clears
   that, but a test group or a thread cap for `s3_integration_` would keep the lane honest.
8. **Second buckets** on R2, GCS, and Spaces would let the cross-bucket flows run there too.
