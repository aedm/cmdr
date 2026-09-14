//! The network pass's volume-kind gate, over real registry instances and a fake host.
//!
//! These drive the process-global provider slot, read pools, and master-toggle gate,
//! so each holds `handle::test_lock()` and `crate::test_read_pool_lock()` and resets the
//! gate itself.

use std::sync::Arc;

use cmdr_fs::testing::wait_until;
use cmdr_fs::volume::InMemoryVolume;

use super::kick_tests::{fake_backend, reset_gate};
use super::*;
use crate::IndexVolumeKind;
use crate::indexing::host::volumes::{FakeVolumeProvider, install_for_test};
use crate::indexing::lifecycle::state::{reserve_initializing_index_for_test, stop_indexing};
use crate::media_index::store::{media_db_path, read_mount_roots};

/// A hand-edited opt-in list can name a local external drive. The network pass reads
/// through the volume's mount root, and nothing a drive's stop waits on holds it, so
/// it refuses a volume whose index isn't a network kind before it reads anything:
/// here, before it even opens the drive's media store. The share runs the same pass
/// to its first write, so the refusal can't pass by the pass never running at all.
#[test]
fn a_network_pass_refuses_a_local_drive_before_reading_anything() {
    let _serialized = crate::indexing::handle::test_lock();
    let _pools = crate::test_read_pool_lock();
    reset_gate();
    gate::set_enabled(true);
    let data = tempfile::tempdir().expect("data dir");
    let volumes = FakeVolumeProvider::shared();
    let _host = install_for_test(Arc::clone(&volumes) as Arc<dyn crate::indexing::host::volumes::VolumeProvider>);
    let sched = MediaScheduler::new(data.path().to_path_buf(), fake_backend());

    let drive = "media-network-pass-local-drive";
    volumes.register(drive, Arc::new(InMemoryVolume::new("Stick")));
    network::config::set_opted_in(drive, true);
    let _drive_index = reserve_initializing_index_for_test(drive, IndexVolumeKind::LocalExternal);

    assert!(
        matches!(sched.run_network_pass_blocking(drive), Ok(PassOutcome::Done(0))),
        "a local drive's network pass enriches nothing"
    );
    assert!(
        !media_db_path(data.path(), drive).exists(),
        "and never gets as far as the drive's media store"
    );

    let share = "media-network-pass-share";
    volumes.register(share, Arc::new(InMemoryVolume::new("Share")));
    network::config::set_opted_in(share, true);
    let _share_index = reserve_initializing_index_for_test(share, IndexVolumeKind::Smb);

    assert!(sched.run_network_pass_blocking(share).is_ok());
    wait_until(
        Duration::from_secs(10),
        "the share's pass to record its mount root",
        || !read_mount_roots(&media_db_path(data.path(), share)).is_empty(),
    );

    for volume_id in [drive, share] {
        stop_indexing(volume_id).expect("stop the test index");
    }
    reset_gate();
}
