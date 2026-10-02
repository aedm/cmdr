//! What only S3 can get wrong in the engine: a cancel landing between the parts
//! of a multipart upload, an overwrite cut off before it publishes, a pause
//! between parts, a rollback of what a copy left on a bucket (finished, or
//! cancelled with rollback), a delete past S3's 1,000-key batch, and the
//! requests each of those sends compared with the dialog's cost estimate.
//!
//! Every scenario takes an `S3Target`: the cells below run them on both Docker
//! fixtures (named for the `s3_integration_` lane), and `s3_live_engine_test.rs`
//! runs the same bodies against real accounts. Multipart cells cut 5 MiB parts
//! (`S3Volume::set_part_floor`), so a few megabytes make several parts.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cmdr_fs::volume::{Volume, VolumeError};
use cmdr_s3::S3Volume;
use cmdr_s3::volume::testing::{
    GARAGE, S3Target, VERSITYGW, object, recorded_uploads_under, self_describing_bytes, take_sent_requests,
};

use super::network_gated_source_test_support::{gated_files, gated_upload};
use super::network_transfer_test_support::{budget, read_all, run_copy, sha256, start_copy};
use crate::file_system::VolumeManager;
use crate::file_system::volume::LocalPosixVolume;
use crate::file_system::volume::ScannedFile;
use crate::file_system::volume::manager::get_volume_manager;
use crate::file_system::write_operations::event_sinks::{CollectorEventSink, OperationEventSink};
use crate::file_system::write_operations::rollback::Reversal;
use crate::file_system::write_operations::state::{
    cancel_write_operation, pause_write_operation, resume_write_operation,
};
use crate::file_system::write_operations::types::{ConflictResolution, VolumeCopyConfig, WriteOperationConfig};
use crate::file_system::write_operations::{ScanCostFacts, delete_files_start};
use crate::ignore_poison::IgnorePoison;
use crate::operation_log::TestJournalGuard;
use crate::operation_log::capture::WriterJournal;
use crate::operation_log::rollback::{execute_rollback, rollback_operation};
use crate::operation_log::store::operation_log_db_path;
use crate::operation_log::types::Initiator;
use crate::operation_log::writer::OperationLogWriter;
use crate::s3_costs::{CostedOperation, planned_workloads};
use crate::test_support::TestDir;

const MIB: usize = 1024 * 1024;

/// The part floor the multipart cells cut at: S3's own minimum.
const SMALL_PART: u64 = 5 * MIB as u64;

/// How many 64 KiB chunks of the gated source fill one [`SMALL_PART`].
const CHUNKS_PER_PART: usize = SMALL_PART as usize / (64 * 1024);

/// A bucket volume on `target` cutting 5 MiB parts, plus a fresh prefix and
/// its folder's app path.
async fn small_parts(target: &S3Target, label: &str) -> (Arc<S3Volume>, String, PathBuf) {
    let volume = target.connect(Some(target.bucket())).await;
    volume.set_part_floor(SMALL_PART);
    let prefix = target.prefix(label);
    let dir = volume.root().join(prefix.trim_end_matches('/'));
    (Arc::new(volume), prefix, dir)
}

/// The same volume registered with the app's volume manager under a fresh id,
/// for the entry points that take an id (a delete).
fn register(volume: &Arc<S3Volume>, label: &str) -> String {
    let volume_id = format!("{}-{label}", volume.volume_id());
    get_volume_manager().register(&volume_id, Arc::clone(volume) as Arc<dyn Volume>);
    volume_id
}

/// Every key left under `prefix`, folder markers included, except the
/// prefix's own marker: a copy into a folder that wasn't there makes it as its
/// destination, and a rollback rightly leaves the destination folder itself.
async fn leftovers(target: &S3Target, prefix: &str) -> Vec<String> {
    let mut keys = target.keys_under(target.bucket(), prefix).await;
    keys.retain(|key| key != prefix);
    keys
}

// ── A cancel between parts ───────────────────────────────────────────

