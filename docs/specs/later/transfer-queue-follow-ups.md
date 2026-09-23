# Transfer queue follow-ups

The lane-based queue shipped: one ordered admission queue with atomic multi-lane reservation, pause and resume that
reach mid-file, a standalone queue window with per-row pause, cancel, and rollback, and Pause + Queue controls on the
progress dialog. `apps/desktop/src-tauri/src/file_system/write_operations/DETAILS.md` § "Operation manager" and § "Pause
/ resume" own admission, lanes, settling, and the pause model; `transfer/DETAILS.md` § "Pause reaches between chunks"
owns the mid-file half; `apps/desktop/src/lib/file-operations/queue/CLAUDE.md` owns the window. The five items below are
independent of each other, and all were re-verified against the code on 2026-09-23.

## 1. Let a lane admit more than one operation at a time

- **Problem**: `LANE_BUDGET` is `const usize = 1` in
  `apps/desktop/src-tauri/src/file_system/write_operations/manager.rs`, so a second copy to the same NAS waits for the
  first even when the server would happily serve both. Two copies on genuinely different physical disks can also share a
  mount-root lane key and serialize for nothing.
- **Impact**: medium for people who queue several transfers to one SFTP or SMB server; throughput left on the table.
- **Solution**: a `Volume::lane_budget()` (default 1) beside the existing `lane_key()`, with the manager's free-check
  comparing `lane_use` (already a count map, chosen for this) against it. ❗ Not the same as
  `Volume::max_concurrent_ops()`, which sizes the file window WITHIN one transfer
  (`transfer/volume/copy.rs::transfer_concurrency`, and the `network.smbConcurrency` setting); the lane budget is how
  many separate OPERATIONS may hold one device. Plus finer local-device detection for the lane key. One risk is already
  bounded: `smb2` caps outstanding write payload connection-wide (32 MiB), so a budget above 1 on SMB can't multiply
  into hundreds of MB of uncancellable in-flight writes.
- **Size**: M (one to two days, mostly per-backend budgets and tests). Tradeoff: parallel operations on one device can
  be slower than serial on spinning disks, so budgets need per-backend judgment.

## 2. Resume after a long pause on SMB or SFTP without a transient error

- **Problem**: a paused transfer parks between chunks and keeps its source stream and backend handle open. Both sessions
  stay alive through a pause (SMB's `smb2` sends ECHO keepalives; SFTP sends SSH keepalives every 10 s,
  `crates/cmdr-sftp/DETAILS.md` § "Coming back"), but an open FILE handle may still hit a server-side idle limit on a
  very long pause, and the resume then surfaces a transient error. MTP is already immune: its reads are bounded windows
  that hold nothing between them (`transfer/DETAILS.md` § "Pause reaches between chunks").
- **Impact**: low. Both backends recover (SMB reconnects, per-file retry covers transient failures), so this buys
  smoothness rather than correctness.
- **Solution**: on resume, detect a stale handle and reopen at the parked offset rather than failing the file; for SFTP,
  an explicit reopen-on-resume is the likelier shape.
- **Size**: S–M. Triggered only by a real report of a resume that errors after a long pause; don't build it
  speculatively.

## 3. Bound how many paused operations can park blocking-pool threads

- **Problem**: `PauseGate::wait_while_paused_sync` (`write_operations/operation_intent.rs`) parks the operation's
  `spawn_blocking` thread for the whole pause, which the deferred-start design avoids for queued operations.
  `write_operations/DETAILS.md` § "Pause / resume" records this as an accepted asymmetry: a paused running operation
  legitimately holds its lane, and it's rarer than a queued one.
- **Impact**: low and hypothetical: only many simultaneously paused operations could pressure the blocking pool.
- **Solution**: if that pressure ever shows up, cap the number of parked threads (or park async). ❌ Don't build the
  bound speculatively.
- **Size**: S. Blocked on evidence that it matters.

## 4. Reorder the transfer queue

- **Problem**: no drag-to-reorder, no "run next", no priority bump in the queue window.
- **Impact**: low–medium. Someone who queues a big backup and then needs one small copy first has to cancel and requeue.
- **Solution**: admission walks a single FIFO `order` vector under the manager lock in `manager.rs`, so reordering is
  reordering that vector plus an IPC command; the real cost is the queue-window UI (drag, keyboard move, a11y).
- **Size**: M (mostly UI). Needs a David product call on the interaction (drag vs "Run next" vs both).

## 5. Keep queued operations across a restart

- **Problem**: the operation registry is in memory, so a crash or quit drops operations that were queued and never
  started. `apps/desktop/src-tauri/capabilities/queue.json` records the no-persistence choice by dropping
  `store:default`.
- **Impact**: low–medium. A long queue built before quitting is gone after relaunch.
- **Solution**: persist enough of each pending operation's descriptor to rebuild its `DeferredStart`, then offer to
  resume on next launch. An operation that was already running reopens what to do with its `.cmdr-tmp-<uuid>` partials,
  so start with never-started ones only.
- **Size**: M–L. Needs a David product call (auto-resume vs ask).
