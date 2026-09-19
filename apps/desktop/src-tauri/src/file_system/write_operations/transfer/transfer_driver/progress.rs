//! Leaf-granular progress accounting shared by every volume transfer path.
//!
//! The progress bars are LEAF-granular: `bytes_total` and `files_total` are the
//! preflight leaf counts, so the emitted `bytes_done` / `files_done` have to
//! climb across leaves, across top-level sources, and across the many leaves a
//! single directory source streams AT ONCE. Three types split that job:
//!
//! - [`LeafProgressLedger`] is OPERATION-wide. It owns the two numbers every
//!   event carries: `finished` (bytes of leaves that have landed or been
//!   skipped) and `in_flight` (the sum of what each unfinished leaf has
//!   reported). One per operation, shared by every source.
//! - [`SourceProgress`] is one TOP-LEVEL source's view of that ledger, carrying
//!   the name its events are labeled with. Leaves are minted from it.
//! - [`LeafProgress`] is ONE leaf file's handle, held for that file's whole
//!   life. It owns the leaf's high-water mark and withdraws it from the ledger
//!   when the leaf settles, whichever way it settles.
//!
//! ## Why a leaf needs a handle of its own
//!
//! A directory source's subtree streams many files at once through the
//! operation-wide `volume/strategy.rs::FileWindow`, and they all report into the
//! same accounting. One shared high-water slot cannot hold that: the biggest
//! leaf's offset is what the bar shows, and the next leaf to finish — any leaf,
//! however small — resets the slot and drops the reported total by everything
//! the big one had streamed. On a 664 MB folder whose largest file was 259 MB
//! that read as the Size bar falling from ~300 MB back to ~80 MB, once per
//! completed file, while the bytes themselves were moving fine.
//!
//! So each leaf gets its own mark and the ledger sums them. A leaf that settles
//! withdraws exactly what it contributed and, when it LANDED, adds its exact
//! byte count to `finished` in the same breath, under one lock — which is what
//! keeps the total from dipping between the two halves of that swap.

use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::file_system::write_operations::event_sinks::OperationEventSink;
use crate::file_system::write_operations::state::{WriteOperationState, is_cancelled};
use crate::file_system::write_operations::types::{WriteOperationPhase, WriteOperationType};
use crate::ignore_poison::IgnorePoison;

use super::emit_progress_and_status;

/// The operation's byte total, split into the part that is settled and the part
/// that is still moving.
///
/// Both live under ONE lock because every emit reads them together: reading
/// `finished` and `in_flight` from two separate atomics lets a leaf settle
/// between the two reads, and the reader then pairs an old `finished` with a
/// new `in_flight` and reports a number lower than the one before it.
struct Totals {
    /// Bytes of leaves that have LANDED or been skipped, plus whatever the
    /// driver credited up front (bulk-skipped sources).
    finished: u64,
    /// The sum of every in-flight leaf's high-water mark.
    in_flight: u64,
}

impl Totals {
    fn reported(&self) -> u64 {
        self.finished + self.in_flight
    }
}

/// Operation-wide leaf accounting. See the module header for the split.
pub(in crate::file_system::write_operations::transfer) struct LeafProgressLedger {
    /// `None` for a SILENT ledger, which still tracks bytes and still stops a
    /// cancelled transfer through [`LeafProgress::on_chunk`], but tells nobody.
    /// See [`LeafProgressLedger::silent`].
    events: Option<Arc<dyn OperationEventSink>>,
    state: Arc<WriteOperationState>,
    operation_id: String,
    operation_type: WriteOperationType,
    totals: Mutex<Totals>,
    /// Operation-wide completed-leaf counter, shared with the drivers (which
    /// seed it with the bulk-skipped leaves and read it for their own events).
    files_done: Arc<AtomicUsize>,
    total_files: usize,
    total_bytes: u64,
    progress_interval: Duration,
}

