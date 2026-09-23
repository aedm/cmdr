# What the in-app agent still owes

The agent ships as Ask Cmdr (the chat rail) plus the proactive wake loop: the event pipeline, the durable proposal
spine, agent memory, `inspect_file`, consent, and cost metering. Its principles and its numbered decision log (D1–D60,
cited from code as `agent decision D49`) live in `apps/desktop/src-tauri/src/agent/DETAILS.md` § Principles and §
Decision log; each subsystem's account is in the colocated `CLAUDE.md` / `DETAILS.md` under
`apps/desktop/src-tauri/src/agent/`. The wake loop's own tuning leftovers are
`docs/specs/later/ai/wake-loop-follow-ups.md`; the importance scorer's are `docs/specs/later/importance-follow-ups.md`.

What follows is the designed-and-unbuilt part. Every item stands alone. Paths are under `apps/desktop/src-tauri/src/`
unless they say otherwise.

## 1. An activity log: what the agent decided, read, and wrote, with reasons

**Problem**: principle 5 ("every decision, proposal, and file read is visible with its reason") is the least-paid
principle in the subsystem. The only transparency is the rail's per-tool lines and the suggestions panel. The agent
already wakes on its own, proposes, reads file contents through `inspect_file`, and writes memory unprompted, and none
of that lands in one place the user can scroll.

**Impact**: trust. An agent that acts on its own initiative needs a full decision record before it earns more autonomy
(item 9 depends on this). It's also context input: a wake that sees recent rejections won't re-suggest what the user
just declined.

**Solution**: an `agent_log` table in `main.db` (`agent/store/`, a new forward migration): `ts`, `source` (wake / chat /
planner / summarizer / detector), `kind` (proposal / notify / memory_write / file_read / observation / error), `target`,
`rationale`, `model`, `tokens_in`, `tokens_out`, `latency_ms`. Write a row on every proposal, memory write
(`agent/memory/`), and `inspect_file` read (with the reason the model gave), plus quiet wakes. A paged IPC read and a
push event for new rows; a surface to show them (open question: the rail's own surface, or a native panel like the
transfer queue window, which is the precedent for a panel both could share). Retention: prune by age and row cap,
following the scaffold `agent/store/DETAILS.md` § "No auto-retention in v1" describes.

Keep the three terms apart so nobody builds a second recorder: the **operation log** (`operation-log.db`) is file
mutations; `agent_log` is the agent's **decisions** (its `kind = file_read` view is the "read log", not a separate
table); the **navigation-intent log** (item 10) is the user's navigation. A merged UI may be labeled "Activity", but
that's copy, not a table name. ❌ Never log a quiet wake's `reason` text or a file's content: `cmdr.log` ships in error
reports (`agent/wake/quiet.rs`).

**Size**: L. Not blocked; the surface choice wants a David decision.

## 2. The knowledge layer: folder summaries the agent can search

**Problem**: the agent's knowledge today is the drive index, importance, the operation log, live app state,
image-derived text, `inspect_file`, and its own memory. It has no compressed picture of what lives where, so "where do
we usually keep invoices?" means listing folders turn by turn, and a wake can't place a new file against what the rest
of the disk holds.

**Impact**: the largest unbuilt block, and the one that makes proposals land in the right folder. Also the only block
that spends the user's money before delivering anything, so it needs the preflight.

**Solution** (decisions D9–D16, D20, D52, D54 in the decision log):

- `folder_summaries` in `main.db`: `(volume_id, rel_path)` key, `summary`, `generated_at`, `model` (provenance only; no
  regeneration on a model switch), `listing_fingerprint` (proposed: a hash over child names, sizes, and mtimes), a
  `stale` flag. FTS5 over `summary` and `rel_path`; embeddings only if FTS disappoints.
- A summarizer job with a tiny context (listing in, `{summary, children_worth_descending}` out, no profile), fed from
  the drive index, never the filesystem. Deterministic pruning first (the importance scorer drops `node_modules` and
  friends; a threshold gates which folders get a call at all); the model refines depth only inside the ambiguous band,
  as a byproduct of a summary already paid for. Walk top-down in importance order; pack many small sibling folders into
  one call, never one call per folder.
- Two tiers: listing-only (the cheap bulk) and content-aware (10–100x the cost, for hot folders and on-demand asks). Hot
  folders (Downloads, Desktop, Documents, project roots) run first and in parallel with indexing.
