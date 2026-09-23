# Data-safety hunt follow-ups

What the 2026-09-01 adversarial hunt over the transfer engines left open after its 15 findings were fixed. The fixes and
their reasoning live beside the code: `apps/desktop/src-tauri/src/file_system/write_operations/transfer/DETAILS.md`,
`transfer/volume/DETAILS.md`, and `transfer/move_op/` (all under
`apps/desktop/src-tauri/src/file_system/write_operations/`). Every item below was re-verified against the code on
2026-09-23 and is still open. Severity scale: **high** = loss or corruption under an unusual but plausible condition;
**medium** = wrong result, wedged state, or recoverable loss; **low** = hygiene.

## 1. A cross-FS move loses bytes written to a file after its copy finished

- **Problem**: a move to another disk, share, or device (`move_with_staging`) copies each file first and deletes the
  sources later, from a ledger of what it landed (`transfer/move_op/source_sweep.rs`, `SweptSource::landed_files`, a
  bare `PathBuf` list). A file rewritten in between (a log, a database, an app saving) is still on the ledger with its
  OLD bytes at the destination, so the sweep removes the source and the newer bytes are gone. Nothing compares the
  source at delete time against what was copied.
- **Impact**: high. Silent loss of the newest writes to a file in a folder being moved off-volume while something writes
  into it.
- **Solution**: snapshot size and mtime per file at copy time (the copy already stats the source; keep the pair beside
  the `landed_files` entry, not on the reversal ledger, whose identity rule is deliberately mtime-free). In the sweep, a
  file whose size or mtime no longer matches is kept, its source directory survives, and it rides out on the existing
  `AppearedDuringMove` channel, so the toast can say "2 items changed during the move and stay in Work". Red test: seed
  the cached scan, copy, rewrite one file, sweep; assert the source survives with its new bytes and the completion event
  counts it.
- **Size**: S (half a day). Clear win.

## 2. A top-level folder symlink on a volume merges through the link

- **Problem**: every recursive merge level treats a symlink as an opaque leaf, but the volume engine's TOP-level
  source-vs-destination type decision follows links: `transfer/volume/strategy.rs::resolve_source_is_directory` answers
  from the preflight hint or `Volume::is_directory`, and the `Volume` trait (`crates/cmdr-fs/src/volume/mod.rs`) has no
  symlink-aware answer. A symlink-to-dir the user selected, meeting a real directory of that name at the destination, is
  read as dir-vs-dir and merged, which lists the link's TARGET and renames its entries out.
- **Impact**: high. A folder outside the selection gets emptied into the destination, on an external volume or a share
  that exposes directory symlinks. Selecting a top-level item is the common shape, since users select what they see.
- **Solution**: give the `Volume` trait a symlink-aware type answer (an `entry_kind`, or `is_symlink` beside
  `is_directory`, defaulting to "not a link" for backends with no link concept); populate it in `LocalPosixVolume`, SMB,
  and SFTP from metadata they already fetch; thread it through `resolve_source_is_directory` and the preflight
  `source_hints`; and route a link-vs-dir top-level clash through `resolve_volume_conflict` as the type mismatch it is.
  Red test: the `transfer/volume/rename_merge_symlink_tests.rs` rig (a `LocalPosixVolume` over a `TempDir`) with the
  link as the top-level source.
- **Size**: M (one to two days). The trait change touches every backend and test double; the engine change is small.
  Clear win.

## 3. An SMB single-shot upload replaces a name another writer took mid-upload

- **Problem**: staged writes land by a no-replace rename, so a name someone else took while our bytes were on the wire
  is refused (`transfer/staged_write.rs`, `LandingName::ExpectedFree`). SMB files small enough for one compound frame
  skip staging (`WriteStaging::SingleShot`) and go out through smb2's `write_file_compound`, whose CREATE is
  `FileOverwriteIf`: a file that appeared at the name is REPLACED, even under Skip, and the copy reports success.
  Documented in `transfer/volume/DETAILS.md` § "The single-shot exemption" and pinned red, outside the lane, by
  `backend_suites/smb_transfer_safety_test.rs::smb_known_gap_a_single_shot_upload_replaces_a_name_taken_mid_upload`.
