//! The per-read wall-clock guard: a hung directory read is abandoned near the
//! timeout and the reader serves the next one.

use super::*;

#[test]
fn guarded_reader_returns_a_quick_result() {
    let read_fn: ReadFn = Arc::new(|_p| Some(vec![]));
    let mut reader = GuardedReader::with_read_fn(
        Duration::from_secs(5),
        read_fn,
        VolumeWork::for_test("guarded-reader-test"),
    );
    assert!(
        reader.read(Path::new("/x")).0.is_some(),
        "a fast read returns its result"
    );
}

#[test]
fn guarded_reader_abandons_a_hung_read_and_recovers() {
    use std::sync::atomic::AtomicUsize;
    // Only the FIRST read hangs; later reads are fast. This proves both that the
    // hung read is abandoned near the timeout (not waited out) AND that the reader
    // recovers — respawns a worker — for the next read.
    let calls = Arc::new(AtomicUsize::new(0));
    let read_fn: ReadFn = {
        let calls = Arc::clone(&calls);
        Arc::new(move |_p| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                // allowed-test-sleep: this stub fakes a hung read; the whole test is that the reader
                // abandons it near the 50 ms timeout instead of waiting it out
                std::thread::sleep(Duration::from_secs(2));
            }
            Some(vec![])
        })
    };
    let mut reader = GuardedReader::with_read_fn(
        Duration::from_millis(50),
        read_fn,
        VolumeWork::for_test("guarded-reader-test"),
    );

    let start = Instant::now();
    assert!(reader.read(Path::new("/hang")).0.is_none(), "a hung read returns None");
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "must abandon near the timeout, not wait out the ~2s hang (elapsed {:?})",
        start.elapsed()
    );
    assert!(
        reader.read(Path::new("/ok")).0.is_some(),
        "the reader recovers after a timeout and serves the next read",
    );
}

/// A reader abandoned mid-read keeps the volume held until that read returns: the
/// walk has moved on, and a stop answering "released" over it would unmount under a
/// read in flight.
#[test]
fn an_abandoned_reader_holds_its_volume_until_its_read_returns() {
    let volume_id = "guarded-reader-test-abandoned";
    let (release, released) = channel::<()>();
    let released = std::sync::Mutex::new(released);
    let read_fn: ReadFn = Arc::new(move |p| {
        if p == Path::new("/hang") {
            let _ = released.lock().unwrap_or_else(|e| e.into_inner()).recv();
        }
        Some(vec![])
    });
    let volume = VolumeWork::for_test(volume_id);
    let mut reader = GuardedReader::with_read_fn(
        Duration::from_millis(50),
        read_fn,
        volume.child(HoldKind::ReconcileRead),
    );
    assert!(
        reader.read(Path::new("/hang")).0.is_none(),
        "precondition: the hung read was abandoned"
    );
    drop(reader);
    drop(volume);

    // Its replacement exits once it finds the walk gone; the abandoned reader is still
    // in its read.
    cmdr_fs::testing::wait_until(
        Duration::from_secs(5),
        "only the abandoned reader to still hold the volume",
        || {
            hold::wait_until_released(volume_id, Duration::ZERO)
                == Release::StillHeld(vec![(HoldKind::ReconcileRead, 1)])
        },
    );
    release.send(()).expect("the abandoned read is still waiting");
    assert_eq!(
        hold::wait_until_released(volume_id, Duration::from_secs(5)),
        Release::Released,
        "and it lets go once its read returns"
    );
}
