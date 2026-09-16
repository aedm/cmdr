use std::time::Duration;

#[cfg(target_os = "macos")]
use super::super::detached_holder::DetachedHolder;
use super::*;

fn app(name: &str, bundle_id: Option<&str>) -> AppFacts {
    AppFacts {
        name: name.to_string(),
        bundle_id: bundle_id.map(str::to_string),
    }
}

/// Everything a holder of a plain third-party tool looks like: nothing knows it.
fn unknown() -> ProcessFacts {
    ProcessFacts::default()
}

// ── The rules, in order ───────────────────────────────────────────

#[test]
fn cmdr_itself_and_anything_it_started_are_cmdr() {
    // ❗ Our bug, not the person's: the copy asks them to send a report.
    let ours = ProcessFacts {
        is_cmdr: true,
        ..unknown()
    };
    assert_eq!(classify(&ours).kind, HolderKind::Cmdr);
    assert_eq!(
        classify(&ours).name,
        None,
        "and it keeps the executable's name, since there's no app to point at"
    );

    // A descendant carries the same answer, and it beats every later rule: a helper
    // Cmdr started is still Cmdr's to let go of.
    let descendant = ProcessFacts {
        is_cmdr: true,
        app: Some(app("Warp", Some("dev.warp.Warp-Stable"))),
        platform_binary: Some(true),
        ..unknown()
    };
    assert_eq!(classify(&descendant).kind, HolderKind::Cmdr);
}

#[test]
fn a_process_an_app_owns_is_that_app_by_name_and_bundle_id() {
    // Finder, and any accessory app: a person can switch to it and close what it has open.
    let finder = ProcessFacts {
        app: Some(app("Finder", Some("com.apple.finder"))),
        platform_binary: Some(true),
        ..unknown()
    };
    let what = classify(&finder);
    assert_eq!(what.kind, HolderKind::App);
    assert_eq!(what.name.as_deref(), Some("Finder"));
    assert_eq!(what.bundle_id.as_deref(), Some("com.apple.finder"));
}

#[test]
fn a_shell_under_a_terminal_names_the_terminal() {
    // `/bin/zsh` is a platform binary with no app of its own; the app comes from its
    // ancestors, which is what the lineage walk is for.
    let zsh = ProcessFacts {
        app: Some(app("Warp", Some("dev.warp.Warp-Stable"))),
        platform_binary: Some(true),
        ..unknown()
    };
    assert_eq!(classify(&zsh).name.as_deref(), Some("Warp"));
    assert_eq!(classify(&zsh).kind, HolderKind::App);
}

#[test]
fn a_disk_image_stored_on_the_drive_is_its_own_kind() {
    // ❗ Closing apps can't free this one: the copy says to eject that image first.
    let helper = ProcessFacts {
        disk_image: Some("Installer".to_string()),
        platform_binary: Some(true),
        ..unknown()
    };
    let what = classify(&helper);
    assert_eq!(what.kind, HolderKind::DiskImage);
    assert_eq!(what.name.as_deref(), Some("Installer"));
}

#[test]
fn a_helper_is_the_app_macos_holds_responsible_for_it() {
    // A `com.apple.WebKit.WebContent` whose parent is launchd: no app of its own, and
    // its parent chain reaches nobody, so the responsible process is the only thing that
    // names the browser a person can quit.
    let helper = ProcessFacts {
        responsible: Some(app("Google Chrome", Some("com.google.Chrome"))),
        platform_binary: Some(true),
        ..unknown()
    };
    let what = classify(&helper);
    assert_eq!(what.kind, HolderKind::App);
    assert_eq!(what.name.as_deref(), Some("Google Chrome"));
}

#[test]
fn a_responsible_app_with_no_running_application_still_names_itself_from_its_signature() {
    // Google Drive's responsible process answers nil for `NSRunningApplication`, so the
    // name comes from the `Info.plist` its signature seals, and it carries no bundle id.
    let helper = ProcessFacts {
        responsible: Some(app("Google Drive", None)),
        ..unknown()
    };
    let what = classify(&helper);
    assert_eq!(what.kind, HolderKind::App);
    assert_eq!(what.name.as_deref(), Some("Google Drive"));
    assert_eq!(what.bundle_id, None);
}

#[test]
fn the_same_helper_without_the_responsible_symbol_falls_through_to_what_it_runs() {
    // ❗ The private symbol is allowed to be missing, and this is what that costs: a
    // WebKit helper reads as macOS rather than as the browser behind it. A worse
    // sentence, never a wrong kind.
    let helper = ProcessFacts {
        platform_binary: Some(true),
        ..unknown()
    };
    assert_eq!(classify(&helper).kind, HolderKind::System);
}

