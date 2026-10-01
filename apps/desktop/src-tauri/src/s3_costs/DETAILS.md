# S3 cost estimates: details

Must-knows: `CLAUDE.md`. The estimator, the table's schema, and the math: `crates/cmdr-s3/DETAILS.md` § "Cost
estimates". The product decision: `docs/specs/s3-support-plan.md` § "Product decisions".

## The flow

1. A setup dialog's scan preview settles. The S3 backend's walk kept every object's size and `LastModified`
   (`ScanSource::keeps_files`), which ride on `BatchScanResult::files` into the scan cache
   (`CachedScanResult::keeping_files`). A local walk's own per-file list gives sizes.
2. The dialog calls `estimate_operation_cost` with the preview id, the operation, and both volume ids.
3. `estimate` downcasts each end to `S3Volume`; neither is S3 means no estimate, before the cache is touched.
4. `plan` builds the workloads, `price_source::current` hands over the table, and `PriceTable::estimate` prices each.
   The line items go to the debug log (`RUST_LOG=cmdr_lib::s3_costs=debug`); the dialog gets one amount per provider.

## What each operation plans

- **Copy**: same account and buckets the provider copies between → `copy_on_server` per file on the one account.
  Otherwise a `download` per file on an S3 source and an `upload` per file on an S3 destination. Each copied folder
  writes a marker at an S3 destination (`upload(0)`).
- **Overwrites** (a copy's or a move's, `CostEstimateRequest.clashes`): the dialog sends the conflict check's file
  clashes and its policy; `plan::overwritten` decides which the policy overwrites the way the transfer does (strictly
  smaller, strictly older), and each one is `replace_object` at the destination plus, for an upload, `upload_over`. A
  server-side copy replaces in one request, so only `replace_object`.
- **Move**: the copy, then at the source a `delete_object` per file (with its date) and a `delete_folder` per folder.
  This is also F2's rename by move: the prefilled Move dialog runs the same scan. Before that, `estimate_rename` prices
  the rename editor's own tally (`Volume::tally_subtree`, at most 101 files) as a same-account move, and any amount that
  doesn't round to zero (`rounds_to_zero`, the cost line's half-a-cent rule) sends the rename to the Move dialog.
- **Delete**: a `delete_object` per file, a `delete_folder` and a `list_folder` per folder (a volume delete lists each
  folder again as it recurses).
- **No per-file list** (a scan answered from a cached listing): the bytes spread evenly over the file count, undated.
  Part counts drift a little; early deletion can't be priced, so it isn't.

## Price table refresh

**Decision**: one fetch per install per day at most, in the background, cached as the server's JSON verbatim in the
app data dir (`s3-prices.json`, temp file then rename). **Why**: prices move a few times a year, and the estimate must
never wait on the network. A refused table (a newer schema, a validation failure) or a failed fetch logs a warning and
keeps the table in hand. `LAST_ATTEMPT` keeps a failing server from being asked once per dialog.

**Decision**: dev, E2E, CI, and capture runs (`prod_instance::NON_PROD_ENV_VARS`) never fetch and price from the
bundled copy. **Why**: tests mustn't depend on the network, and every worktree's dev build would otherwise ask the
production server. The fetch itself is tested against a mock server (`price_source_tests.rs`).

There's no "stay offline" setting in the app today; if one lands, `fetching_allowed` is where it goes.

## Known gaps

- **Only the overwrites the dialog's conflict check saw are priced**: its one destination listing finds clashes at the
  top level, so a file inside a folder that merges isn't known until the operation writes it. Stop asks per clash, so
  no overwrite is assumed under it, and a skipped clash's copy is still counted.
- AWS prices are US East's for every region.
