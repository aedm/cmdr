use std::time::Duration;

use super::*;

fn holder(pid: u32, name: &str) -> VolumeHolder {
    VolumeHolder {
        pid,
        name: name.to_string(),
        bundle_id: None,
        kind: HolderKind::Unclassified,
    }
}

// ── What the answers mean ─────────────────────────────────────────

#[test]
fn a_scan_that_covered_every_path_is_the_whole_story_even_when_it_found_nobody() {
    // The one answer that may say "nothing holds this drive": a root-owned holder is
    // invisible to a same-uid scan, so this is still "nobody we can see".
    let merged = merge(&[PathScan::Named(Vec::new()), PathScan::Named(Vec::new())], 2);
    assert_eq!(merged, HolderScan::Complete { named: Vec::new() });
}

#[test]
fn a_path_the_scan_couldnt_read_makes_the_whole_answer_incomplete() {
    // ❗ This is the collapse two milestones in a row found a real defect behind: an
    // unreadable answer read as an empty one would word a plainly-held drive as free.
    let merged = merge(&[PathScan::Named(vec![holder(7, "sleep")]), PathScan::Unreadable], 2);
    assert_eq!(
        merged,
        HolderScan::Incomplete {
            named: vec![holder(7, "sleep")]
        },
        "what it did see is kept, but it's never the whole story"
    );
}

#[test]
fn a_scan_that_answered_for_fewer_paths_than_it_was_asked_about_is_incomplete() {
    // What the budget leaves behind: the thread was walked away from partway.
    let merged = merge(&[PathScan::Named(vec![holder(7, "sleep")])], 2);
    assert!(
        matches!(merged, HolderScan::Incomplete { .. }),
        "got {merged:?}, but one of two paths answered"
    );
}

#[test]
fn a_scan_with_nothing_to_ask_about_names_nobody_rather_than_clearing_the_drive() {
    // Every captured mount left the table while the tool was refusing. Nobody could be
    // asked, so ❌ nobody may be cleared either.
    assert_eq!(merge(&[], 0), HolderScan::not_scanned());
}

#[test]
fn one_process_holding_two_of_a_disks_volumes_is_named_once_in_first_seen_order() {
    let merged = merge(
        &[
            PathScan::Named(vec![holder(7, "sleep"), holder(9, "Finder")]),
            PathScan::Named(vec![holder(9, "Finder"), holder(11, "lsd")]),
        ],
        2,
    );
    assert_eq!(
        merged,
        HolderScan::Complete {
            named: vec![holder(7, "sleep"), holder(9, "Finder"), holder(11, "lsd")]
        }
    );
}

// ── The mount that changed under the scan ─────────────────────────

#[test]
fn a_mount_whose_device_changed_mid_scan_discards_what_the_scan_saw() {
    // DiskArbitration reuses BSD unit numbers at once, so a volume that went away and a
    // different one that took its mount point would have the scan name processes
    // holding somebody else's drive.
    let mut device = [16_777_241_u64, 16_777_250].into_iter();
    let scanned = scan_path_with(
        move || device.next(),
        || Some(vec![7]),
        |pid| Some(holder(pid, "sleep")),
    );
    assert_eq!(scanned, PathScan::Unreadable);
}

#[test]
fn a_mount_that_stayed_put_keeps_what_the_scan_saw() {
    let scanned = scan_path_with(|| Some(16_777_241), || Some(vec![7]), |pid| Some(holder(pid, "sleep")));
    assert_eq!(scanned, PathScan::Named(vec![holder(7, "sleep")]));
}

#[test]
fn a_root_that_wouldnt_stat_and_a_walk_that_wouldnt_run_both_name_nobody() {
    assert_eq!(
        scan_path_with(|| None, || Some(vec![7]), |pid| Some(holder(pid, "sleep"))),
        PathScan::Unreadable,
        "a root that wouldn't stat says nothing about who holds it"
    );
    assert_eq!(
        scan_path_with(|| Some(16_777_241), || None, |pid| Some(holder(pid, "sleep"))),
        PathScan::Unreadable
    );
}

#[test]
fn a_process_that_ended_between_the_walk_and_its_name_drops_out() {
    // ESRCH: it let go while the scan ran, so naming it would send a person after a
    // process that isn't there.
    let scanned = scan_path_with(
        || Some(16_777_241),
        || Some(vec![7, 9]),
        |pid| (pid == 9).then(|| holder(pid, "Finder")),
    );
    assert_eq!(scanned, PathScan::Named(vec![holder(9, "Finder")]));
}

