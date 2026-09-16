//! Cmdr's own eject, per PHYSICAL disk: one flight for the whole disk, every volume
//! on it gated and stopped before anything unmounts, and everything it stopped handed
//! back when the disk stays.
//!
//! **Why per disk.** `diskutil eject` of one volume takes the whole disk down, so a
//! sibling partition or APFS volume Cmdr indexes would meet the unmount with a live
//! FSEvents watcher on it, which is the FSKit wedge the pre-stop exists to avoid. And
//! a disk that refuses because a sibling is held used to read as a clean eject
//! (`real_image.rs` pinned it), leaving the drive powered on.
//!
//! The order is the one in `volume/DETAILS.md` § "Eject": resolve the disk, capture
//! its registered volumes, adopt them into the ejecting set, the busy gate, one
//! release under one deadline, the teardown, then the resume. ❌ Nothing unmounts
//! after a step before it stalled or refused.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::disk_target::{self, DiskMounts, DiskTarget};
use super::in_flight::{self, DiskFlight, DiskOwnership};
use super::unmount_tool::{AbandonedRun, UnmountVerb};
use super::{EjectError, EjectStep, IndexStopped, Teardown, deadlines, run_teardown, stop_indexes_blocking};
use crate::file_system::volume::drive_release::{
    self, DriveRelease, LateRelease, Release, ResumeCandidate, ResumeOwner, VolumeRelease,
};
use crate::volumes::disk_units::MountedVolume;

/// A registered volume of the disk being ejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Sibling {
    pub(super) volume_id: String,
    /// Where it was mounted when the flight captured it. ❗ Captured, because once the
    /// disk is down nothing can look up where its volumes were.
    pub(super) path: PathBuf,
}

/// Ejects the physical disk under `volume_id`, which is mounted at `mount_path`.
pub(super) async fn eject_disk(volume_id: &str, mount_path: &str, target: DiskTarget) -> Result<(), EjectError> {
    // ❗ Fail closed: without the disk's volumes nobody knows which siblings the
    // teardown would take down, so stopping only the volume that was clicked and
    // unmounting anyway is exactly the live-watcher unmount the pre-stop exists to
    // avoid. ❌ An unreadable answer is never an empty disk.
    let DiskMounts::Read(mounted) = disk_target::mounted_volumes_on_disk(&target.units) else {
        log::warn!(
            target: "eject",
            "DiskArbitration wouldn't say what's mounted on {volume_id}'s disk, so nothing is unmounted"
        );
        return Err(EjectError::NotResponding {
            step: EjectStep::DiskResolve,
        });
    };
    let siblings = capture(volume_id, Path::new(mount_path), &mounted, |path| {
        let (id, volume) = crate::file_system::volume::manager::get_volume_manager().find_by_root(path)?;
        (volume.root() == path).then_some(id)
    });
    let ids: Vec<String> = siblings.iter().map(|sibling| sibling.volume_id.clone()).collect();

    // ❗ Before the busy gate and everything after it: from here a sibling's own eject
    // joins this flight in `join_or_start` rather than running a teardown of its own.
    let ownership = match in_flight::join_or_own_disk(target.key, volume_id, &ids) {
        DiskFlight::Joined(running) => return running.await,
        DiskFlight::Owner(ownership) => ownership,
    };
    if !ownership.adopted().is_empty() {
        log::info!(
            target: "eject",
            // allowed-pluralize-noun: a log line, and `{:?}` is a list of sibling volume ids, never a count
            "Ejecting {volume_id} takes its whole disk down, so {:?} come with it",
            ownership.adopted()
        );
    }

    let busy = crate::file_system::busy_volume_ids();
    if let Some(volume_id) = busy_sibling(&ids, &busy) {
        log::info!(target: "eject", "Not ejecting the disk under {mount_path}: a write op is busy on {volume_id}");
        return Err(EjectError::Busy);
    }

    eject_stopped_disk(
        drive_release::gate(),
        volume_id,
        mount_path,
        &target,
        &siblings,
        ownership,
    )
    .await
}