impl LeafProgressLedger {
    #[allow(
        clippy::too_many_arguments,
        reason = "matches WriteProgressEvent shape; bundling into a context struct adds ceremony without cleaning anything up"
    )]
    pub(in crate::file_system::write_operations::transfer) fn new(
        events: Arc<dyn OperationEventSink>,
        state: Arc<WriteOperationState>,
        operation_id: String,
        operation_type: WriteOperationType,
        files_done: Arc<AtomicUsize>,
        total_files: usize,
        total_bytes: u64,
        progress_interval: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            events: Some(events),
            state,
            operation_id,
            operation_type,
            totals: Mutex::new(Totals {
                finished: 0,
                in_flight: 0,
            }),
            files_done,
            total_files,
            total_bytes,
            progress_interval,
        })
    }

    /// A ledger for a transfer nobody is watching: the archive copy-into flow
    /// materializing a remote source in a scratch dir it will repackage and
    /// throw away. It has no operation of its own to report against, so it
    /// emits nothing; what it still owes its caller is the per-chunk cancel
    /// check, which rides `state` exactly as a reporting ledger's does.
    pub(in crate::file_system::write_operations::transfer) fn silent(state: Arc<WriteOperationState>) -> Arc<Self> {
        Arc::new(Self {
            events: None,
            state,
            operation_id: String::new(),
            operation_type: WriteOperationType::Copy,
            totals: Mutex::new(Totals {
                finished: 0,
                in_flight: 0,
            }),
            files_done: Arc::new(AtomicUsize::new(0)),
            total_files: 0,
            total_bytes: 0,
            progress_interval: Duration::ZERO,
        })
    }

    /// A [`silent`](Self::silent) ledger's one source view, which is all a
    /// caller that reports to nobody ever needs.
    pub(in crate::file_system::write_operations::transfer) fn silent_source(
        state: Arc<WriteOperationState>,
    ) -> Arc<SourceProgress> {
        Self::silent(state).for_source(None, Arc::new(Mutex::new(Instant::now())))
    }

    /// One top-level source's view, carrying the name its events are labeled
    /// with. Every leaf of that source is minted from the returned handle.
    ///
    /// `last_emit` is the throttle clock, and whose it is belongs to the caller:
    /// a driver whose sources overlap shares ONE across them, so the event rate
    /// the user sees is the operation's rather than each task's. A driver that
    /// runs sources one at a time may hand each a fresh clock instead, since the
    /// previous source's last-emit instant says nothing about this one.
    pub(in crate::file_system::write_operations::transfer) fn for_source(
        self: &Arc<Self>,
        file_name: Option<String>,
        last_emit: Arc<Mutex<Instant>>,
    ) -> Arc<SourceProgress> {
        Arc::new(SourceProgress {
            ledger: Arc::clone(self),
            file_name,
            last_emit,
        })
    }

    /// Credits bytes that landed (or were declined) OUTSIDE any leaf: the
    /// driver's bulk-skip prelude, and a source whose transfer reported a byte
    /// count no leaf accounted for.
    pub(in crate::file_system::write_operations::transfer) fn credit_finished(&self, bytes: u64) {
        self.totals.lock_ignore_poison().finished += bytes;
    }

    /// Replaces the settled total with the driver's own running tally.
    ///
    /// The SERIAL driver keeps its own `bytes_done` across top-level sources
    /// (it counts skipped and bulk-skipped sources the leaves never see), and
    /// hands it down per iteration. One source is in flight at a time there, so
    /// overwriting is exact; the concurrent driver never calls this, because
    /// its sources overlap and the ledger IS its tally.
    pub(in crate::file_system::write_operations::transfer) fn reseed_finished(&self, bytes: u64) {
        self.totals.lock_ignore_poison().finished = bytes;
    }

    /// What has actually landed, which is what a driver reports as the
    /// operation's outcome. Excludes in-flight leaves by construction.
    pub(in crate::file_system::write_operations::transfer) fn finished_bytes(&self) -> u64 {
        self.totals.lock_ignore_poison().finished
    }

    fn emit(&self, file_name: Option<String>, files_done: usize, bytes_done: u64) {
        let Some(events) = self.events.as_ref() else {
            return;
        };
        emit_progress_and_status(
            &**events,
            &self.state,
            &self.operation_id,
            self.operation_type,
            WriteOperationPhase::Copying,
            file_name,
            files_done,
            self.total_files,
            bytes_done,
            self.total_bytes,
        );
    }
}