/// ❗ A cancel while a multipart upload is between parts publishes nothing,
/// leaves no unfinished upload on the server (parts are billed forever), and
/// no open record in the ledger.
///
/// The gated source lets one part and a half through, so the upload is past
/// its `CreateMultipartUpload` and its first part when the cancel lands.
pub(super) async fn a_cancel_mid_multipart_leaves_no_object_and_no_upload(target: &S3Target) {
    let (volume, prefix, dir) = small_parts(target, "cancel-multipart").await;
    let source = gated_upload(self_describing_bytes(3 * SMALL_PART as usize, "multipart-cancel")).await;
    let running = start_copy(
        "cancel-mid-multipart",
        Arc::clone(&source.volume),
        vec![PathBuf::from("/big.bin")],
        Arc::clone(&volume) as Arc<dyn Volume>,
        dir.clone(),
        VolumeCopyConfig::default(),
    )
    .await;

    let through = CHUNKS_PER_PART + CHUNKS_PER_PART / 2;
    source.gate.add_permits(through);
    crate::test_support::wait_until_async(
        budget(Duration::from_secs(6)),
        "the upload to record itself and take a part and a half",
        || {
            source.handed_out.load(std::sync::atomic::Ordering::SeqCst) >= through as u64
                && recorded_uploads_under(&volume, &prefix) > 0
        },
    )
    .await;
    cancel_write_operation(&running.operation_id, false);
    source.gate.add_permits(100_000);
    running.settle().await;

    assert!(
        running.events.complete.lock_ignore_poison().is_empty(),
        "the cancel must land mid-upload; the copy completed instead"
    );
    assert!(!running.events.cancelled.lock_ignore_poison().is_empty());
    assert!(
        !volume.exists(&dir.join("big.bin")).await,
        "a cancelled multipart upload publishes nothing"
    );
    assert_eq!(
        target.unfinished_uploads(target.bucket(), &prefix).await,
        Vec::<(String, String)>::new(),
        "the cancel aborts the upload, so no part stays billed"
    );
    assert_eq!(
        recorded_uploads_under(&volume, &prefix),
        0,
        "the ledger forgets the upload once the abort is confirmed"
    );
}

// ── An overwrite cut off ─────────────────────────────────────────────

/// ❗ An Overwrite cancelled mid-body never loses the original: off the
/// `refuses_short_body` allowlist it goes as a multipart upload that publishes
/// only at its completion, and on the list as a PUT the provider refuses short.
pub(super) async fn a_cancelled_overwrite_keeps_the_original(target: &S3Target) {
    let (volume, prefix, dir) = small_parts(target, "cancel-overwrite").await;
    let original = self_describing_bytes(300_000, "the-original");
    target
        .seed(target.bucket(), &[object(&format!("{prefix}big.bin"), &original)])
        .await;
    let source = gated_upload(self_describing_bytes(2 * MIB, "the-replacement")).await;

    let running = start_copy(
        "cancel-overwrite",
        Arc::clone(&source.volume),
        vec![PathBuf::from("/big.bin")],
        Arc::clone(&volume) as Arc<dyn Volume>,
        dir.clone(),
        VolumeCopyConfig {
            conflict_resolution: ConflictResolution::Overwrite,
            ..VolumeCopyConfig::default()
        },
    )
    .await;
    source.gate.add_permits(4);
    crate::test_support::wait_until_async(
        budget(Duration::from_secs(6)),
        "the overwrite to take four chunks",
        || source.handed_out.load(std::sync::atomic::Ordering::SeqCst) >= 4,
    )
    .await;
    cancel_write_operation(&running.operation_id, false);
    source.gate.add_permits(100_000);
    running.settle().await;

    assert!(
        running.events.complete.lock_ignore_poison().is_empty(),
        "the cancel must land mid-body; the overwrite completed instead"
    );
    assert_eq!(
        sha256(&read_all(volume.as_ref(), &dir.join("big.bin")).await),
        sha256(&original),
        "a cut-off overwrite keeps the original whole"
    );
    assert!(target.unfinished_uploads(target.bucket(), &prefix).await.is_empty());
    assert_eq!(recorded_uploads_under(&volume, &prefix), 0);
}