- Resumable walk bookkeeping (`walk_state`), a concurrency-limited drip with retry and backoff (batch APIs were
  rejected, D13). Respect the FDA gate: enabling this without FDA must not stack TCC popups. Never materialize dataless
  cloud files (D52).
- A preflight before any spend: "I found ~N folders worth reading. Initial read with {model}: ~$X, roughly N minutes.
  File and folder names will be sent to {provider}." Cancelable, with progress. The per-folder token model needs
  calibration against real runs.
- Refresh: on access, event-driven for hot folders, and a monthly budget for the rest (default ~$10/month, adjustable,
  D54). Staleness from the fingerprint; the wake pipeline marks summaries stale instead of calling a model.
- Tools: `get_folder_summary`, `search_summaries` (FTS), `list_stale_summaries(min_interest)`, the recovery path after a
  full rescan or a purged cache (D20). The search box's natural-language path can use `search_summaries` too.
- Open detail: whether to copy `interest_weight` into `main.db` at summary time or always read `importance.db`. The
  indexer must never write into `main.db` either way.

Scope it against what proposals actually miss (acceptance data exists now), not against a whole-drive ambition up front.
Pull the eval fixture generator (item 11) forward for it.

**Size**: XL. Not blocked; wants a milestone plan and a David decision on the default spend.

## 3. Scoped rules: folder-specific guidance without files in folders

**Problem**: the user can write one global profile (`~/.cmdr/CMDR.md`, read into every prefix), but can't say "in
`~/Dropbox/invoices`, name files `YYYY-MM vendor.pdf`" without it riding every turn about every folder.

**Impact**: better proposals where the user has opinions, at no context cost elsewhere.