/// Stop every sibling, tear the disk down, and hand back what stayed.
async fn eject_stopped_disk(
    gate: &'static DriveRelease,
    volume_id: &str,
    mount_path: &str,
    target: &DiskTarget,
    siblings: &[Sibling],
    ownership: DiskOwnership,
) -> Result<(), EjectError> {
    let owner = Arc::new(FlightResume {
        flight_id: ownership.flight_id(),
        paths: siblings
            .iter()
            .map(|sibling| (sibling.volume_id.clone(), sibling.path.clone()))
            .collect(),
    });
    let ids: Vec<String> = siblings.iter().map(|sibling| sibling.volume_id.clone()).collect();

    // A sibling whose stop lands after the deadline is handed back by its own
    // continuation: nothing unmounted, so it's owed its index either way.
    let record_late = {
        let owner = Arc::clone(&owner);
        move |late: LateRelease| {
            log::info!(
                target: "eject",
                "The index for {} answered after the eject stopped waiting: {:?}",
                late.volume_id,
                late.outcome
            );
            if late.outcome == (VolumeRelease::Released { was_indexing: true }) {
                let candidate = ResumeCandidate {
                    volume_id: late.volume_id.clone(),
                    epoch: late.epoch,
                };
                drop(gate.resume(vec![candidate], Arc::clone(&owner) as Arc<dyn ResumeOwner>));
            }
        }
    };
    let stopped = deadlines::within_deadline(
        EjectStep::IndexStop,
        volume_id,
        super::INDEX_STOP_DEADLINE,
        stop_indexes_blocking(ids, super::stop_removable_index, record_late),
    )
    .await?;
    let release = match stopped {
        IndexStopped::LetGo(release) => release,
        IndexStopped::Refused { release, error } => {
            // ❌ Nothing unmounts: a drive an index may still hold is the wedge-prone
            // case. What DID let go is owed back, at the epochs that release set.
            if let Some(release) = release {
                hand_back(gate, &owner, owed_ids(&release), |id| epoch_of(&release, id));
            }
            return Err(error);
        }
    };

    let teardown = DiskTeardown::new(captured_paths(&mounted_now(target), siblings), {
        let units = target.units.clone();
        move || anything_mounted_on(&units)
    });
    let result = run_teardown(
        volume_id,
        Teardown::Tool {
            verb: UnmountVerb::Eject,
            mount_path,
            disk: Some(&teardown),
        },
    )
    .await;

    let owed = owed_ids(&release);
    match &result {
        // The disk is gone: nothing to hand back, and its volumes go with it.
        Ok(()) => {}
        Err(EjectError::TimedOut) => resume_when_the_tool_ends(gate, &owner, owed, teardown, volume_id),
        // ❗ Read NOW, not at the pre-stop: this flight's own `diskutil eject` triggers
        // unmount approvals whose release moves every sibling's epoch, and a resume
        // recorded against the older one could only fail its check.
        Err(_) => hand_back(gate, &owner, owed, |id| gate.epoch(id)),
    }
    drop(ownership);
    result
}

/// The disk's mounted volumes right now, for the capture the teardown checks against.
/// An unreadable answer contributes nothing: the captured sibling roots still stand,
/// and [`anything_mounted_on`] is what keeps the teardown from calling the disk gone.
fn mounted_now(target: &DiskTarget) -> Vec<MountedVolume> {
    match disk_target::mounted_volumes_on_disk(&target.units) {
        DiskMounts::Read(mounted) => mounted,
        DiskMounts::Unreadable => Vec::new(),
    }
}

/// Whether anything is still mounted on the disk. ❗ An unreadable answer counts as
/// YES: reading a silent DiskArbitration as an empty disk would answer `Ok` for an
/// eject that left the drive powered on.
fn anything_mounted_on(units: &[u32]) -> bool {
    reads_as_mounted(disk_target::mounted_volumes_on_disk(units))
}

/// What a disk read means for "is anything still mounted". Pure, so the three answers
/// are pinned without a DiskArbitration session.
fn reads_as_mounted(mounts: DiskMounts) -> bool {
    match mounts {
        DiskMounts::Read(mounted) => !mounted.is_empty(),
        DiskMounts::Unreadable => {
            log::warn!(
                target: "eject",
                "Nobody could say what's still mounted on the disk, so the eject reads it as still up"
            );
            true
        }
    }
}