// ── The budget ────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn a_scan_that_outlasts_its_budget_is_abandoned_rather_than_waited_out() {
    // `proc_listpidspath` `stat`s its path first, and that `stat` can hang for good on a
    // wedged mount. Nobody may wait for it, and what it never answered is "couldn't
    // tell".
    let budget = Duration::from_millis(1500);
    let started = tokio::time::Instant::now();
    let (release, wedged) = std::sync::mpsc::channel::<()>();

    let ran = within_budget(budget, move || {
        // Blocks until the test lets go, the way a `stat` on a wedged mount would.
        // allowed-discarded-outcome: the recv IS the block; whether it ended in a send or a hang-up says nothing
        let _ = wedged.recv();
    })
    .await;

    assert!(!ran, "the scan didn't finish, so its answer can't be trusted");
    assert_eq!(started.elapsed(), budget, "and the eject waited exactly the budget");
    drop(release);
}

/// ❗ On the REAL clock. The scan runs on a plain std thread, which tokio's paused
/// clock knows nothing about, so a paused runtime auto-advances past the budget before
/// the thread has run and every scan reads as abandoned. (It really does: this assertion
/// failed that way the first time it ran.)
#[tokio::test]
async fn a_scan_that_finishes_inside_its_budget_is_trusted() {
    assert!(within_budget(Duration::from_secs(30), || {}).await);
}

#[tokio::test(start_paused = true)]
async fn asking_about_no_paths_at_all_answers_at_once_without_a_thread() {
    let started = tokio::time::Instant::now();
    assert_eq!(
        scan(Vec::new(), Duration::from_millis(1500)).await,
        HolderScan::not_scanned()
    );
    assert_eq!(started.elapsed(), Duration::ZERO);
}

// ── The words a log and an MCP reply read ─────────────────────────

#[test]
fn the_log_line_names_every_holder_and_says_when_the_list_is_short() {
    assert_eq!(
        HolderScan::Complete {
            named: vec![holder(94646, "Warp"), holder(983, "lsd")]
        }
        .to_string(),
        "held by Warp [unclassified, pid 94646], lsd [unclassified, pid 983]"
    );
    assert_eq!(
        HolderScan::Complete { named: Vec::new() }.to_string(),
        "held by nobody a same-uid scan can see"
    );
    assert_eq!(
        HolderScan::not_scanned().to_string(),
        "held by nobody anything could name",
        "❌ never 'nobody is holding it'"
    );
    assert_eq!(
        HolderScan::Incomplete {
            named: vec![holder(983, "lsd")]
        }
        .to_string(),
        "held by lsd [unclassified, pid 983], and maybe more the scan couldn't cover"
    );
}

// ── The real kernel walk ──────────────────────────────────────────

/// A child process holding a file open is what `proc_listpidspath` is for, and this is
/// the one test that proves the FFI declaration itself: the wrong argument order or a
/// wrong flag value answers nobody, quietly.
///
/// Asked about the FILE rather than its volume (`FILE_FLAGS`), so it needs no drive: the
/// same walk, without the volume widening.
#[cfg(target_os = "macos")]
#[test]
fn a_child_holding_a_file_open_is_named_by_the_real_walk() {
    use std::process::{Command, Stdio};

    let dir = crate::test_support::TestDir::new("holder-scan");
    let held = dir.as_ref().join("held.txt");
    std::fs::write(&held, b"held").expect("write the held file");
    let open = std::fs::File::open(&held).expect("open the held file");
    let mut holder = Command::new("/bin/sleep")
        .arg("60")
        .stdin(Stdio::from(open))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the holder");
    let held_by = holder.id();

    // The child takes a moment to exist; 8 s is the cap, not the expected wait.
    crate::test_support::wait_until(Duration::from_secs(8), "the holder to show up in the scan", || {
        scan::pids_holding(&held, scan::FILE_FLAGS).is_some_and(|pids| pids.contains(&held_by))
    });

    // allowed-discarded-outcome: a holder that already exited has nothing left to stop
    let _ = holder.kill();
    // allowed-discarded-outcome: reaping is cleanup, and a child that already went is fine
    let _ = holder.wait();
}

/// A path nothing could ever hold answers an empty list, ❌ not an error: that's the
/// difference between `Complete` and `Incomplete`.
#[cfg(target_os = "macos")]
#[test]
fn a_file_nobody_holds_answers_an_empty_list_rather_than_nothing() {
    let dir = crate::test_support::TestDir::new("holder-scan-idle");
    let idle = dir.as_ref().join("idle.txt");
    std::fs::write(&idle, b"idle").expect("write the file");

    let pids = scan::pids_holding(&idle, scan::FILE_FLAGS);
    assert_eq!(pids, Some(Vec::new()), "the walk ran and found nobody");
}

/// A path that isn't there can't be scanned, and that reads as "couldn't tell".
#[cfg(target_os = "macos")]
#[test]
fn a_path_that_isnt_there_cant_be_scanned() {
    let missing = PathBuf::from("/Volumes/cmdr-test-never-mounted-4b71/held.txt");
    assert_eq!(scan::pids_holding(&missing, scan::FILE_FLAGS), None);
    assert_eq!(scan::scan_path(&missing), PathScan::Unreadable);
}