/// One top-level source's view of the operation's ledger.
pub(in crate::file_system::write_operations::transfer) struct SourceProgress {
    ledger: Arc<LeafProgressLedger>,
    /// The name every event from this source carries. A directory source labels
    /// its events with the FOLDER's name, not each inner file's.
    file_name: Option<String>,
    /// This source's throttle clock. Shared with its siblings or private to it,
    /// as [`LeafProgressLedger::for_source`] describes.
    last_emit: Arc<Mutex<Instant>>,
}

impl SourceProgress {
    /// Opens accounting for ONE leaf file, held for that file's whole life.
    ///
    /// ❗ The returned handle must live until the leaf settles: its `Drop` is
    /// what takes a failed or cancelled leaf's bytes back out of the in-flight
    /// sum. Dropping it early reports the rest of that file's stream as nothing,
    /// and leaking it reports bytes that never landed.
    pub(in crate::file_system::write_operations::transfer) fn begin_leaf(self: &Arc<Self>) -> LeafProgress {
        LeafProgress {
            source: Arc::clone(self),
            high_water: AtomicU64::new(0),
            settled: AtomicBool::new(false),
        }
    }

    /// Credits ONE leaf the conflict policy declined.
    ///
    /// A skipped leaf IS done, so both bars have to count it; `note_skipped` is
    /// what keeps its bytes out of the rate, since nothing moved for them.
    ///
    /// ❗ Throttled, unlike [`LeafProgress::complete`]. A merge into a folder
    /// the user already has can decline tens of thousands of children back to
    /// back, and an unthrottled emit apiece is a flood of IPC nobody reads. The
    /// operation's completion event carries the final tally regardless.
    pub(in crate::file_system::write_operations::transfer) fn skip_leaf(&self, leaf_bytes: u64) {
        let ledger = &self.ledger;
        let reported = {
            let mut totals = ledger.totals.lock_ignore_poison();
            totals.finished += leaf_bytes;
            totals.reported()
        };
        let files_done = ledger.files_done.fetch_add(1, Ordering::Relaxed) + 1;
        ledger.state.note_skipped(1, leaf_bytes);
        // The bool says whether the throttle let this tick through, and a skip
        // has nothing to do about either answer: the counters are credited
        // above, and the completion event carries the final tally whatever the
        // throttle ate.
        // allowed-discarded-outcome: whether this tick was throttled changes nothing here.
        try_emit_throttled(self, files_done, reported);
    }
}

/// One leaf file's slot in the operation's in-flight sum.
///
/// Held for the file's whole life, including across the per-file retries
/// `retry.rs` runs, and settled exactly once: by [`complete`](Self::complete)
/// when the bytes landed, or by `Drop` when they didn't.
pub(in crate::file_system::write_operations::transfer) struct LeafProgress {
    source: Arc<SourceProgress>,
    /// The furthest this leaf has reported, which is exactly what it has
    /// contributed to the ledger's in-flight sum.
    ///
    /// A file that hits a transport blip is run again from its first byte
    /// (`retry.rs`), so the raw per-chunk count legitimately drops to 0
    /// mid-leaf. The bar must not: dropping from 4 MiB back to 0 reads as data
    /// being lost, and the ETA estimator would take the reversal as negative
    /// throughput. Holding the high-water mark keeps the number monotonic AND
    /// keeps it honest at the end, because `complete` adds the leaf's exact size
    /// once, whatever the attempt count.
    high_water: AtomicU64,
    /// Whether this leaf has already been taken out of the in-flight sum, so
    /// `Drop` after a `complete` doesn't withdraw a second time.
    settled: AtomicBool,
}

