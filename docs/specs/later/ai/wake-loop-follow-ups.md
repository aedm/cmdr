# What the wake loop still owes

The proactive agent ships end to end: it notices what changes on disk, decides whether that's worth saying, proposes,
remembers, and hears what the user did with each suggestion. The design lives beside the code:
`apps/desktop/src-tauri/src/agent/wake/DETAILS.md`, `agent/memory/DETAILS.md`, `agent/suggested_ops/DETAILS.md`, and the
rail's `apps/desktop/src/lib/ask-cmdr/DETAILS.md`. The agent's wider unbuilt work is
`docs/specs/later/ai/agent-follow-ups.md`. Two items are open.

## 1. Tune the wake loop's guessed constants from real wakes

**Problem**: five numbers were picked before anyone used the feature, and they're still guesses. The interest knobs in
`apps/desktop/src-tauri/src/agent/wake/interest.rs`: `UNKNOWN_IMPORTANCE_WEIGHT` (0.35) and the hot/warm thresholds
`HOT_THRESHOLD` (0.7) and `WARM_THRESHOLD` (0.3), which decide which folders qualify at all. The cadence constants in
`agent/wake/schedule.rs`: `DECLINED_WAKE_BACKOFF` (5 min) and `IDLE_POLL` (60 s). The outcome ring's size in
`agent/memory/outcomes.rs`: `OUTCOMES_MAX_BYTES` (4 KB) and `OUTCOMES_MAX_ENTRIES` (40).

**Impact**: thresholds set too high make the agent look idle; too low and it wakes on noise and burns the user's quota
(the daily ceiling in `agent/wake/spend.rs` is a backstop, not calibration). The cadence slider already made the DELAYS
a user choice, so this is only about the numbers a user can't move.

**Solution**: read a week of real wakes from what already ships for exactly this: the per-outcome counted log line
(`interest.rs`) and the anonymous analytics event (`agent/wake/runner.rs`). Rank, then move the numbers; each is a
one-line change. ❌ Don't tune from a single support message or from intuition.

**Size**: S once the data exists. Blocked on a week of real-use data.

## 2. The rail doesn't show an approve or reject until the thread reloads

**Problem**: `SuggestionsChanged` (`suggestions-changed`, emitted from
`apps/desktop/src-tauri/src/agent/suggested_ops/changed.rs`) fires on every approve and reject, but nothing under
`apps/desktop/src/lib/ask-cmdr/` subscribes; only the suggestions badge and trigger do. So an approve/reject line
reaches an open thread on next load, not live. The limitation is documented in
`apps/desktop/src/lib/ask-cmdr/DETAILS.md`.

**Impact**: small. The user sees a stale thread right after acting, which reads as "did that work?".

**Solution**: subscribe in the rail, filtered by conversation the way the turn stream already filters, so an open thread
refetches only when a decision concerns it. ❌ A naive subscription would refetch the open thread on every decision
anywhere.

**Size**: S. Not blocked.