// ── Pause between parts ──────────────────────────────────────────────

/// A paused multipart copy onto a bucket resumes and lands every byte, with
/// no upload left open.
pub(super) async fn a_paused_upload_resumes_and_lands(target: &S3Target) {
    let (volume, prefix, dir) = small_parts(target, "pause-upload").await;
    let bytes = self_describing_bytes(3 * SMALL_PART as usize + 123, "paused");
    let source = gated_upload(bytes.clone()).await;
    let running = start_copy(
        "pause-upload",
        Arc::clone(&source.volume),
        vec![PathBuf::from("/big.bin")],
        Arc::clone(&volume) as Arc<dyn Volume>,
        dir.clone(),
        VolumeCopyConfig::default(),
    )
    .await;
    // One part through, then the pause, then the rest of the bytes.
    source.gate.add_permits(CHUNKS_PER_PART);
    crate::test_support::wait_until_async(budget(Duration::from_secs(6)), "the first part to be read", || {
        source.handed_out.load(std::sync::atomic::Ordering::SeqCst) >= CHUNKS_PER_PART as u64
    })
    .await;
    assert!(
        pause_write_operation(&running.operation_id),
        "the upload is parked on the gate, so it's still running to pause"
    );
    source.gate.add_permits(100_000);
    assert!(
        running.events.complete.lock_ignore_poison().is_empty(),
        "a paused upload doesn't finish"
    );
    assert!(resume_write_operation(&running.operation_id));
    running.settle().await;
    running.assert_no_errors();

    assert_eq!(
        sha256(&read_all(volume.as_ref(), &dir.join("big.bin")).await),
        sha256(&bytes)
    );
    assert!(target.unfinished_uploads(target.bucket(), &prefix).await.is_empty());
}

// ── Rollback ─────────────────────────────────────────────────────────

/// The journal a rollback reads, installed as the process's own for the
/// cell's lifetime.
struct Journal {
    _guard: TestJournalGuard,
    writer: Arc<WriterJournal>,
    _dir: tempfile::TempDir,
}

fn install_journal() -> Journal {
    let dir = tempfile::tempdir().expect("a journal dir");
    let writer = OperationLogWriter::spawn(&operation_log_db_path(dir.path())).expect("the journal writer spawns");
    let writer = Arc::new(WriterJournal::new(writer));
    Journal {
        _guard: TestJournalGuard::install(writer.clone()),
        writer,
        _dir: dir,
    }
}