impl LeafProgress {
    /// Per-chunk progress. `file_bytes_done` is THIS leaf's running byte count
    /// (0 → leaf size). Throttled; returns `Break` to abort the write on cancel.
    pub(in crate::file_system::write_operations::transfer) fn on_chunk(&self, file_bytes_done: u64) -> ControlFlow<()> {
        let ledger = &self.source.ledger;
        if is_cancelled(&ledger.state.intent) {
            return ControlFlow::Break(());
        }
        let reported = {
            let mut totals = ledger.totals.lock_ignore_poison();
            // Under the ledger's lock, so the sum and this leaf's share of it
            // always agree. Only this leaf's own task writes `high_water`.
            let previous = self.high_water.load(Ordering::Relaxed);
            if file_bytes_done > previous {
                self.high_water.store(file_bytes_done, Ordering::Relaxed);
                totals.in_flight += file_bytes_done - previous;
            }
            totals.reported()
        };
        let files_done = ledger.files_done.load(Ordering::Relaxed);
        // allowed-discarded-outcome: a throttled chunk needs no follow-up; the counters are already credited.
        try_emit_throttled(&self.source, files_done, reported);
        ControlFlow::Continue(())
    }

    /// The leaf LANDED: swap its in-flight share for its exact byte count and
    /// bump the operation-wide leaf counter.
    ///
    /// The swap happens under one lock so the total never dips between the two
    /// halves, and it can only ever raise the total: a leaf's high-water mark
    /// is bounded by the bytes it streamed.
    ///
    /// Emits UNTHROTTLED so the bumped `files_done` always reaches the frontend
    /// — chunked emits inside the file carry the pre-completion counter, so
    /// without this a single large leaf would never cross `N/N`.
    pub(in crate::file_system::write_operations::transfer) fn complete(self, leaf_bytes: u64) {
        let ledger = &self.source.ledger;
        let reported = {
            let mut totals = ledger.totals.lock_ignore_poison();
            totals.in_flight -= self.withdraw_share();
            totals.finished += leaf_bytes;
            totals.reported()
        };
        let files_done = ledger.files_done.fetch_add(1, Ordering::Relaxed) + 1;
        *self.source.last_emit.lock_ignore_poison() = Instant::now();
        ledger.emit(self.source.file_name.clone(), files_done, reported);
    }

    /// This leaf's contribution to the in-flight sum, claimed exactly once.
    /// Returns `0` on every call after the first, so the sum can't go negative
    /// when `Drop` runs behind a `complete`.
    ///
    /// ❗ Callers hold the ledger's lock. It isn't taken here because `complete`
    /// needs the withdrawal and the credit to land under ONE acquisition.
    fn withdraw_share(&self) -> u64 {
        if self.settled.swap(true, Ordering::Relaxed) {
            return 0;
        }
        self.high_water.load(Ordering::Relaxed)
    }
}

impl Drop for LeafProgress {
    /// A leaf that never completed — a read failure, a transport give-up, a
    /// cancel between chunks — has to take its bytes back out: they aren't on
    /// the destination, and leaving them in the sum reports data that never
    /// landed. The reported total DOES fall here, which is the honest answer
    /// for a file that failed.
    fn drop(&mut self) {
        let share = self.withdraw_share();
        if share > 0 {
            self.source.ledger.totals.lock_ignore_poison().in_flight -= share;
        }
    }
}

/// Throttle gate + paired emit. Returns `true` if it emitted, `false` if the
/// call was suppressed by the throttle.
///
/// Two callers racing on the gate can both succeed; over-emission is fine — the
/// throttle protects the *floor* event rate, not a strict ceiling. The Mutex is
/// released before the emit (which may take its own internal locks for the ETA
/// estimator and status cache), so the gate never serializes downstream emits.
fn try_emit_throttled(source: &SourceProgress, files_done: usize, bytes_done: u64) -> bool {
    let mut last = source.last_emit.lock_ignore_poison();
    if last.elapsed() < source.ledger.progress_interval {
        return false;
    }
    *last = Instant::now();
    drop(last);
    source
        .ledger
        .emit(source.file_name.clone(), files_done, bytes_done);
    true
}

