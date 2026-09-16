//! The delete gates: a reconcile may only reap the rows its listing didn't mention
//! when the observation behind it was WHOLE — the listing saw everything, and the
//! drive was still listed after the read.
//!
//! The control test is load-bearing. A gate that refused every delete would pass the
//! other tests here and quietly stop the index ever converging, so each "reaps
//! nothing" case is paired with a "still reaps" one on a healthy drive.

use super::*;
use crate::indexing::hold::VolumeWork;
use crate::indexing::host::volumes::{FakeVolumeProvider, MountIdentity, VolumeProvider, install_for_test};
use crate::indexing::metadata::MetadataSnapshot;
use std::sync::Arc;

/// The filesystem the drive in these tests is.
const DRIVE: MountIdentity = MountIdentity::from_raw(0x0DE1_E7E5);

/// A tree with two files, indexed, and work whose generation captured the drive's
/// identity — the shape a real `LocalExternal` start leaves behind.
///
/// Returns the provider (so a test can pull the drive), the work, and the installed
/// guard, which must stay alive for the test's duration.
struct Drive {
    provider: Arc<FakeVolumeProvider>,
    work: VolumeWork,
    _installed: crate::indexing::host::volumes::TestProviderGuard,
}

fn mount_a_drive(root: &Path, volume_id: &str) -> Drive {
    let provider = FakeVolumeProvider::shared();
    provider.mount(root.to_path_buf(), DRIVE);
    let installed = install_for_test(Arc::clone(&provider) as Arc<dyn VolumeProvider>);
    Drive {
        provider,
        work: VolumeWork::for_test_on(volume_id, DRIVE),
        _installed: installed,
    }
}

/// The names the index holds under `dir`.
fn indexed_children(conn: &Connection, dir: &str) -> Vec<String> {
    let id = store::resolve_path(conn, dir)
        .expect("resolve the directory")
        .expect("the directory is indexed");
    let mut names: Vec<String> = IndexStore::list_children_on(id, conn)
        .expect("list the children")
        .into_iter()
        .map(|row| row.name)
        .collect();
    names.sort();
    names
}

/// Index `root`'s two files, then delete one on disk and reconcile again, reporting
/// what the index holds afterwards. `pull_the_drive` runs between the two passes.
fn reconcile_after_a_removal(volume_id: &str, pull_the_drive: bool) -> Vec<String> {
    let _serialized = crate::indexing::handle::test_lock();
    let dir = non_excluded_tempdir();
    let root = dir.path();
    std::fs::write(root.join("keep.txt"), b"keep").expect("write keep.txt");
    std::fs::write(root.join("gone.txt"), b"gone").expect("write gone.txt");

    let (writer, _db_dir, conn) = setup_test_writer();
    let root_str = root.to_string_lossy().to_string();
    ensure_path_in_db(&writer.db_path(), &root_str, &writer);
    let drive = mount_a_drive(root, volume_id);
    let space = IndexPathSpace::root();

    // First pass: a healthy drive fills the index.
    reconcile_subtree(root, &space, &conn, &writer, &drive.work, None).expect("the first reconcile walks");
    writer.flush_blocking().expect("flush the first pass");
    assert_eq!(
        indexed_children(&conn, &root_str),
        vec!["gone.txt".to_string(), "keep.txt".to_string()],
        "precondition: both files are indexed"
    );

    std::fs::remove_file(root.join("gone.txt")).expect("remove gone.txt");
    if pull_the_drive {
        // ⚠️ AFTER the removal and before the second pass: the directory still lists
        // fine (its mount-point folder is a real temp dir), so only the presence read
        // can tell that the drive is gone.
        drive.provider.mark_unmounted(root);
    }

    reconcile_subtree(root, &space, &conn, &writer, &drive.work, None).expect("the second reconcile walks");
    writer.flush_blocking().expect("flush the second pass");
    indexed_children(&conn, &root_str)
}

