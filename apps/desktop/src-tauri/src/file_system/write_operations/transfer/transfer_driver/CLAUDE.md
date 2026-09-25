# Shared transfer driver

The scaffolding all four transfer cores run through (local-FS copy plus the three volume ops), so the bulk-skip prelude,
the per-iter cancellation check, skip accounting, post-loop bookkeeping, and the paired progress + status updates exist
ONCE. Each operation supplies a `transfer_one` closure that does only the per-source work and reports a
`TransferOutcome`. `sync_driver.rs` serves local-FS, `async_driver.rs` the volume ops, `progress.rs` the per-file
callbacks. The transfers themselves: `../CLAUDE.md`.

## Must-knows

- **The whole point is that `transfer_one` is NEVER invoked** (1) for a source in the pre-known-conflicts bulk-skip set
  under `Skip`, (2) after a top-level conflict resolution returned Skip (async driver only), or (3) after cancellation
  is signaled. ❌ Never move a destructive call above those gates. The `transfer_driver_*_tests.rs` suites pin all
  three, so a violation is caught here rather than by inspecting four functions.
- **The cancellation check sits BEFORE any destructive call**, ❌ not after the closure returns.
- **EVERY leaf holds its OWN share of the in-flight total** (`progress.rs`: `LeafProgressLedger` per operation,
  `SourceProgress` per top-level source, `LeafProgress` per file). A directory streams many leaves AT ONCE through
  `volume/merge_ctx.rs::FileWindow`, so ❌ never a shared high-water slot: the next leaf to finish wipes it and the bar
  drops by everything the big one had streamed. Reported = `finished + sum(in-flight)`, under ONE lock.
- **A leaf's mark is its HIGH-WATER**, because an attempt restarts at byte zero; ❌ never lower it on a restart (the
  prefix gets credited twice). `complete` swaps that share for the leaf's exact size; `Drop` withdraws it if the leaf
  never landed.
- **The bars are leaf-granular against preflight LEAF totals**, so ❌ never reset the tally per inner file.
- **Every skip credits the bars AND calls `state.note_skipped`** (`SourceProgress::skip_leaf` does both), ❌ never one
  without the other. The bars must reach
  their totals; the rate must not see bytes nothing moved, or one big skipped file spikes the reported speed.
  `../DETAILS.md` § "Skipped work moves the bars, and stays out of the rate".
- **The three closure future shapes (`FetchFut` / `ResolveFut` / `TransferFut`) live HERE**, with the driver whose
  `where` clause they ARE; ❌ never copied into an operation's module. Three operations write those closures, and
  parking the aliases in one of them welded three `volume/` modules into a cycle.
- **Sync and async are deliberate siblings, ❌ not one generic driver.** Boxing futures for the sync caller would cost
  an allocation per source and lose the closure's `&mut` captures.
- **Conflict resolution is closure-owned for sync, driver-owned for async.** ❌ Don't unify without moving the sync
  closure's `&mut` state too.

Progress across a retry, and the sync/async and conflict-ownership splits: `DETAILS.md`. Read it before any non-trivial
work here: editing, planning, reorganizing, or advising.