#[test]
fn a_process_running_from_the_drive_is_a_tool_and_is_never_asked_about() {
    // ❗ Rule 5 is the safety rule: `platform_binary` is `None` here because nothing
    // asked, and asking would have made Cmdr a holder of the drive it's letting go of.
    let from_the_drive = ProcessFacts {
        executable_on_target: true,
        ..unknown()
    };
    assert_eq!(classify(&from_the_drive).kind, HolderKind::Tool);
    assert_eq!(
        classify(&from_the_drive).name,
        None,
        "named by its executable, which is all anything knows about it"
    );
}

#[test]
fn an_apple_platform_binary_with_nothing_else_behind_it_is_macos() {
    // `/usr/libexec/lsd`, `mds_stores`, `backupd`: the copy asks for a minute's patience
    // rather than for something to close.
    let lsd = ProcessFacts {
        platform_binary: Some(true),
        ..unknown()
    };
    assert_eq!(classify(&lsd).kind, HolderKind::System);
}

#[test]
fn a_third_party_binary_with_nothing_else_behind_it_is_a_tool() {
    let daemon = ProcessFacts {
        platform_binary: Some(false),
        ..unknown()
    };
    assert_eq!(classify(&daemon).kind, HolderKind::Tool);
}

#[test]
fn a_holder_nothing_could_read_stays_unclassified_rather_than_being_guessed_at() {
    // ❗ What the budget leaves behind, and what a signature nothing could read answers.
    // The holder keeps its name, and the copy falls back to the sentence that names
    // nobody — ❌ never to "a tool is using this drive", which would be invented.
    assert_eq!(classify(&unknown()).kind, HolderKind::Unclassified);
}

// ── Putting an answer onto a holder ───────────────────────────────

#[test]
fn a_holder_an_app_answered_for_takes_the_apps_name_and_bundle_id() {
    let mut holder = VolumeHolder {
        pid: 94646,
        name: "stable".to_string(),
        bundle_id: None,
        kind: HolderKind::Unclassified,
    };
    rename(
        &mut holder,
        &classify(&ProcessFacts {
            app: Some(app("Warp", Some("dev.warp.Warp-Stable"))),
            ..unknown()
        }),
    );
    assert_eq!(holder.name, "Warp", "the executable name was `stable`, which names nothing");
    assert_eq!(holder.bundle_id.as_deref(), Some("dev.warp.Warp-Stable"));
    assert_eq!(holder.kind, HolderKind::App);
}

#[test]
fn a_holder_no_app_answered_for_keeps_the_name_the_walk_gave_it() {
    let mut holder = VolumeHolder {
        pid: 983,
        name: "lsd".to_string(),
        bundle_id: None,
        kind: HolderKind::Unclassified,
    };
    rename(
        &mut holder,
        &classify(&ProcessFacts {
            platform_binary: Some(true),
            ..unknown()
        }),
    );
    assert_eq!(holder.name, "lsd");
    assert_eq!(holder.kind, HolderKind::System);
}

// ── The budget ────────────────────────────────────────────────────

#[test]
fn a_deadline_that_has_already_passed_classifies_nobody_rather_than_running_over() {
    let mut named = Vec::new();
    name_the_kinds(
        &[std::process::id()],
        &[],
        Instant::now() - Duration::from_secs(1),
        |pid, what| named.push((pid, what)),
    );
    assert!(
        named.is_empty(),
        "the holders stay named and unclassified, got {named:?}"
    );
}

#[test]
fn asking_about_no_holders_at_all_does_nothing() {
    let mut named = Vec::new();
    name_the_kinds(&[], &[], Instant::now() + Duration::from_secs(30), |pid, what| {
        named.push((pid, what));
    });
    assert!(named.is_empty());
}

// ── The real signals ──────────────────────────────────────────────

/// Cmdr's own process is Cmdr, through the whole gathering rather than through a table.
#[test]
fn the_running_process_classifies_itself_as_cmdr() {
    let mut named = Vec::new();
    name_the_kinds(
        &[std::process::id()],
        &[],
        Instant::now() + Duration::from_secs(30),
        |pid, what| named.push((pid, what.kind)),
    );
    assert_eq!(named, [(std::process::id(), HolderKind::Cmdr)]);
}

/// A child of this process is Cmdr too, which is the lineage walk running against a real
/// process tree: the wrong `proc_pidinfo` flavor or a misread `pbsi_ppid` answers `System`
/// here, quietly.
#[cfg(target_os = "macos")]
#[test]
fn a_child_of_this_process_is_cmdr_through_the_real_process_tree() {
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("60")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn a child");

    let lineage = platform::lineage(child.id());
    assert_eq!(
        lineage.first(),
        Some(&child.id()),
        "the walk starts at the process itself"
    );
    assert!(
        lineage.contains(&std::process::id()),
        "and reaches this process, got {lineage:?}"
    );

    // allowed-discarded-outcome: a child that already exited has nothing left to stop
    let _ = child.kill();
    // allowed-discarded-outcome: reaping is cleanup, and a child that already went is fine
    let _ = child.wait();
}

