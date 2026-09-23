# Ask Cmdr rename review: follow-ups

A bulk rename too big for one model reply now opens ONE review at the end of the turn, over every batch the model
staged, and Apply starts one operation per batch. How it works, why the batching stays, and the alternatives that were
rejected: `apps/desktop/src/lib/ask-cmdr/DETAILS.md` § "One review for one job". The batch arithmetic lives in
`apps/desktop/src-tauri/src/agent/chat/budget.rs`.

## 1. Open the rename review immediately and grow it as batches land

- **Problem**: the review opens only when the turn ends. A 500-file rename is five model replies at a 60,000-token
  budget and 22 at the default 16,000 (`budget::files_per_batch` caps a batch near 101 rows, and lower when the prompt
  binds), and the user sees nothing in the review while all of them stream.
- **Impact**: a long silent wait on exactly the job where the user most wants to follow along, with no early chance to
  spot a bad naming pattern and stop the turn before it burns every batch.
- **Solution**: open the review on the first staged plan and append rows as later plans arrive. The hard part is
  preflight: it's per proposal today, so each arriving batch needs its own preflight while the user is already reading
  and toggling earlier rows, and Apply has to stay disabled (or apply only what's settled) until the turn ends. The
  proposal stays the unit, so every per-row guardrail (evidence, pane-scoped source validation, the fingerprinted
  preflight, the write-time recheck) is unchanged. `ask-cmdr-stream.svelte.ts` stops holding `proposalReady` until turn
  end; `BulkRenameReviewDialog.svelte` grows a "still arriving" state.
- **Size**: M, frontend only. No backend change, since preflight, revise, apply, and cancel are already keyed by
  proposal id.
- **Clear win or a tradeoff?** A tradeoff: better feedback against a review that changes under the user's cursor. Wants
  a David call on whether the wait is actually felt before building it.