- **Impact**: medium. A silent replacement of someone else's file, but only inside one small file's upload window, and
  only on SMB.
- **Solution**: two parts. (a) In `smb2` (David's crate): an exclusive-create compound write (`FILE_CREATE`) that
  returns `STATUS_OBJECT_NAME_COLLISION` as a typed error. (b) In Cmdr: a way to tell `Volume::write_from_stream` the
  name must be new (a `WriteDisposition` of `CreateNew` vs `Overwrite`), mapped to `VolumeError::AlreadyExists`; a
  backend that can't express exclusivity falls back to staging for that item. Then un-ignore the pinned test.
- **Size**: M (one to two days). Roughly eight production backends plus every test double take the new parameter; the
  SMB disposition is a few lines once smb2 has it. Blocked on the smb2 API (a).

## 4. The volume engine's folder-over-file Overwrite deletes the file before the folder lands

- **Problem**: in `transfer/volume/conflict.rs::apply_volume_conflict_resolution`, the Overwrite arm for a cross-type
  clash deletes the destination file (`dest_volume.delete`), then the recursive copy creates the directory and fills it.
  The local engine renames the file aside instead (`DisplacedEntry`, kept until commit and restored on rollback via
  `ledger.rs::commit_keeping_displaced_aside`), and the same-volume move has its own aside
  (`transfer/volume/displaced_destination.rs`); this arm has neither. Only an Overwrite a person picked on a Stop prompt
  reaches it (blanket policies turn cross-type Overwrite into Skip).
- **Impact**: medium. A failure or cancel mid-subtree leaves neither the file nor a complete folder. The user agreed to
  replace the file and the source still exists, so the loss is the replaced file plus the wasted work.
- **Solution**: rename the file aside through `StagingTemp::mint_aside` (reuse `displaced_destination.rs` if its shape
  fits), record it in `CreatedPaths` as a displaced entry, discard it when the operation commits, and restore it on
  rollback or failure the way the local engine does (a ` (recovered)` sibling when the directory already took the name).
  `ResolvedConflict` grows an aside field that `merge.rs`, `strategy.rs`, and the write sites thread through.
- **Size**: S–M (about one day). Mostly plumbing through the volume-side ledger; the mechanism exists twice already.
  Clear win.

## 5. The local folder-over-file Stop prompt describes the clash as file-vs-file

- **Problem**: `transfer/copy/single_item.rs` hands the blocking FILE to `resolve_conflict` as both source and
  destination (it passes `IncomingItem::Directory` for the decision logic, but the event is built from the paths), so
  `build_conflict_event` in `write_operations/conflict.rs` renders `source_is_directory: false`: the dialog shows a
  file-vs-file prompt for a folder-replacing-a-file decision.
- **Impact**: medium. Refusing blanket cross-type Overwrite rests on "an explicit Stop answer is informed because the
  prompt shows both types"; this is the one prompt where it doesn't. Safe on failure (the file is kept aside), but the
  consent is uninformed.
- **Solution**: pass the real source directory as the prompt's source (its size from the drive index, as the volume
  prompt already does), so the dialog says folder-over-file and shows the folder's size; keep the destination as the
  blocking file. Add a gallery fixture so the prompt can be reviewed.
- **Size**: S (two hours plus the fixture). Clear win.

## 6. A merge leaf whose future is dropped keeps its 0-byte Rename placeholder

- **Problem**: a deep-merge Rename reserves `name (1).ext` with an `O_EXCL` 0-byte file, and `transfer/volume/merge.rs`
  (`copy_leaf`) takes it back when the leaf fails. A leaf whose FUTURE is dropped by the concurrent driver's
  cancel-drain deadline (`copy_concurrent.rs`) runs no failure path, so its placeholder stays.