/// A ledger wired to a collector, for an engine test that needs to watch the
/// bytes a copy reports as it runs.
///
/// It reads the emitted progress EVENTS, which is what the transfer dialog
/// reads, so a test asserting on it is asserting on the number the user sees.
/// That is also why the engine tests don't hand-roll the per-chunk closure any
/// more: the closure they used to pass was a copy of production's, so it could
/// agree with itself while disagreeing with the app.
#[cfg(test)]
pub(in crate::file_system::write_operations::transfer) struct ObservedProgress {
    pub(in crate::file_system::write_operations::transfer) source: Arc<SourceProgress>,
    /// The byte total the latest progress event carried. An `AtomicU64` rather
    /// than an accessor so the engine tests' existing waiters (`wait_for_bytes`,
    /// `park_holds_at`) read it the way they always have.
    pub(in crate::file_system::write_operations::transfer) bytes: Arc<AtomicU64>,
    /// The leaf count the latest progress event carried.
    pub(in crate::file_system::write_operations::transfer) files: Arc<AtomicUsize>,
    /// How many progress events the copy has emitted.
    pub(in crate::file_system::write_operations::transfer) emits: Arc<AtomicUsize>,
}

/// Records what a progress event says and discards every other emit.
#[cfg(test)]
struct ByteWatchSink {
    bytes: Arc<AtomicU64>,
    files: Arc<AtomicUsize>,
    emits: Arc<AtomicUsize>,
}

#[cfg(test)]
#[allow(
    unused_variables,
    reason = "this sink exists to record ONE field of ONE event; every other emit is deliberately dropped"
)]
impl OperationEventSink for ByteWatchSink {
    fn emit_progress(&self, event: crate::file_system::write_operations::types::WriteProgressEvent) {
        self.bytes.store(event.bytes_done, Ordering::SeqCst);
        self.files.store(event.files_done, Ordering::SeqCst);
        self.emits.fetch_add(1, Ordering::SeqCst);
    }
    fn emit_complete(&self, event: crate::file_system::write_operations::types::WriteCompleteEvent) {}
    fn emit_cancelled(&self, event: crate::file_system::write_operations::types::WriteCancelledEvent) {}
    fn emit_error(&self, event: crate::file_system::write_operations::types::WriteErrorEvent) {}
    fn emit_conflict(&self, event: crate::file_system::write_operations::types::WriteConflictEvent) {}
    fn emit_conflict_resolved(&self, event: crate::file_system::write_operations::types::WriteConflictResolvedEvent) {}
    fn emit_source_item_done(&self, event: crate::file_system::write_operations::types::WriteSourceItemDoneEvent) {}
    fn emit_scan_progress(&self, event: crate::file_system::write_operations::types::ScanProgressEvent) {}
    fn emit_scan_conflict(&self, conflict: crate::file_system::write_operations::types::ConflictInfo) {}
    fn emit_dry_run_complete(&self, result: crate::file_system::write_operations::types::DryRunResult) {}
    fn emit_settled(&self, event: crate::file_system::write_operations::types::WriteSettledEvent) {}
}