/// Every mount the teardown has to see gone: the disk's mounted volumes, plus every
/// sibling's captured root (a volume whose mount left the table between the capture
/// and here is already gone, and one the fresh read missed still counts).
pub(super) fn captured_paths(mounted: &[MountedVolume], siblings: &[Sibling]) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = siblings.iter().map(|sibling| sibling.path.clone()).collect();
    for volume in mounted {
        if !paths.contains(&volume.path) {
            paths.push(volume.path.clone());
        }
    }
    paths
}

/// Every registered volume of the disk, the one the person asked about first.
///
/// ❗ By ACTIVE root: a volume reachable through a spare mount keeps working when that
/// mount goes, so a mount that isn't its current root names nobody.
pub(super) fn capture(
    volume_id: &str,
    mount_path: &Path,
    mounted: &[MountedVolume],
    volume_at: impl Fn(&Path) -> Option<String>,
) -> Vec<Sibling> {
    let mut siblings = vec![Sibling {
        volume_id: volume_id.to_string(),
        path: mount_path.to_path_buf(),
    }];
    for volume in mounted {
        let Some(volume_id) = volume_at(&volume.path) else {
            continue;
        };
        if siblings.iter().any(|sibling| sibling.volume_id == volume_id) {
            continue;
        }
        siblings.push(Sibling {
            volume_id,
            path: volume.path.clone(),
        });
    }
    siblings
}

/// A whole physical disk's teardown: where its volumes were mounted when the flight
/// captured them, and how to ask whether anything at all is still mounted on it.
///
/// ❗ A disk's `diskutil eject` can unmount some volumes and stop at one that's held
/// (verified on macOS 27.0, `diskutil unmountDisk` of a two-partition image with one
/// partition held, 2026-09-16), so "this volume's root is gone" would read that
/// partial unmount as done and leave the disk powered on.
pub(super) struct DiskTeardown {
    /// Every mounted volume of the disk when the flight captured it, its own first.
    captured: Vec<PathBuf>,
    /// Whether any volume the capture didn't name is still mounted on the disk.
    fresh_look: Box<dyn Fn() -> bool + Send + Sync>,
    abandoned: AbandonedRun,
}

impl DiskTeardown {
    pub(super) fn new(captured: Vec<PathBuf>, fresh_look: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self {
            captured,
            fresh_look: Box::new(fresh_look),
            abandoned: AbandonedRun::default(),
        }
    }

    /// Which path this attempt runs the tool against.
    pub(super) fn aim(&self, fallback: &str) -> String {
        aim_at(&self.captured, fallback, listed)
    }

    /// Where a run this teardown's timeout gave up on lands.
    pub(super) fn abandoned(&self) -> &AbandonedRun {
        &self.abandoned
    }

    /// Whether anything of the disk is still mounted.
    pub(super) fn is_still_mounted(&self) -> bool {
        disk_is_still_mounted(&self.captured, listed, &self.fresh_look)
    }
}

/// Whether `path` is still in the OS mount table.
fn listed(path: &Path) -> bool {
    super::is_still_mounted(&path.to_string_lossy())
}

/// Which of a disk's captured mounts the next attempt aims at: the first still listed,
/// or `fallback` when none is.
///
/// ❗ A retry aimed at the volume the person clicked reads a PARTIAL unmount as done,
/// since that one really did go. Aiming at a mount that's still there is what makes the
/// retry mean anything; with none left, `fallback` lets `diskutil` answer "no such
/// disk", which `unmount_tool::settle` counts as done once the table agrees.
fn aim_at(captured: &[PathBuf], fallback: &str, listed: impl Fn(&Path) -> bool) -> String {
    captured
        .iter()
        .find(|path| listed(path))
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| fallback.to_string())
}