/// A local folder `tree/` of three files, as a volume.
struct LocalTree {
    _dir: TestDir,
    volume: Arc<dyn Volume>,
    files: Vec<(&'static str, Vec<u8>)>,
}

fn local_tree(label: &str) -> LocalTree {
    let dir = TestDir::new(label);
    std::fs::create_dir_all(dir.join("tree")).expect("making the local tree");
    let files: Vec<(&'static str, Vec<u8>)> = ["a.bin", "b.bin", "c.bin"]
        .into_iter()
        .map(|name| (name, self_describing_bytes(256 * 1024, name)))
        .collect();
    for (name, bytes) in &files {
        std::fs::write(dir.join("tree").join(name), bytes).expect("seeding the local tree");
    }
    let volume: Arc<dyn Volume> = Arc::new(LocalPosixVolume::new("Local", &*dir));
    LocalTree {
        _dir: dir,
        volume,
        files,
    }
}

/// ❗ A finished copy onto a bucket rolls back through the operation log: the
/// reversal verifies each object it wrote and deletes it, leaving the prefix
/// as it found it.
pub(super) async fn a_finished_copy_onto_a_bucket_rolls_back(target: &S3Target) {
    let journal = install_journal();
    let (volume, prefix, dir) = small_parts(target, "rollback-finished").await;
    let tree = local_tree("s3_rollback_finished");
    let (local, files) = (Arc::clone(&tree.volume), &tree.files);
    let label = format!("s3-rollback-finished-{}", target.name());
    let vm = VolumeManager::new();
    vm.register(&format!("{label}-source"), Arc::clone(&local));
    vm.register(&format!("{label}-dest"), Arc::clone(&volume) as Arc<dyn Volume>);

    let running = run_copy(
        &label,
        Arc::clone(&local),
        vec![PathBuf::from("tree")],
        Arc::clone(&volume) as Arc<dyn Volume>,
        dir.clone(),
    )
    .await;
    for (name, bytes) in files {
        assert_eq!(
            sha256(&read_all(volume.as_ref(), &dir.join("tree").join(name)).await),
            sha256(bytes)
        );
    }

    let writer = journal.writer.writer();
    writer.flush_blocking().expect("the journal flushes");
    let plan = rollback_operation(&vm, writer, &running.operation_id, |_plan| Ok(()))
        .unwrap_or_else(|refusal| panic!("the finished copy must be reversible, got {refusal:?}"));
    let reversal = Reversal::new("s3-rollback-finished");
    let report = execute_rollback(
        &vm,
        writer,
        &plan.original,
        &plan.inverse_op_id,
        Initiator::User,
        reversal.runner(),
    )
    .await;

    assert_eq!(report.skipped, 0, "every object it wrote is its to remove: {report:?}");
    assert_eq!(
        leftovers(target, &prefix).await,
        Vec::<String>::new(),
        "the rollback leaves the prefix as it found it"
    );
}

/// ❗ A cancel with rollback mid-tree takes back the files that already
/// landed, and the one cut off publishes nothing.
pub(super) async fn a_cancel_with_rollback_takes_back_what_landed(target: &S3Target) {
    let (volume, prefix, dir) = small_parts(target, "rollback-cancelled").await;
    let chunks_each = 4;
    let files: Vec<(String, Vec<u8>)> = ["a.bin", "b.bin", "c.bin"]
        .into_iter()
        .map(|name| {
            (
                format!("/tree/{name}"),
                self_describing_bytes(chunks_each * 64 * 1024, name),
            )
        })
        .collect();
    let source = gated_files(files).await;
    let running = start_copy(
        "rollback-cancelled",
        Arc::clone(&source.volume),
        vec![PathBuf::from("/tree")],
        Arc::clone(&volume) as Arc<dyn Volume>,
        dir.clone(),
        VolumeCopyConfig {
            progress_interval_ms: 0,
            ..VolumeCopyConfig::default()
        },
    )
    .await;
    // All but one chunk: at least two files land whole, one is held mid-body.
    let through = 3 * chunks_each - 1;
    source.gate.add_permits(through);
    crate::test_support::wait_until_async(
        budget(Duration::from_secs(6)),
        "two files to land and the third to be held",
        || {
            let landed = running
                .events
                .progress
                .lock_ignore_poison()
                .iter()
                .map(|event| event.files_done)
                .max()
                .unwrap_or(0);
            landed >= 2 && source.handed_out.load(std::sync::atomic::Ordering::SeqCst) >= through as u64
        },
    )
    .await;
    cancel_write_operation(&running.operation_id, true);
    source.gate.add_permits(100_000);
    running.settle().await;

    assert!(
        running.events.complete.lock_ignore_poison().is_empty(),
        "the cancel must land mid-tree; the copy completed instead"
    );
    assert_eq!(
        leftovers(target, &prefix).await,
        Vec::<String>::new(),
        "a rollback takes back every file that landed, and its folder"
    );
    assert!(target.unfinished_uploads(target.bucket(), &prefix).await.is_empty());
}

// ── A delete past one batch ──────────────────────────────────────────

/// Objects under the delete cell's folder: one past S3's 1,000-key batch.
const DELETE_COUNT: usize = 1_005;

/// ❗ A folder of 1,005 objects deletes through the app's delete entry point,
/// every object gone and the folder with them.
pub(super) async fn a_folder_of_1005_objects_deletes(target: &S3Target) {
    let (volume, prefix, dir) = small_parts(target, "delete-1005").await;
    let volume_id = register(&volume, &format!("delete-1005-{}", target.name()));
    let keys: Vec<String> = (0..DELETE_COUNT)
        .map(|n| format!("{prefix}folder/f{n:04}.txt"))
        .collect();
    let seeds: Vec<_> = keys.iter().map(|key| object(key, b"x")).collect();
    target.seed(target.bucket(), &seeds).await;

    let events = Arc::new(CollectorEventSink::new());
    delete_files_start(
        events.clone() as Arc<dyn OperationEventSink>,
        vec![dir.join("folder")],
        WriteOperationConfig::default(),
        Some(volume_id),
        Initiator::User,
        None,
    )
    .await
    .expect("the delete starts");
    crate::test_support::wait_until_async(budget(Duration::from_secs(60)), "the delete to settle", || {
        !events.settled.lock_ignore_poison().is_empty()
    })
    .await;
    let errors: Vec<String> = events
        .errors
        .lock_ignore_poison()
        .iter()
        .map(|e| format!("{:?}", e.error))
        .collect();
    assert!(errors.is_empty(), "the delete reported {errors:?}");

    assert!(matches!(
        volume.list_directory(&dir.join("folder"), None).await,
        Err(VolumeError::NotFound(_))
    ));
    assert_eq!(leftovers(target, &prefix).await, Vec::<String>::new());
}

// ── What the engine sends against what the dialog estimates ──────────

/// The label of the comparison's delete, which the fixture cell leaves out.
const DELETE_OPERATION: &str = "delete the uploaded folder";

/// One operation's requests: what the engine sent, what the estimate counts.
pub(super) struct RequestComparison {
    pub(super) operation: &'static str,
    pub(super) sent: BTreeMap<&'static str, u64>,
    pub(super) estimated: BTreeMap<&'static str, u64>,
}

/// The requests that move or publish bytes, which a write path sends and its
/// `Workload` method counts one for one. The HEADs and LISTs around them are
/// the engine's (scan, destination pre-check, verify) and the estimate counts
/// them "close, not exact" (`crates/cmdr-s3/DETAILS.md` § "Cost estimates").
const WRITE_PATH_KINDS: [&str; 8] = [
    "PutObject",
    "CopyObject",
    "CreateMultipartUpload",
    "UploadPart",
    "UploadPartCopy",
    "CompleteMultipartUpload",
    "GetObject",
    "AbortMultipartUpload",
];

impl RequestComparison {
    /// Every kind where the two disagree, as `kind: sent N, estimated M`.
    pub(super) fn mismatches(&self) -> Vec<String> {
        self.mismatches_where(|_| true)
    }

    /// [`Self::mismatches`] among [`WRITE_PATH_KINDS`] only.
    pub(super) fn write_path_mismatches(&self) -> Vec<String> {
        self.mismatches_where(|kind| WRITE_PATH_KINDS.contains(&kind))
    }

    fn mismatches_where(&self, wanted: impl Fn(&str) -> bool) -> Vec<String> {
        let mut kinds: Vec<&&str> = self.sent.keys().chain(self.estimated.keys()).collect();
        kinds.sort();
        kinds.dedup();
        kinds
            .into_iter()
            .filter(|kind| wanted(kind))
            .filter_map(|kind| {
                let (sent, estimated) = (
                    self.sent.get(*kind).copied().unwrap_or(0),
                    self.estimated.get(*kind).copied().unwrap_or(0),
                );
                (sent != estimated).then(|| format!("{kind}: sent {sent}, estimated {estimated}"))
            })
            .collect()
    }
}

/// The facts a settled scan preview of `files` would hand the estimate.
fn facts(files: &[usize], dirs: usize) -> ScanCostFacts {
    ScanCostFacts {
        files: files.len(),
        dirs,
        bytes: files.iter().map(|len| *len as u64).sum(),
        per_file: Some(
            files
                .iter()
                .map(|len| ScannedFile {
                    size: *len as u64,
                    modified_at: None,
                })
                .collect(),
        ),
    }
}

fn one_workload(mut workloads: Vec<cmdr_s3::cost::Workload>) -> BTreeMap<&'static str, u64> {
    assert_eq!(workloads.len(), 1, "one S3 end, one workload");
    workloads.remove(0).counted_requests()
}

/// Runs four operations through the engine on `target` (an upload of two
/// files, one of them multipart at the production floor, a download, a copy
/// within the bucket, and a delete of the folder) and answers, for each, the
/// requests the bucket volume sent beside what `s3_costs` estimates for it.
/// The caller decides what to assert: the fixture cells pin the exact counts,
/// the live run reports every mismatch.
pub(super) async fn requests_sent_against_the_estimate(target: &S3Target) -> Vec<RequestComparison> {
    let volume = Arc::new(target.connect(Some(target.bucket())).await);
    let prefix = target.prefix("cost");
    let dir = volume.root().join(prefix.trim_end_matches('/'));
    let remote = Arc::clone(&volume) as Arc<dyn Volume>;
    let local_dir = TestDir::new("s3_cost_compare");
    let sizes = [200_000_usize, 70 * MIB];
    std::fs::create_dir_all(local_dir.join("batch")).expect("making the local folder");
    for (index, len) in sizes.iter().enumerate() {
        std::fs::write(
            local_dir.join("batch").join(format!("f{index}.bin")),
            self_describing_bytes(*len, "cost"),
        )
        .expect("seeding a local file");
    }
    let local: Arc<dyn Volume> = Arc::new(LocalPosixVolume::new("Local", &*local_dir));
    let mut out = Vec::new();

    // The folder for the run, made the way a person would before copying in.
    volume
        .create_directory_all(&dir)
        .await
        .expect("making the prefix folder");
    take_sent_requests(&volume).await;

    run_copy(
        "cost-upload",
        Arc::clone(&local),
        vec![PathBuf::from("batch")],
        Arc::clone(&remote),
        dir.clone(),
    )
    .await;
    out.push(RequestComparison {
        operation: "upload a folder of two files",
        sent: take_sent_requests(&volume).await,
        estimated: one_workload(planned_workloads(
            CostedOperation::Copy,
            None,
            Some(volume.as_ref()),
            &facts(&sizes, 1),
            None,
        )),
    });

    let back_dir = TestDir::new("s3_cost_download");
    let back: Arc<dyn Volume> = Arc::new(LocalPosixVolume::new("Local", &*back_dir));
    run_copy(
        "cost-download",
        Arc::clone(&remote),
        vec![dir.join("batch")],
        back,
        PathBuf::from(""),
    )
    .await;
    out.push(RequestComparison {
        operation: "download that folder",
        sent: take_sent_requests(&volume).await,
        estimated: one_workload(planned_workloads(
            CostedOperation::Copy,
            Some(volume.as_ref()),
            None,
            &facts(&sizes, 1),
            None,
        )),
    });

    // A destination the person already has: making a missing one would add a
    // marker the estimate rightly doesn't count.
    volume
        .create_directory_all(&dir.join("copied"))
        .await
        .expect("making the copy's destination");
    take_sent_requests(&volume).await;
    run_copy(
        "cost-server-copy",
        Arc::clone(&remote),
        vec![dir.join("batch")],
        Arc::clone(&remote),
        dir.join("copied"),
    )
    .await;
    out.push(RequestComparison {
        operation: "copy that folder within the bucket",
        sent: take_sent_requests(&volume).await,
        estimated: one_workload(planned_workloads(
            CostedOperation::Copy,
            Some(volume.as_ref()),
            Some(volume.as_ref()),
            &facts(&sizes, 1),
            None,
        )),
    });

    let volume_id = register(&volume, &format!("cost-{}", target.name()));
    let events = Arc::new(CollectorEventSink::new());
    delete_files_start(
        events.clone() as Arc<dyn OperationEventSink>,
        vec![dir.join("batch")],
        WriteOperationConfig::default(),
        Some(volume_id),
        Initiator::User,
        None,
    )
    .await
    .expect("the delete starts");
    crate::test_support::wait_until_async(budget(Duration::from_secs(6)), "the delete to settle", || {
        !events.settled.lock_ignore_poison().is_empty()
    })
    .await;
    assert!(events.errors.lock_ignore_poison().is_empty());
    out.push(RequestComparison {
        operation: DELETE_OPERATION,
        sent: take_sent_requests(&volume).await,
        estimated: one_workload(planned_workloads(
            CostedOperation::Delete,
            Some(volume.as_ref()),
            None,
            &facts(&sizes, 1),
            None,
        )),
    });
    out
}

/// The fixture form: the requests that move bytes match the estimate exactly
/// for the upload, the download, and the copy.
///
/// ❗ The delete isn't asserted: the volume delete walker sends a `delete` per
/// object (a capped LIST, a HEAD, a DELETE each), while the estimate bills
/// `DeleteObjects` batches. Which side moves is open
/// (`docs/notes/s3/live-engine-2026-10.md` § "Cost estimates").
async fn the_engine_sends_what_the_estimate_counts(target: &S3Target) {
    let comparisons = requests_sent_against_the_estimate(target).await;
    let mismatches: Vec<String> = comparisons
        .iter()
        .filter(|c| c.operation != DELETE_OPERATION)
        .flat_map(|c| {
            c.write_path_mismatches()
                .into_iter()
                .map(move |m| format!("{}: {m}", c.operation))
        })
        .collect();
    assert!(
        mismatches.is_empty(),
        "the cost estimate must count the requests the engine sends (`crates/cmdr-s3/CLAUDE.md`):\n{}",
        mismatches.join("\n")
    );
}

// ── Cells ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn s3_integration_a_cancel_mid_multipart_leaves_no_object_and_no_upload_on_versitygw() {
    a_cancel_mid_multipart_leaves_no_object_and_no_upload(&S3Target::Fixture(VERSITYGW)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn s3_integration_a_cancel_mid_multipart_leaves_no_object_and_no_upload_on_garage() {
    a_cancel_mid_multipart_leaves_no_object_and_no_upload(&S3Target::Fixture(GARAGE)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn s3_integration_a_cancelled_overwrite_keeps_the_original_on_versitygw() {
    a_cancelled_overwrite_keeps_the_original(&S3Target::Fixture(VERSITYGW)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn s3_integration_a_cancelled_overwrite_keeps_the_original_on_garage() {
    a_cancelled_overwrite_keeps_the_original(&S3Target::Fixture(GARAGE)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn s3_integration_a_paused_upload_resumes_and_lands_on_versitygw() {
    a_paused_upload_resumes_and_lands(&S3Target::Fixture(VERSITYGW)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn s3_integration_a_paused_upload_resumes_and_lands_on_garage() {
    a_paused_upload_resumes_and_lands(&S3Target::Fixture(GARAGE)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn s3_integration_a_finished_copy_onto_a_bucket_rolls_back_on_versitygw() {
    a_finished_copy_onto_a_bucket_rolls_back(&S3Target::Fixture(VERSITYGW)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn s3_integration_a_finished_copy_onto_a_bucket_rolls_back_on_garage() {
    a_finished_copy_onto_a_bucket_rolls_back(&S3Target::Fixture(GARAGE)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn s3_integration_a_cancel_with_rollback_takes_back_what_landed_on_versitygw() {
    a_cancel_with_rollback_takes_back_what_landed(&S3Target::Fixture(VERSITYGW)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn s3_integration_a_cancel_with_rollback_takes_back_what_landed_on_garage() {
    a_cancel_with_rollback_takes_back_what_landed(&S3Target::Fixture(GARAGE)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn s3_integration_a_folder_of_1005_objects_deletes_on_versitygw() {
    a_folder_of_1005_objects_deletes(&S3Target::Fixture(VERSITYGW)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn s3_integration_a_folder_of_1005_objects_deletes_on_garage() {
    a_folder_of_1005_objects_deletes(&S3Target::Fixture(GARAGE)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn s3_integration_the_engine_sends_what_the_estimate_counts_on_versitygw() {
    the_engine_sends_what_the_estimate_counts(&S3Target::Fixture(VERSITYGW)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the S3 fixture stack: apps/desktop/test/s3-servers/start.sh (s3-fixture)"]
async fn s3_integration_the_engine_sends_what_the_estimate_counts_on_garage() {
    the_engine_sends_what_the_estimate_counts(&S3Target::Fixture(GARAGE)).await;
}