/// `/bin/sleep` is signed as part of the OS and this test binary isn't, which is the one
/// pair that proves the Security call reads what it's supposed to: a wrong flag or a
/// wrong key answers the same for both.
#[cfg(target_os = "macos")]
#[test]
fn an_apple_binary_reads_as_a_platform_binary_and_this_test_binary_doesnt() {
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("60")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn a child");

    assert_eq!(
        platform::is_platform_binary(child.id()),
        Some(true),
        "`/bin/sleep` ships with macOS"
    );
    assert_eq!(
        platform::is_platform_binary(std::process::id()),
        Some(false),
        "and a locally built test binary doesn't"
    );

    // allowed-discarded-outcome: a child that already exited has nothing left to stop
    let _ = child.kill();
    // allowed-discarded-outcome: reaping is cleanup, and a child that already went is fine
    let _ = child.wait();
}

/// A holder running a binary from a scratch directory that stands in for the drive, so
/// the rules below `Cmdr` run against a real process.
#[cfg(target_os = "macos")]
fn holder_running_from_the_drive(named: &str) -> (crate::test_support::TestDir, DetachedHolder) {
    let dir = crate::test_support::TestDir::new(named);
    let held = dir.as_ref().join("held.txt");
    std::fs::write(&held, b"held").expect("write the held file");
    let holder = DetachedHolder::running_from(dir.as_ref(), &held);
    (dir, holder)
}

/// ❗ Invariant 14, against a real process: NOTHING asks Security about a process whose
/// executable lives on the drive being ejected. That query reads the binary, which puts
/// Cmdr itself in the kernel's holder list for the very drive it's letting go of (4.5 s,
/// and a Whole unmount in that window named the prober as the dissenter).
///
/// The assertion is that the signature was never read, which holds whichever of the two
/// rules above rule 6 answered: both return before anything asks.
#[cfg(target_os = "macos")]
#[test]
fn nothing_asks_security_about_a_binary_that_lives_on_the_drive_being_ejected() {
    let (_dir, holder) = holder_running_from_the_drive("holder-facts-tool");
    let on = device_of(holder.executable()).expect("the holder's own volume answers");

    let facts = gather(holder.pid(), std::process::id(), &Surroundings::new(vec![on]));

    assert_eq!(
        facts.platform_binary, None,
        "❌ no code-signing query ran against a binary on the drive"
    );
    assert!(
        !facts.is_cmdr,
        "and the holder descends from launchd, or rule 1 would have answered first"
    );
}

/// The drive test itself, against a real process: the executable's own device against
/// the teardown's mounts. A `proc_pidpath` that answered the wrong path, or an `lstat` of
/// the wrong thing, reads the same as "not on the drive" — which is exactly the case the
/// safety rule above must never miss.
#[cfg(target_os = "macos")]
#[test]
fn whether_a_process_runs_from_the_drive_is_its_executables_own_device() {
    let (_dir, holder) = holder_running_from_the_drive("holder-facts-device");
    let on = device_of(holder.executable()).expect("the holder's own volume answers");

    assert!(Surroundings::new(vec![on]).owns_executable(holder.pid()));
    assert!(
        !Surroundings::new(vec![u64::MAX]).owns_executable(holder.pid()),
        "a drive somewhere else isn't this executable's"
    );
    assert!(
        !Surroundings::new(Vec::new()).owns_executable(holder.pid()),
        "and a teardown whose mounts wouldn't stat asks about nobody"
    );
}

/// The private responsible-process symbol resolves on this macOS and answers for a real
/// process. ❗ A `None` isn't a failure of the rule — the symbol is allowed to go — but
/// this is how we'd learn that it had.
///
/// ❗ **Responsibility is INHERITED, and launchd adopting a process doesn't clear it**
/// (verified on macOS 27.0, 2026-09-16: a holder started through a shell that exited,
/// `PPID` 1, still answered the terminal app that ran the test). So rule 4 names the app
/// a drive-resident tool was STARTED from, and rule 5 answers only for a holder no app
/// ever launched. Both are honest sentences; what matters is that neither reads the
/// binary.
#[cfg(target_os = "macos")]
#[test]
fn the_responsible_process_symbol_answers_for_a_real_process() {
    let (_dir, holder) = holder_running_from_the_drive("holder-facts-responsible");

    let responsible = platform::responsible_pid(holder.pid());

    assert!(
        responsible.is_some_and(|pid| pid > 1),
        "something is responsible for a live process, got {responsible:?}"
    );
}

/// A pid nothing runs under answers nothing, rather than panicking or naming somebody.
#[cfg(target_os = "macos")]
#[test]
fn a_process_that_isnt_there_answers_nothing_at_all() {
    // `PID_MAX` is 99,999 on macOS, so this one can't exist.
    let gone = 999_999;
    assert_eq!(platform::executable_path(gone), None);
    assert_eq!(platform::running_app(gone), None);
    assert_eq!(platform::is_platform_binary(gone), None);
    assert_eq!(platform::lineage(gone), vec![gone], "and the walk stops where it began");
}