/// Whether anything of a disk is still mounted: any captured mount still listed, or a
/// fresh look finding one the capture didn't name.
///
/// ❗ Fail closed. A volume mounted after the capture, or one that was never registered,
/// keeps the disk alive just as much, and reading the disk as gone would answer `Ok` for
/// an eject that left it powered on.
fn disk_is_still_mounted(captured: &[PathBuf], listed: impl Fn(&Path) -> bool, fresh_look: impl Fn() -> bool) -> bool {
    captured.iter().any(|path| listed(path)) || fresh_look()
}

/// The first volume of the disk a write op is busy on.
///
/// ❗ Every volume, not just the one clicked: the teardown takes the whole disk down,
/// so a transfer running on a sibling would be truncated just the same.
fn busy_sibling<'a>(ids: &'a [String], busy: &[String]) -> Option<&'a str> {
    ids.iter().find(|id| busy.contains(id)).map(String::as_str)
}

/// Every volume the release stopped that was indexing before it: what a flight owes
/// back when the disk stays. ❌ Nothing else: a volume with no index had nothing taken
/// from it, and one still releasing is handed back by its own continuation.
pub(super) fn owed_ids(release: &Release) -> Vec<String> {
    release
        .volumes
        .iter()
        .filter(|volume| volume.outcome == (VolumeRelease::Released { was_indexing: true }))
        .map(|volume| volume.volume_id.clone())
        .collect()
}

/// The epoch the release itself set for `volume_id`, for a hand-back with no unmount
/// in between.
pub(super) fn epoch_of(release: &Release, volume_id: &str) -> u64 {
    release
        .volumes
        .iter()
        .find(|volume| volume.volume_id == volume_id)
        .map_or(0, |volume| volume.epoch)
}

/// Hand `owed` back to the gate at `epoch`.
pub(super) fn hand_back(
    gate: &DriveRelease,
    owner: &Arc<FlightResume>,
    owed: Vec<String>,
    epoch: impl Fn(&str) -> u64,
) {
    if owed.is_empty() {
        return;
    }
    log::info!(target: "eject", "The disk stayed, so {owed:?} get their index back");
    let candidates = owed
        .into_iter()
        .map(|volume_id| ResumeCandidate {
            epoch: epoch(&volume_id),
            volume_id,
        })
        .collect();
    drop(gate.resume(candidates, Arc::clone(owner) as Arc<dyn ResumeOwner>));
}

/// A timed-out eject answers at once, so the hand-back waits for the tool's own end.
///
/// ❗ The unmount was NOT cancelled and may still land. Starting an index again before
/// the tool is done would put a watcher back on a drive that's going down; once it
/// ends, the owner's presence check drops whatever really went.
fn resume_when_the_tool_ends(
    gate: &'static DriveRelease,
    owner: &Arc<FlightResume>,
    owed: Vec<String>,
    teardown: DiskTeardown,
    volume_id: &str,
) {
    let Some(still_running) = teardown.abandoned().take() else {
        // The timeout came from somewhere other than a run of the tool, so there's
        // nothing left to wait for.
        hand_back(gate, owner, owed, |id| gate.epoch(id));
        return;
    };
    if owed.is_empty() {
        return;
    }
    let owner = Arc::clone(owner);
    let volume_id = volume_id.to_string();
    tauri::async_runtime::spawn(async move {
        match still_running.await {
            Ok(outcome) => {
                log::info!(target: "eject", "The eject tool for {volume_id} ended after the eject answered: {outcome}")
            }
            Err(join_err) => {
                log::warn!(target: "eject", "The eject tool for {volume_id} left no answer: {join_err}")
            }
        }
        hand_back(gate, &owner, owed, |id| gate.epoch(id));
    });
}

/// The flight as a resume owner: what it stopped it hands back, for as long as no
/// other eject has taken the volume over.
pub(super) struct FlightResume {
    pub(super) flight_id: u64,
    /// Each sibling's captured mount root, which is where presence is read.
    pub(super) paths: HashMap<String, PathBuf>,
}

