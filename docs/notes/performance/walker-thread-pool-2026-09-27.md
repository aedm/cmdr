# Pooling the index walker's threads, and what it did to the allocator slack (2026-09-27)

**What this settles:** whether giving the index walker and the rescan drain long-lived threads cuts thread creation, and
whether that changes the mimalloc-versus-system-allocator decision in `allocator-comparison-2026-09-23.md`.

## The change

- `cmdr_fs::utility_pool::UtilityPool`: a keep-alive pool of `Utility`-QoS threads (60 s keep-alive, unbounded, never
  queues a job behind a busy thread).
- The walker's workers are jobs on the process-wide `WALK_THREADS`, and its watchdog runs on the thread that called
  `walk`, which blocks anyway. Mechanism: `crates/cmdr-index/src/indexing/scanner/walker/DETAILS.md` § "The engine".
- The rescan drain's walks run on `RESCAN_THREADS`.