**Solution**: `~/.cmdr/rules/*.md`, each with optional YAML frontmatter `applies_to: <glob>`. A wake or turn loads only
the rules whose globs match the paths involved, into the stable prefix after the profile (`agent/chat/context.rs`,
`agent/chat/runtime/cmdr_md.rs` is the profile's loader to copy). User-authored, read-only to the agent: the agent's own
inferences stay in its memory folder (D25), so "the user told me" and "I inferred" never blur. Folder-level `CMDR.md`
files stay cut (D24: a cloned repo or zip can carry one). A profile that references another file ("see
`~/.claude/CLAUDE.md`") needs the reference inlined; the agent won't follow it.

**Size**: M. Not blocked.

## 4. Notification etiquette and a proactivity dial

**Problem**: proactivity today is an on/off toggle (`askCmdr.proactive`), a cadence slider (`askCmdr.wakeDelay`), a
toast toggle, and a daily spend ceiling (`agent/wake/spend.rs`). There's no cap on how often the agent surfaces
something, no per-folder mute, no "snooze today", and no named level the user picks.

**Impact**: principle 8. A too-eager agent is noise and gets switched off; a too-quiet default reads as "the feature
does nothing".

**Solution** (D39, D57): named, hard-coded policy bundles (off / quiet / normal / eager) mapping to interest thresholds
and a daily cap on surfaced suggestions; chosen during agent onboarding with "Normal" pre-highlighted, no silent
default. The cap counts only what reached the screen. Per-folder mute and "snooze today" at every level; throttle and
snooze state live in `main.db`, never in the settings store (D56). After several consecutive dismissals the agent may
ASK "want me to pipe down?", and ❌ never changes a setting by itself. Build on what ships: the corner indicator
(`agent/wake/indicator.rs`) and the staged toast (`agent/wake/staged.rs`) are the surfaces, not a new `notify_user`
tool. Settle alongside: whether every tunable gets a dial (D58) or the main UI keeps about three with the rest in an
advanced section.

**Size**: L. Wants a David decision on the bundles and on D58.

## 5. The bulk model slot

**Problem**: only the interactive model slot is settable (`askCmdr.interactiveModel`). Summarization (item 2) wants a
cheaper model than judgment does (D43).

**Impact**: without it, the knowledge layer runs on the interactive model and costs several times more.

**Solution**: an `askCmdr.bulkModel` key beside the interactive one, layered over the shared `ai/` provider config the
same way (`settings/loader.rs` reserves the name; no migration). Local is allowed in both slots, labeled honestly
("experimental, may underperform on agent tasks"), and degrades gracefully: summaries and simple chat keep working, wake
and planner do less instead of erroring, and after repeated loop failures the agent says politely that a cloud model
would do better (D53). Also decide whether the shipped local model (`ai/mod.rs` `AVAILABLE_MODELS`, Ministral 3B today)
is good enough at tool calling or wants replacing.

**Size**: S for the slot; M with the degradation policy. Blocked on item 2 (nothing uses the slot until then).

## 6. Move the drive index to `~/Library/Caches/`

**Problem**: the per-volume drive-index files (`index-{volume_id}.db`) live in the app data dir (Application Support),
so Time Machine backs up multi-GB regenerable caches, and macOS can't purge them under disk pressure.

**Impact**: backup bloat for every user; a platform-native "purgeable, don't back up" signal missed.

**Solution** (D1, D2, D50): move them to `~/Library/Caches/<bundle id>/` (or a `drive-index/` subdirectory there),
renamed `drive-index-{volume_id}.db`. Migrate by moving the files (a same-volume rename, cheap); a one-time full rescan
on upgrade is the acceptable fallback. A purge takes the same path as a full reindex. First verify that Time Machine
skips that path and how purging behaves. Independent of everything else here. The directory NAME is owned by
`docs/specs/later/data-dir-rename-spec-draft.md`; decide there whether the two ship as one migration or two.

**Size**: M. Not blocked.

## 7. Prompts as repo assets, with a `prompt-lint` check

**Problem**: every prompt (system prompt, wake, rejection follow-up) is Rust source (`agent/chat/system_prompt.rs` and
siblings), so iterating on wording means a rebuild, and nothing catches a template referencing a variable its call site
doesn't provide.

**Impact**: slower prompt iteration; a silent `{{folder_sumary}}` class of bug once templates exist.

**Solution** (D45): Markdown files with YAML frontmatter (`name`, `purpose`, intended model class, version note), plain
`{{variable}}` substitution, `minijinja` only where a prompt needs conditionals or loops. Dev builds load from disk;
release builds embed. A `prompt-lint` check in the Go check runner: every template compiles, and each prompt's variables
match what its call site provides.

**Size**: M. Not blocked. Worth doing before item 8 multiplies the prompt count.

## 8. A planner job for situations a single wake can't handle

**Problem**: a wake has one bounded tool loop. "Reorganize this project folder" or "sort three years of Downloads" wants
a longer, focused loop on one situation.

**Impact**: the agent stays limited to small, local suggestions.

**Solution** (D34): a third job type beside wake and chat: triggered when a wake decides a situation needs a plan, fed
the wake's context focused on one situation, a longer tool loop, the interactive model. It outputs ordinary proposals
through the existing spine. Before it ships, re-verify the provider quirks (parallel tool calls, reasoning-state
round-tripping, schema dialects; `agent/llm/DETAILS.md`) under a long multi-turn loop, which today are verified only for
the chat and wake shapes.

**Size**: L. Not blocked; best after items 1 and 7.

## 9. Auto-apply: let the user skip the review click

**Problem**: every proposal needs a review click. Some users will want "just do it".

**Impact**: convenience for trusting users; a trust risk if done wrong.

**Solution** (D60): a policy on the proposal pipeline's APPLY step, never raw tool exposure. The agent still emits
frozen proposals; the system applies them, keeping drift detection, per-op statuses, trash by default, the activity log,
and rollback through the operation log. A Settings toggle, default off; coarse first (off / auto-apply), scoped later
(per-folder, per-verb, a size ceiling). ❌ Changeable only in the Settings UI: `set_setting` over MCP must refuse it
regardless of token (the existing `mcpSettable: false` marker in `apps/desktop/src/lib/settings/types.ts` is the
mechanism), and the agent has no settings tool. Auto-applied work still surfaces ("the agent moved 12 files: review /
undo"), and an undo counts as a rejection.

**Size**: M. Blocked on item 1 and on acceptance-rate data justifying it; David decision.

## 10. A navigation-intent log as agent input

**Problem**: mutations reach the agent (the operation log, the outcome ring), and navigation reaches importance
(`record_visit` in `crates/cmdr-index/src/importance/writer.rs`), but the agent never sees what the user looks at: which
folders they opened, which file they searched for.

**Impact**: intent is the highest-signal input. "Opened `~/Downloads` three times today, then searched for 'invoice'"
tells a wake more than any FS event.

**Solution** (D21, D48): a `user_action_log` table in `main.db` for navigation and intent events only, never mutations
(those stay in `operation-log.db`; don't build a second recorder). Local only, an opt-out setting, ~90-day retention.
Feeds the wake digest as a few lines. Revisit whether `record_visit` folds into it or stays importance's own.

**Size**: M. Not blocked.

## 11. An eval harness for summaries and plans, doubling as the model regression suite

**Problem**: LLM behavior has only seeded evals (`agent/tools/propose/name_quality_eval.rs`, the importance corpus).
There's no fixture home directory to run the agent against, and no way to certify a new model.

**Impact**: model churn is guesswork; items 2 and 8 can't be scored.

**Solution** (D42): a fixture generator for synthetic home directories (build on `InMemoryVolume`), and a harness
scoring summarizer and planner output against expectations ("did it propose moving the invoices? did it leave the code
folder alone?"). Run per provider as a regression suite: a fixture run costs about a dollar, so certifying a model is a
button press. Pinned default models get an "untested" badge when a user overrides them.

**Size**: L. Not blocked; pull forward ahead of item 2.

## 12. Knowledge about volumes that aren't mounted

**Problem**: agent state keys by volume id, but there's no `volumes` table and no stable identity per volume, so the
same NAS share reached as `nas.local`, by IP, or by DNS name can be three volumes, and nothing records when a volume was
last reconciled.

**Impact**: blocks the headline "where's that 2024 photo backup?" answered from summaries while the NAS is off, and any
SMB, MTP, or S3 knowledge.

**Solution** (D5, D6, D7): a `volumes` table (`volume_id`, `kind`, `stable_identity`, `display_name`, index and summary
opt-ins, `last_reconciled_at`); a `stable_identity()` on the `Volume` trait (APFS UUID, server plus share for SMB,
device serial for MTP, endpoint plus bucket for S3), reusing the `Location` vocabulary (`src/location.rs`). Answers
caveat staleness ("as of May 28"). Per type: an SMB reconnect means "needs reconciliation, importance-gated rescan, diff
into a digest"; MTP summaries are on-demand only. Open: whether an SMB server exposes a GUID per protocol for
canonicalizing identity.

**Size**: L. Blocked on item 2 for most of its value.

## 13. `inspect_file` has no sensitive-path denylist

**Problem**: `inspect_file` reads any path the user can read, including `~/.ssh`, browser profiles, and keychain files,
and sends the text to the provider. The consent copy names content egress, but nothing refuses the obviously sensitive
places, and there's no per-wake read budget separate from the result caps.

**Impact**: one prompt-injected or confused turn can put a private key in a provider's logs.

**Solution**: a fixed sensitive-path denylist in `agent/tools/read/inspect/` answering a typed refusal row, plus a
per-wake read budget. Consider gating content-to-cloud separately from content-to-local-model. Any change to what
egresses is a consent-copy change and a `CONSENT_COPY_VERSION` bump (invariant 8 in `agent/DETAILS.md`).

**Size**: S for the denylist. David decision on whether the consent disclosure is enough.

## 14. Learning from the user's own moves

**Problem**: a rejection earns a follow-up turn that can write memory (`agent/wake/followup.rs`), but a manual move the
user makes (three PDFs from Downloads to `~/Dropbox/invoices`) teaches the agent nothing.

**Impact**: the agent keeps proposing what the user keeps doing differently by hand.

**Solution**: mine implicit signals from the operation log and item 10's intent log into PROPOSED memory entries. Open:
which signals, what confidence threshold, and whether a mined entry needs its own review affordance before it rides
every prefix (memory is an injection surface, `agent/memory/DETAILS.md`).

**Size**: L. Someday; best after item 10.

## 15. Per-folder enrollment in expensive analyses

**Problem**: there's no notion of which folders are enrolled in which expensive analyses (deep photo analysis, content
summaries), so it's all-or-nothing per feature.

**Impact**: users with a huge photo archive can't opt one folder in without paying for the rest.

**Solution**: a per-folder capability enrollment the agent can SUGGEST ("want me to analyze the photos in this folder?")
through its surfacing path (item 4), never through `proposal_ops`: the freeze and drift semantics fit file operations,
not settings changes.

**Size**: M. Someday; blocked on item 4.