- **Impact**: low. One 0-byte `file (1).ext` per unresolved clash in that window, indistinguishable from a real file.
- **Solution**: either a `Drop` guard on the reservation (take it back on drop unless committed, the shape `StagedWrite`
  uses for its temp), or have the driver's drain sweep placeholders the way it sweeps partials (`in_flight_partials`).
  Test by forcing the drain deadline.
- **Size**: S (half a day). Clear win.

## 7. Sequential archive extraction leaves Rename placeholders on cancel

- **Problem**: `transfer/volume/sequential_extract.rs::extract_sequential_subtree` resolves every conflict in a plan
  pass (reserving each clashing member's `name (1).ext`) and streams the bytes in a later pass. A cancel or read error
  between the two returns early and leaves every not-yet-streamed placeholder behind.
- **Impact**: low. 0-byte leftovers in the destination when extracting from a solid archive (`.tar.gz`, `.7z`) is
  cancelled mid-way.
- **Solution**: reserve at stream time instead (the plan only needs to know the name is free, which `ClaimedNames`
  answers in memory), or take back every unstreamed reservation on the early-return path.
- **Size**: S (half a day). Clear win.

## 8. A late name collision in the streaming deep merge fails the item instead of prompting

- **Problem**: `transfer/volume/rename_merge.rs::late_detected_collision` re-lists and runs the resolver (prompting
  under Stop) when a child's name turns out taken after all. The streaming merge in `transfer/volume/merge.rs` reports
  the same case as the item's failure (`DestinationExists`), because a merge leaf runs inside a `FuturesUnordered` with
  no resolver to prompt from. Reachable only by a file arriving between the level listing and the landing.
- **Impact**: low. Safe (nothing is replaced), but the two engines answer the same event differently, and the user gets
  a failed item where the same-volume path would have asked.
- **Solution**: hand the leaf a prompting seam (its `MergeCtx` already carries the `FileWindow`; add the resolver) so a
  late collision goes through `resolve_merge_child` like a listed one.
- **Size**: S (half a day); a modest refactor of the leaf's context. Tradeoff: more plumbing for a rare case.

## 9. Pin the concurrent driver's post-cancel cleanup end to end

- **Problem**: `transfer/volume/copy_concurrent.rs::drive_transfer_concurrent` returns a `ConcurrentOutcome`, so the
  post-loop (partial sweep, rollback, cancelled event) runs by construction, and `copy_concurrent_driver_tests.rs`
  asserts the outcome at that seam. No test observes the post-loop's effects after a resolver refusal.
- **Impact**: low. A coverage gap, not a defect: the type already makes skipping the post-loop unrepresentable.
- **Solution**: one cell in `transfer/volume/copy_cancel_tests.rs`, using `copy_wedge_test_support.rs` (a task parked
  mid-write) and `transfer/conflict_responder_test_support.rs::ConflictResponderSink` refusing the prompt, asserting the
  partial is swept and the cancelled event fires.
- **Size**: S (two hours). Clear win.

## 10. Run a second data-safety hunt over the subsystems the first one never reached

- **Problem**: the 2026-09-01 hunt (one reading agent per subsystem, a fixed finding schema with `file:line` and a
  verbatim excerpt, verification by a second reader following the cited lines) covered only the two transfer engines and
  the write-ops umbrella. Never reached: archive edits; delete, trash, and clipboard; the `cmdr-fs` `Volume` trait and
  `LocalPosixVolume`; SMB, SFTP, and MTP; the operation log; secrets and settings persistence; the file viewer; the
  slices of `cmdr-index`; git, downloads, listing, cloud actions, tags, and `open_with`.
- **Impact**: unknown by definition. The first hunt found 15 real findings in its scope, several high.
- **Solution**: repeat the same method per subsystem, one agent each, triaged into a follow-up list like this one.
- **Size**: L (a few agent-days plus fixes). Needs David's go-ahead on scope and timing.
