# What the reviewed bulk rename deliberately left

Ask Cmdr's natural-language bulk rename ships hardened: an exclusive local rename primitive, a dependency planner, live
overwrite and missing-source detection, extension and cycle warnings in review, a prompt contract that forces truncation
disclosure, and honest provenance in the operation log. The design lives beside the code:
`apps/desktop/src-tauri/src/agent/tools/propose/DETAILS.md`,
`apps/desktop/src-tauri/src/file_system/write_operations/DETAILS.md`, and
`apps/desktop/src-tauri/src/operation_log/DETAILS.md`. Two items are open, both waiting on a real report rather than on
effort.

## 1. One invented filename on a remote volume refuses the whole rename plan

**Problem**: a model can invent a filename. On a local volume that row stays reviewable and preflight blocks it as
`SourceMissing`, so the user sees a "(doesn't exist)" badge on exactly the row the agent got wrong. On a remote volume
the whole plan is refused at the proposal boundary instead. `missing_local_child` in
`apps/desktop/src-tauri/src/agent/tools/propose/rename/plan.rs` is where the split lives; its doc comment carries the
rule. The asymmetry exists because proposal construction is synchronous and must never touch a live mount (a dead mount
must not hang an agent turn; `agent/tools/propose/CLAUDE.md`), so for a remote path it can't tell an absent child from a
path escape.

**Impact**: over SMB, one hallucinated name costs 40 good rows, and the user gets a refusal instead of a fixable review.

**Solution**: either accept the asymmetry, or make proposal construction async and authoritative for remote paths (a
live probe, timeout-bounded, against exactly the mount the boundary refuses to touch today).

**Size**: M. Blocked on a real report (a user renaming over SMB and losing a plan to one invented name), then a David
decision.

## 2. Nothing checks a reply's coverage claim against what the tool returned

**Problem**: when `list_pane_files` truncates, the system prompt requires the reply to say it inspected `returned` of
`total` items and never to imply full coverage; `prompt_requires_exact_truncation_disclosure` in
`apps/desktop/src-tauri/src/agent/chat/system_prompt.rs` pins that. A model that ignores the rule isn't caught.

**Impact**: a user could believe every file was renamed when only the first page was.

**Solution**: the narrow, affordable version: refuse a rename plan whose row count equals `total` when the listing it
was built from came back `truncated`. ❌ Not the obvious version: judging whether a sentence claims full coverage means
reading the model's prose, which is classification by string matching and breaks on a paraphrase or a locale.

**Size**: S. Blocked on evidence that a model actually misstates coverage; the prompt rule has no counted signal yet, so
adding one is the first step.