/// The control: on a drive that is still there, a file that really was removed is
/// still reaped. Without this the gate could refuse every delete and the rest of
/// this file would still pass.
#[test]
fn a_reconcile_on_a_listed_drive_still_reaps_a_removed_row() {
    assert_eq!(
        reconcile_after_a_removal("gate-test-listed", false),
        vec!["keep.txt".to_string()],
        "a real removal on a healthy drive is still a delete"
    );
}

/// The case the gate exists for: the drive left between the two passes, so the
/// second listing is not evidence of anything and no row may be reaped from it.
#[test]
fn a_reconcile_whose_drive_left_reaps_nothing() {
    let mut names = reconcile_after_a_removal("gate-test-left", true);
    names.sort();
    assert_eq!(
        names,
        vec!["gone.txt".to_string(), "keep.txt".to_string()],
        "a drive that stopped being listed authorizes no deletes, so both rows survive"
    );
}

/// A listing that came back SHORT reaps nothing, and still writes everything it did
/// see: the upserts are driven by what was observed, only the reaping needs a whole
/// observation.
#[test]
fn an_incomplete_listing_reaps_nothing_and_still_upserts() {
    let (writer, db_dir, conn) = setup_test_writer();
    let db_path = db_dir.path().join("test-reconciler.db");
    let parent_id = IndexStore::insert_entry_v2(&conn, ROOT_ID, "short", true, false, None, None, None, None)
        .expect("insert the parent");
    let stale_id = IndexStore::insert_entry_v2(&conn, parent_id, "stale.txt", false, false, Some(1), None, None, None)
        .expect("insert the row the listing won't mention");
    let next_id = IndexStore::get_next_id(&conn).expect("next id");
    writer.next_id().fetch_max(next_id, Ordering::Relaxed);

    let seen = LiveChild {
        name: "fresh.txt".to_string(),
        is_directory: false,
        is_symlink: false,
        snap: MetadataSnapshot {
            logical_size: Some(7),
            physical_size: Some(7),
            modified_at: Some(1),
            inode: None,
            nlink: Some(1),
        },
    };
    let db_children = IndexStore::list_children_on(parent_id, &conn).expect("list the parent");

    // Bound first: `DirDiff` borrows the listing it was diffed against.
    let live = [seen];
    let diff = diff_dir_against_db(parent_id, &live, &db_children, MissingRows::Keep, &writer);
    writer.flush_blocking().expect("flush the diff");

    assert_eq!(diff.removed, 0, "a short listing reaps nothing");
    let conn = IndexStore::open_read_connection(&db_path).expect("read connection");
    let mut names: Vec<String> = IndexStore::list_children_on(parent_id, &conn)
        .expect("list the parent again")
        .into_iter()
        .map(|row| row.name)
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["fresh.txt".to_string(), "stale.txt".to_string()],
        "the row it DID see is written, and the one it couldn't vouch for is kept"
    );
    assert!(
        IndexStore::get_entry_by_id(&conn, stale_id)
            .expect("read the stale row")
            .is_some(),
        "the unmentioned row is still there by id"
    );
}

/// What makes a listing whole, decided on the ERRNO and ❌ never on a message.
///
/// A child that vanished between `readdir` and `stat` is genuinely absent, so the
/// listing still saw everything there is to see and may be diffed for deletes.
/// Every other failure means we never got to look at that child, which must not read
/// as "it's gone".
#[test]
fn only_a_gone_child_keeps_a_listing_whole() {
    use std::io::{Error, ErrorKind};

    assert!(
        child_is_absent(&Error::from(ErrorKind::NotFound)),
        "removed between readdir and stat: really absent"
    );
    assert!(
        child_is_absent(&Error::from(ErrorKind::NotADirectory)),
        "a path component stopped being a directory: the child isn't there either"
    );
    for unobserved in [
        ErrorKind::PermissionDenied,
        ErrorKind::TimedOut,
        ErrorKind::Interrupted,
        ErrorKind::Other,
    ] {
        assert!(
            !child_is_absent(&Error::from(unobserved)),
            "{unobserved:?} means we never looked, so the listing is short rather than complete"
        );
    }
}
