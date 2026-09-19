//! Tests for `transfer_driver.rs`'s concurrent per-file progress callback
//! (`make_concurrent_per_file_progress`).
//!
//! Coverage: the concurrent copy path (`copy_volumes_with_progress`'s
//! `FuturesUnordered`) has no between-files boundary, so its per-file progress
//! callback is cancel-only — it breaks on cancel but ignores pause. This pins
//! that contract so any future change to gate the concurrent path for pause is a
//! deliberate decision. See transfer/DETAILS.md § "Pause and the concurrent copy
//! path".

use super::super::super::event_sinks::CollectorEventSink;
use super::super::super::state::{register_operation_status, unregister_operation_status};
use super::super::super::types::WriteOperationType;
use super::test_support::{install_state, make_state, unique_op_id};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_per_file_callback_is_cancel_only_not_pause_aware() {
    // The concurrent copy path (`copy_volumes_with_progress` FuturesUnordered)
    // has no between-files boundary, so v1 does NOT gate it for pause: its
    // per-file progress callback breaks on cancel but ignores pause. Pin that so
    // a future change to gate the concurrent path is a deliberate decision, not
    // an accident. See transfer/DETAILS.md § "Pause and the concurrent copy
    // path".
    use super::LeafProgressLedger;
    use std::sync::atomic::AtomicUsize;

    let op_id = unique_op_id("concurrent-pause-noop");
    let state = make_state();
    let _op_guard = install_state(&op_id, Arc::clone(&state));
    register_operation_status(&op_id, WriteOperationType::Copy, vec![]);
    let sink: Arc<dyn super::super::super::event_sinks::OperationEventSink> = Arc::new(CollectorEventSink::new());

    let ledger = LeafProgressLedger::new(
        Arc::clone(&sink),
        Arc::clone(&state),
        op_id.clone(),
        WriteOperationType::Copy,
        Arc::new(AtomicUsize::new(0)),
        1,
        100,
        Duration::from_millis(0),
    );
    let leaf = ledger
        .for_source(Some("f".to_string()), Arc::new(Mutex::new(std::time::Instant::now())))
        .begin_leaf();

    // Paused, not cancelled: the chunk callback must still Continue (pause is a
    // no-op on the concurrent per-file path in v1).
    state.pause_gate.pause();
    assert_eq!(
        leaf.on_chunk(10),
        std::ops::ControlFlow::Continue(()),
        "concurrent per-file callback must ignore pause (cancel-only in v1)"
    );

    // Cancelled: it must Break, exactly as before.
    super::super::super::state::cancel_write_operation(&op_id, false);
    assert_eq!(
        leaf.on_chunk(20),
        std::ops::ControlFlow::Break(()),
        "concurrent per-file callback must still break on cancel"
    );
    unregister_operation_status(&op_id);
}