#[cfg(test)]
impl ObservedProgress {
    pub(in crate::file_system::write_operations::transfer) fn new(
        state: &Arc<WriteOperationState>,
        operation_id: &str,
    ) -> Arc<Self> {
        let bytes = Arc::new(AtomicU64::new(0));
        let files = Arc::new(AtomicUsize::new(0));
        let emits = Arc::new(AtomicUsize::new(0));
        let ledger = LeafProgressLedger::new(
            Arc::new(ByteWatchSink {
                bytes: Arc::clone(&bytes),
                files: Arc::clone(&files),
                emits: Arc::clone(&emits),
            }) as Arc<dyn OperationEventSink>,
            Arc::clone(state),
            operation_id.to_owned(),
            WriteOperationType::Copy,
            Arc::new(AtomicUsize::new(0)),
            1,
            u64::MAX,
            // No throttle: a test watching for one chunk must see it.
            Duration::ZERO,
        );
        Arc::new(Self {
            source: ledger.for_source(None, Arc::new(Mutex::new(Instant::now() - Duration::from_secs(1)))),
            bytes,
            files,
            emits,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_system::write_operations::event_sinks::CollectorEventSink;
    use crate::file_system::write_operations::test_support::TestOperationGuard;

    /// The bytes the FE was told about, in order.
    fn totals(sink: &CollectorEventSink) -> Vec<u64> {
        sink.progress
            .lock_ignore_poison()
            .iter()
            .map(|e| e.bytes_done)
            .collect()
    }

    fn source_progress(guard: &TestOperationGuard, sink: &Arc<CollectorEventSink>, seed: u64) -> Arc<SourceProgress> {
        let ledger = LeafProgressLedger::new(
            Arc::clone(sink) as Arc<dyn OperationEventSink>,
            Arc::clone(guard.state()),
            guard.id().to_owned(),
            WriteOperationType::Copy,
            Arc::new(AtomicUsize::new(0)),
            10,
            1_000_000,
            // No throttle: every call must be observable.
            Duration::ZERO,
        );
        ledger.reseed_finished(seed);
        ledger.for_source(None, Arc::new(Mutex::new(Instant::now() - Duration::from_secs(1))))
    }

    /// A file that is run again after a transport blip restarts at byte zero
    /// (`retry.rs`), so the leaf's own counter goes backwards. What the user sees
    /// must not: the Size bar dropping from 4 MiB back to 0 and climbing again
    /// reads as data being lost, and the ETA estimator would take the reversal as
    /// negative throughput.
    #[test]
    fn a_retried_leaf_never_walks_the_size_bar_backwards() {
        let guard = TestOperationGuard::register("leaf-retry-progress");
        let sink = Arc::new(CollectorEventSink::new());
        let progress = source_progress(&guard, &sink, 1_000);

        let leaf = progress.begin_leaf();
        // First attempt gets a third of the way in, then the blip.
        let _ = leaf.on_chunk(2_000);
        let _ = leaf.on_chunk(4_000);
        // The retry starts over from zero.
        let _ = leaf.on_chunk(1_000);
        let _ = leaf.on_chunk(4_000);
        let _ = leaf.on_chunk(6_000);
        leaf.complete(6_000);

        let seen = totals(&sink);
        assert!(
            seen.windows(2).all(|w| w[1] >= w[0]),
            "the reported byte total must never go backwards across a retry: {seen:?}"
        );
        assert_eq!(
            *seen.last().expect("the leaf milestone emits"),
            7_000,
            "the finished leaf must be counted exactly once: base 1000 + 6000 bytes"
        );
    }

    /// A DIRECTORY source streams many leaves at once through the
    /// operation-wide `volume/strategy.rs::FileWindow`, all reporting into this
    /// accounting. A single shared high-water slot could not hold that: the big
    /// leaf's mark was what the bar showed, and the next small leaf to finish
    /// wiped it, dropping the reported total by everything the big one had
    /// streamed.
    #[test]
    fn a_small_leaf_finishing_does_not_wipe_a_big_leaf_still_in_flight() {
        let guard = TestOperationGuard::register("leaf-interleaved-progress");
        let sink = Arc::new(CollectorEventSink::new());
        let progress = source_progress(&guard, &sink, 0);

        // A big leaf climbs while a small one runs alongside it.
        let big = progress.begin_leaf();
        let small = progress.begin_leaf();
        let _ = big.on_chunk(200_000);
        let _ = small.on_chunk(1_000);
        // The small one lands first. The big one is still streaming.
        small.complete(1_000);
        let _ = big.on_chunk(210_000);

        let seen = totals(&sink);
        assert!(
            seen.windows(2).all(|w| w[1] >= w[0]),
            "a leaf completing must not drop the total while another is still in flight: {seen:?}"
        );
        assert_eq!(
            *seen.last().expect("the in-flight chunk emits"),
            211_000,
            "the finished leaf's 1,000 bytes plus the in-flight leaf's 210,000"
        );
    }

    /// Several leaves in flight add up, rather than the largest one standing in
    /// for all of them. The old single-slot accounting reported `max`, so a
    /// subtree streaming ten files at once showed roughly a tenth of its real
    /// progress between completions.
    #[test]
    fn leaves_in_flight_sum_instead_of_reporting_the_largest() {
        let guard = TestOperationGuard::register("leaf-in-flight-sum");
        let sink = Arc::new(CollectorEventSink::new());
        let progress = source_progress(&guard, &sink, 0);

        let first = progress.begin_leaf();
        let second = progress.begin_leaf();
        let third = progress.begin_leaf();
        let _ = first.on_chunk(5_000);
        let _ = second.on_chunk(3_000);
        let _ = third.on_chunk(2_000);

        assert_eq!(
            totals(&sink).last().copied(),
            Some(10_000),
            "three leaves at 5,000 + 3,000 + 2,000 are 10,000 bytes of progress, not 5,000"
        );
    }

    /// A leaf that never landed — a read failure, a give-up, a cancel between
    /// chunks — takes its bytes back out. They aren't on the destination, and
    /// leaving them in would report data that never arrived, which is the one
    /// direction this accounting must never lie in.
    #[test]
    fn a_leaf_that_never_landed_withdraws_its_bytes() {
        let guard = TestOperationGuard::register("leaf-failed-withdraws");
        let sink = Arc::new(CollectorEventSink::new());
        let progress = source_progress(&guard, &sink, 0);

        let landed = progress.begin_leaf();
        let _ = landed.on_chunk(4_000);
        landed.complete(4_000);

        {
            let doomed = progress.begin_leaf();
            let _ = doomed.on_chunk(7_000);
            assert_eq!(
                totals(&sink).last().copied(),
                Some(11_000),
                "while it is streaming, its bytes count"
            );
        }

        let survivor = progress.begin_leaf();
        let _ = survivor.on_chunk(500);
        assert_eq!(
            totals(&sink).last().copied(),
            Some(4_500),
            "the failed leaf's 7,000 bytes are gone again: only the landed 4,000 plus the new leaf's 500"
        );
    }

    /// And a leaf's `Drop` after a `complete` must not withdraw a second time,
    /// which would silently erase bytes that really did land.
    #[test]
    fn a_completed_leaf_is_not_withdrawn_again_when_it_drops() {
        let guard = TestOperationGuard::register("leaf-complete-then-drop");
        let sink = Arc::new(CollectorEventSink::new());
        let progress = source_progress(&guard, &sink, 0);

        {
            let leaf = progress.begin_leaf();
            let _ = leaf.on_chunk(9_000);
            leaf.complete(9_000);
        }

        let next = progress.begin_leaf();
        let _ = next.on_chunk(100);
        assert_eq!(
            totals(&sink).last().copied(),
            Some(9_100),
            "the completed leaf's bytes stay counted after its handle drops"
        );
    }

    /// A skipped leaf is done: both bars count it, and its bytes stay out of the
    /// rate.
    #[test]
    fn a_skipped_leaf_credits_both_bars() {
        let guard = TestOperationGuard::register("leaf-skipped-credits");
        let sink = Arc::new(CollectorEventSink::new());
        let progress = source_progress(&guard, &sink, 0);

        progress.skip_leaf(2_500);

        assert_eq!(
            totals(&sink).last().copied(),
            Some(2_500),
            "a declined child still moves the Size bar to its total"
        );
        assert_eq!(
            sink.progress
                .lock_ignore_poison()
                .last()
                .expect("the skip emits")
                .files_done,
            1,
            "and the File bar, or a merge that declines everything reaches neither total"
        );
    }
}