impl ResumeOwner for FlightResume {
    fn name(&self) -> &'static str {
        "Cmdr's own eject"
    }

    fn still_owns(&self, _candidate: &ResumeCandidate) -> bool {
        // A flight offers each volume once and never outlives its own offer.
        true
    }

    fn is_listed(&self, candidate: &ResumeCandidate) -> bool {
        self.paths
            .get(&candidate.volume_id)
            .is_some_and(|path| super::is_still_mounted(&path.to_string_lossy()))
    }

    fn is_ejected_by_another_owner(&self, candidate: &ResumeCandidate) -> bool {
        // ❗ By flight, not by "is it ejecting": this flight's own adopted siblings read
        // as ejecting until it lands, and refusing over that would strand them stopped.
        in_flight::is_ejecting_by_another(&candidate.volume_id, self.flight_id)
    }

    fn consume(&self, _candidate: &ResumeCandidate) {}
}

/// The index a flight test drives: nothing here touches a real one. Shared by the pure
/// tests below and `real_image.rs`, whose resume pin runs the same gate against a real
/// disk image.
#[cfg(test)]
pub(super) mod test_support {
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    use cmdr_index::{IndexVolumeKind, RemovableStop};

    use crate::file_system::volume::drive_release::{IndexDoor, RESUME_SETTLE, Ticket};
    use crate::ignore_poison::IgnorePoison;

    /// An index whose kind, intent, and starts a test writes and reads.
    // DEFAULT-OK: an index that knows nothing yet, which is what every test starts from.
    #[derive(Default)]
    pub(in crate::file_system::volume::eject) struct FakeIndex {
        indexing: Mutex<HashSet<String>>,
        intent: Mutex<HashSet<String>>,
        started: Mutex<Vec<String>>,
    }

    impl FakeIndex {
        /// The volume is indexing, and its intent says it should be.
        pub(in crate::file_system::volume::eject) fn indexes(&self, volume_id: &str) {
            self.indexing.lock_ignore_poison().insert(volume_id.to_string());
            self.intent.lock_ignore_poison().insert(volume_id.to_string());
        }

        /// Every volume a resume started again, sorted.
        pub(in crate::file_system::volume::eject) fn started(&self) -> Vec<String> {
            let mut started = self.started.lock_ignore_poison().clone();
            started.sort();
            started
        }

        /// The stop a release runs: it lets go of whatever this index has.
        pub(in crate::file_system::volume::eject) fn stop(
            self: &Arc<Self>,
        ) -> impl Fn(&str) -> RemovableStop + Send + Sync + 'static {
            let index = Arc::clone(self);
            move |volume_id: &str| {
                if index.indexing.lock_ignore_poison().remove(volume_id) {
                    RemovableStop::Released
                } else {
                    RemovableStop::NothingToStop
                }
            }
        }

