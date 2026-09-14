# Indexing integration + stress tests

Cross-module tests that exercise the whole pipeline (scan → aggregate → enrich → watch) or hammer it under load. Unit
tests stay colocated in each module; these are the integration tier.

## Module map

- **integration_tests.rs** — end-to-end (scan → aggregate → enrich → watcher update → re-enrich), the enrichment fast
  path / fallback / root-level path, the `ReadPool` (reuse, invalidation, cross-thread, contention), the
  `should_auto_start_indexing` FDA gate, and the `IndexPhase` lifecycle transitions.
- **event_stream_tests.rs** — what a scan tells its host, asserted at the `EventSink`: the lifecycle's shape
  (`ScanStarted` first, `ScanComplete` after, progress in between), the reporter's tick, and per-volume isolation (two
  concurrent scans, two streams, neither mentioning the other). Drives a real `IndexManager` over a temp-dir fixture
  with a `RecordingSink` — no app involved.
- **stress_tests\_{concurrency,lifecycle,partial_aggregation}.rs** + **stress_test_helpers.rs** — concurrency,
  start/stop/restart-under-load, and the partial-aggregation differential test, plus shared setup helpers.
- **external_drive_fixture.rs** — a macOS-only synthetic disk-image FIXTURE (`#[cfg(target_os = "macos")]`), NOT a test
  file. Its FSKit-panic-safe attach/detach discipline is load-bearing.
- **vanish_tests.rs** — macOS real-image pins of a drive vanishing mid-scan, on the walker's park point (`#[ignore]`d).

## Must-knows

- **⚠️ FSKit kernel-panic guardrail: never mount, unmount, or probe a physical removable card in a test or a research
  script.** On 2026-07-15 a `diskutil unmount` on a physical, nearly-full FAT32 SD card wedged macOS 26's userspace
  FSKit `msdos` service mid-unmount; it held kernel vnode locks until the pile-up blocked WindowServer and the watchdog
  **kernel-panicked and rebooted the machine**. The wedge happens DURING unmount, so no post-unmount hook can undo it —
  the only defense is to never trigger it. Every real-image test goes through the guarded runner in
  `cmdr_fs::testing::disk_images` (30 s → SIGKILL per call, a machine-wide lock, an ownership proof before every
  change). **A FAT/exFAT image (`external_drive_fixture`) is attached once and detached once: never cycle its mount,
  never `diskutil unmount` it.** APFS and HFS+ images may unmount under test; that's what the pins are for. ❌ Don't
  "clean up" the timeouts or the single-detach discipline; they're the guardrail against the incident.
- **Tests serialize on a dedicated mutex.** `INDEX_REGISTRY` is a global; concurrent tests corrupt each other. The
  pattern (in `integration_tests.rs` and `state/tests.rs`): a dedicated guard mutex + an `IndexStore` fixtured via
  `tempdir`, clearing the `root` entry AND withdrawing root's read handles before and after. ❌ NEVER
  `INDEX_REGISTRY.clear()` in a test: it wipes every OTHER module's concurrent private instances (an isolation flake);
  remove only your own ids. A registry removal alone no longer un-routes reads — uninstall the handles too, or go
  through `stop_indexing`.
- **A test that asserts on pending-sizes / read-pool / `dir_stats` state must route through a PRIVATE per-volume
  instance, never the ROOT volume's tracker or pool** (foreign root writers clear those under bare `cargo test`).
  `stress_test_helpers::TestInstanceGuard` (the shared home) registers one under a unique id and removes it on drop;
  `register_identity_paths` gives an `mtp-` id whose read side maps plain `/paths` identically, so
  `get_dir_stats_on_volume` / `enrich_*_on_volume` work privately. Rationale: `writer/DETAILS.md` § "Test isolation".
- **"Disabled" is the absence of an instance.** There's no `IndexPhase::Disabled`, so assert `!contains_key` (or
  `get_read_pool_for(vid).is_none()`, the read-path "is it indexed?" predicate), never "phase is Disabled".
- **The external-drive tests are `#[ignore]`d and serialized** via the `disk-image` nextest group
  (`.config/nextest.toml`, 30 s cap); `pnpm check rust` compiles them but the default suite skips them. Concurrent
  attach/detach churn on one FSKit service is the very surface the incident warns about.

The test inventory, the state-machine testing bar, and the disk-image fixture mechanics: `DETAILS.md`. Read it before
any non-trivial work here: editing, planning, reorganizing, or advising.