        /// Wait for a resume batch to settle, for a gate on the REAL clock (a fake one
        /// is moved by hand instead).
        pub(in crate::file_system::volume::eject) fn wait_for_a_resume_to_settle(&self) {
            crate::test_support::wait_until(RESUME_SETTLE * 4, "the resume batch to settle", || {
                !self.started.lock_ignore_poison().is_empty()
            });
        }
    }

    impl IndexDoor for FakeIndex {
        fn volume_kind(&self, volume_id: &str) -> Option<IndexVolumeKind> {
            self.indexing
                .lock_ignore_poison()
                .contains(volume_id)
                .then_some(IndexVolumeKind::LocalExternal)
        }

        fn drives_to_resume(&self) -> Vec<String> {
            self.intent.lock_ignore_poison().iter().cloned().collect()
        }

        fn start_resumed(&self, volume_id: String, ticket: Ticket) {
            self.started.lock_ignore_poison().push(volume_id);
            drop(ticket);
        }

        fn is_ejecting(&self, _volume_id: &str) -> bool {
            false
        }

        fn is_listed(&self, _volume_id: &str) -> bool {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::FakeIndex;
    use super::*;
    use crate::file_system::volume::drive_release::IndexDoor;
    use std::time::Instant;

    /// Well past `RESUME_SETTLE`, so a resume batch's quiet period is over.
    const SETTLED: std::time::Duration = std::time::Duration::from_secs(30);

    fn mounted(path: &str, whole_unit: u32) -> MountedVolume {
        MountedVolume {
            bsd_name: format!("disk{whole_unit}s1"),
            whole_unit,
            volume_uuid: None,
            path: PathBuf::from(path),
        }
    }

    #[test]
    fn a_disks_siblings_are_the_registered_volumes_at_its_mounts_with_the_asked_one_first() {
        let registry: HashMap<PathBuf, String> = [
            (PathBuf::from("/Volumes/A"), "vol-a".to_string()),
            (PathBuf::from("/Volumes/B"), "vol-b".to_string()),
        ]
        .into();
        let disk = [
            mounted("/Volumes/B", 7),
            mounted("/Volumes/A", 7),
            // A volume of the same disk that Cmdr doesn't have registered.
            mounted("/Volumes/Unknown", 7),
        ];

        let siblings = capture("vol-a", Path::new("/Volumes/A"), &disk, |path| {
            registry.get(path).cloned()
        });

        assert_eq!(
            siblings,
            [
                Sibling {
                    volume_id: "vol-a".to_string(),
                    path: PathBuf::from("/Volumes/A"),
                },
                Sibling {
                    volume_id: "vol-b".to_string(),
                    path: PathBuf::from("/Volumes/B"),
                },
            ],
            "the asked volume leads, its registered sibling follows, and an unregistered mount names nobody"
        );
    }

    #[test]
    fn a_volume_reachable_through_a_spare_mount_isnt_captured_twice() {
        // `find_by_root` matches ANY known root, so the active-root check is what keeps
        // one volume from being stopped and resumed as two.
        let disk = [mounted("/Volumes/A", 7), mounted("/Volumes/A-1", 7)];
        let siblings = capture("vol-a", Path::new("/Volumes/A"), &disk, |_| Some("vol-a".to_string()));
        assert_eq!(siblings.len(), 1, "got {siblings:?}");
    }

    // ── What the teardown aims at, and what counts as done ───────────

    fn captured() -> Vec<PathBuf> {
        [PathBuf::from("/Volumes/A"), PathBuf::from("/Volumes/B")].to_vec()
    }

    #[test]
    fn a_disk_whose_captured_mounts_are_all_gone_is_done_only_if_nothing_else_is_mounted_on_it() {
        // `diskutil eject` of a two-partition disk unmounts what it can and exits
        // nonzero on the held one. Asking only about the volume the person clicked
        // would call that done and leave the disk powered on.
        assert!(
            disk_is_still_mounted(&captured(), |path| path == Path::new("/Volumes/B"), || false),
            "a sibling that stayed mounted means the disk is still up"
        );
        assert!(
            disk_is_still_mounted(&captured(), |_| false, || true),
            "and so does a volume the capture never named"
        );
        assert!(
            !disk_is_still_mounted(&captured(), |_| false, || false),
            "every captured mount gone and nothing else on the disk is the only 'done'"
        );
    }

    #[test]
    fn a_disk_nobody_could_read_counts_as_still_mounted_and_an_empty_one_doesnt() {
        // ❗ Three answers, not two. DiskArbitration going silent is NOT an empty disk:
        // collapsing them would answer `Ok` for an eject that left the drive powered on
        // with a volume Cmdr never registered still mounted on it.
        assert!(
            reads_as_mounted(DiskMounts::Unreadable),
            "nobody could say, so the disk is still up"
        );
        assert!(reads_as_mounted(DiskMounts::Read(vec![mounted("/Volumes/A", 7)])));
        assert!(
            !reads_as_mounted(DiskMounts::Read(Vec::new())),
            "a disk genuinely read as empty is the only one that's gone"
        );
    }

    #[test]
    fn a_retry_aims_at_a_mount_thats_still_there_not_at_the_one_that_already_went() {
        // Aimed at `/Volumes/A` after a partial unmount, `diskutil` answers "no such
        // disk" and the retry proves nothing.
        assert_eq!(
            aim_at(&captured(), "/Volumes/A", |path| path == Path::new("/Volumes/B")),
            "/Volumes/B"
        );
        assert_eq!(
            aim_at(&captured(), "/Volumes/A", |_| true),
            "/Volumes/A",
            "with everything still mounted it aims at the disk's own volume, which is captured first"
        );
        assert_eq!(
            aim_at(&captured(), "/Volumes/A", |_| false),
            "/Volumes/A",
            "with nothing left it falls back, and `settle` reads the tool's refusal as already gone"
        );
    }

    #[test]
    fn a_write_op_on_any_volume_of_the_disk_refuses_the_eject() {
        // The teardown takes the whole disk down, so a transfer on a sibling would be
        // truncated just as surely as one on the volume clicked.
        let ids = ["vol-a".to_string(), "vol-b".to_string()];
        assert_eq!(busy_sibling(&ids, &["vol-b".to_string()]), Some("vol-b"));
        assert_eq!(busy_sibling(&ids, &["vol-elsewhere".to_string()]), None);
    }

    #[test]
    fn the_mounts_a_teardown_has_to_see_gone_cover_the_captured_roots_and_a_fresh_read() {
        let siblings = [Sibling {
            volume_id: "vol-a".to_string(),
            path: PathBuf::from("/Volumes/A"),
        }];
        let paths = captured_paths(&[mounted("/Volumes/A", 7), mounted("/Volumes/Unknown", 7)], &siblings);
        assert_eq!(
            paths,
            [PathBuf::from("/Volumes/A"), PathBuf::from("/Volumes/Unknown")],
            "a volume Cmdr never registered still keeps the disk up"
        );
    }

    // ── What a flight owes back ───────────────────────────────────────

    #[test]
    fn only_a_volume_that_was_indexing_is_owed_its_index_back() {
        let index = Arc::new(FakeIndex::default());
        index.indexes("vol-a");
        let gate = DriveRelease::with_fake_clock(Arc::clone(&index) as Arc<dyn IndexDoor>);

        let release = gate.release(
            &["vol-a".to_string(), "vol-idle".to_string()],
            Instant::now(),
            index.stop(),
            |_| {},
        );

        assert_eq!(
            owed_ids(&release),
            ["vol-a"],
            "a volume with no index had nothing taken from it"
        );
    }

    #[test]
    fn a_refusal_after_the_flights_own_asks_moved_the_epochs_still_hands_the_index_back() {
        // `diskutil eject` triggers an unmount approval per volume, and each ask's own
        // release moves the epoch. Reading it at the pre-stop would fail every check.
        let index = Arc::new(FakeIndex::default());
        index.indexes("vol-a");
        let gate = DriveRelease::with_fake_clock(Arc::clone(&index) as Arc<dyn IndexDoor>);
        let owner = Arc::new(FlightResume {
            flight_id: u64::MAX,
            paths: [("vol-a".to_string(), PathBuf::from("/"))].into(),
        });

        let release = gate.release(&["vol-a".to_string()], Instant::now(), index.stop(), |_| {});
        let stale = epoch_of(&release, "vol-a");
        // The approver's ask during the teardown, which the eject then refused.
        gate.release(&["vol-a".to_string()], Instant::now(), index.stop(), |_| {});
        assert_ne!(gate.epoch("vol-a"), stale, "the ask moved the epoch");

        let batch = gate.resume(
            vec![ResumeCandidate {
                volume_id: "vol-a".to_string(),
                epoch: gate.epoch("vol-a"),
            }],
            Arc::clone(&owner) as Arc<dyn ResumeOwner>,
        );
        gate.advance(SETTLED);
        batch.wait();

        assert_eq!(index.started(), ["vol-a"], "the index came back");

        // And the same hand-back against the epoch the pre-stop saw resumes nothing.
        index.indexes("vol-a");
        gate.release(&["vol-a".to_string()], Instant::now(), index.stop(), |_| {});
        let batch = gate.resume(
            vec![ResumeCandidate {
                volume_id: "vol-a".to_string(),
                epoch: stale,
            }],
            owner as Arc<dyn ResumeOwner>,
        );
        gate.advance(SETTLED);
        batch.wait();
        assert_eq!(index.started(), ["vol-a"], "the stale epoch resumed nothing");
    }
}
