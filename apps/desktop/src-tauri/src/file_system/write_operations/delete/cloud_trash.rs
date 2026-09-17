//! Where "move to trash" has to become a permanent delete: online-only files in
//! cloud-storage folders.
//!
//! A third-party File Provider (Dropbox, Google Drive, OneDrive, …) mounts its
//! drive under `~/Library/CloudStorage/<domain>/` and evicts files it has
//! already uploaded, leaving a stub macOS marks `SF_DATALESS`. The Finder calls
//! those "online-only".
//!
//! ## Why an online-only file must not go to the Trash
//!
//! The Trash is a folder on the boot volume, so putting an evicted file in it
//! means DOWNLOADING it first. macOS does exactly that: a 111 kB online-only
//! Dropbox file trashed in ~500 ms and landed in `~/.Trash` at its full size
//! (verified on macOS 27.0, 2026-09-17, prod build). Three small PNGs are half a
//! second. A folder holding 50 GB of online-only content is 50 GB pulled over
//! someone's connection, to fill a folder they are about to empty, for files
//! they just said they don't want.
//!
//! So that download is the reason this module exists — ❌ not the refusals. The
//! same file trashed fine at 08:45 and was refused twice at 09:06 while a
//! sibling went through, and the refusals arrive under two different `NSError`
//! codes (513 `NSFileWriteNoPermissionError`, 3328 `NSFeatureUnsupportedError`)
//! that land in two different `TrashRefusalKind` buckets. Reacting to the error
//! is therefore both unreliable and beside the point; the file's own flag is the
//! signal, and it answers before anything is downloaded.
//!
//! F8 asks this module first and routes to the permanent delete when the
//! selection carries an online-only item. That delete has its own confirmation
//! dialog (visibly different from the trash one) and the provider keeps its own
//! server-side retention, so the file is recoverable from the service.
//!
//! ## ❌ Don't widen this back to "any cloud folder"
//!
//! An ordinary, materialized Dropbox file trashes perfectly well, and routing it
//! to a permanent delete converts a working trash into data loss. Being inside a
//! cloud drive is only half the predicate; `SF_DATALESS` on the item is the
//! other half.
//!
//! ## ❌ Don't widen this to "anywhere without a trash"
//!
//! A freshly formatted USB stick answers "no trash" from a volume probe purely
//! because nobody has trashed anything on it yet, and routing that to a permanent
//! delete would destroy data the OS would happily have kept. The rule is narrow on
//! purpose: a path strictly inside `~/Library/CloudStorage/<domain>/`, under a
//! provider Cmdr knows by name, in Apple's documented location for File Provider
//! storage since macOS 12.3. Everything else keeps attempting the trash and lets
//! the typed `TrashRefusalKind` refusal speak.
//!
//! ❌ Don't gate on the volume either. A File Provider folder is NOT its own
//! volume: `~/Library/CloudStorage/Dropbox` and `~` report the same device on the
//! same `/dev/disk3s5` (verified on macOS 27.0, `stat -f '%d'`, 2026-09-17), so
//! `trash_dir_for_path()` answers `~/.Trash` for these paths and can't see this.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
use crate::file_system::cloud_provider::{CloudProvider, locate};

/// macOS's "file is dataless object" flag, the bit a File Provider sets when it
/// evicts a file's contents and leaves a stub behind. Finder words it as
/// "online-only"; `stat -f "%Sf"` prints it as `dataless`.
///
/// Defined here because the `libc` crate doesn't export it: its `SF_*` set stops
/// at `SF_SETTABLE`, `SF_ARCHIVED`, `SF_IMMUTABLE`, and `SF_APPEND`. The value is
/// Apple's, from `/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk/usr/include/sys/stat.h:359`
/// (verified on macOS 27.0 / Command Line Tools, 2026-09-17, and confirmed
/// against a real evicted Dropbox file whose `stat -f "%Sf"` reads
/// `compressed,dataless`).
#[cfg(target_os = "macos")]
const SF_DATALESS: u32 = 0x4000_0000;

/// What a routing decision needs to know about one selected item, read from the
/// item's OWN `lstat`.
///
/// Injectable (see [`routing_with`]) because no test can create a real dataless
/// file: only a File Provider can set `SF_DATALESS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemFacts {
    /// A directory never carries `SF_DATALESS` itself (the flag lives on files),
    /// so a selected folder's answer can only come from walking it.
    pub is_directory: bool,
    /// The item's contents live only on the provider's servers.
    pub online_only: bool,
}

/// What an F8 over a given selection should actually run.
///
/// Typed rather than a bare `bool` so the frontend's routing reads as a decision
/// the backend made, and so a future "this one is trashless for another reason"
/// arrives as a variant instead of a second boolean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum TrashRouting {
    /// Ask the OS trash, and let a refusal answer for itself. The default for
    /// everything, including an unknown or unreachable path.
    Trash,
    /// The selection holds online-only content in a cloud-storage drive. Run the
    /// permanent-delete flow, whose dialog says why.
    PermanentDeleteCloudStorage,
}

/// The backend's full answer about a selection: what to run, and whether that
/// answer is final.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TrashRoutingAnswer {
    pub routing: TrashRouting,
    /// A selected FOLDER sits in a known cloud drive, so `routing` is only what
    /// the top-level items say: an online-only file deeper inside would flip it.
    /// The confirmation dialog already walks that folder for its scan preview,
    /// and that walk is what answers (`scan_walker.rs`'s `online_only_seen`).
    ///
    /// ❗ False for a selection of plain files, where the per-item `lstat` above
    /// IS the whole answer, so the dialog never waits for a walk it doesn't need.
    pub folder_may_hold_online_only: bool,
}

impl TrashRoutingAnswer {
    /// The answer for anything unresolvable, and the starting point every
    /// selection is judged from: today's behavior, with nothing left to wait for.
    pub const TRASH: Self = Self {
        routing: TrashRouting::Trash,
        folder_may_hold_online_only: false,
    };
}

/// Whether `path` sits strictly inside a third-party cloud drive under
/// `~/Library/CloudStorage/<domain>/`.
///
/// HALF the routing predicate: being here makes an item's `SF_DATALESS` flag
/// meaningful, and ❌ never routes anything on its own.
///
/// Pure (path comparison only, no disk) and `home`-injected, so it's unit-testable
/// without a real cloud folder. `path` must already be symlink-resolved; see
/// [`routing_for`].
///
/// Four deliberate exclusions:
/// - **iCloud Drive** (`~/Library/Mobile Documents/`): Finder trashes from there
///   fine, so it keeps today's behavior. If a provider ever refuses, the typed
///   refusal path already catches it.
/// - **A provider we don't know by name**
///   ([`CloudProvider::keeps_deleted_items_recoverable`]): a `CloudStorage`
///   directory isn't always a cloud service. MacDroid publishes an Android phone
///   as one, and a delete there is final, so it keeps the OS trash and its
///   refusal instead of a dialog promising a copy that doesn't exist.
/// - **The `CloudStorage` container itself**, which holds no user data.
/// - **A drive's own root** (`…/CloudStorage/Dropbox`): deleting a whole cloud
///   root permanently would take the account's entire local copy with it, so that
///   one stays with the OS trash and its refusal.
#[cfg(target_os = "macos")]
pub fn is_inside_known_cloud_drive(home: &Path, path: &Path) -> bool {
    let Some(found) = locate(home, path) else {
        return false;
    };
    found.provider != CloudProvider::ICloudDrive
        && found.provider.keeps_deleted_items_recoverable()
        && path != found.root
}

/// `~/Library/CloudStorage` is macOS's layout, so everywhere else this is simply
/// "an ordinary location": attempt the trash and let it answer.
#[cfg(not(target_os = "macos"))]
pub fn is_inside_known_cloud_drive(_home: &Path, _path: &Path) -> bool {
    false
}

/// Whether metadata the caller already has marks the item online-only.
///
/// Takes `std::fs::Metadata` rather than a path so a walk that just stat'ed an
/// entry pays nothing extra to ask. It must be `symlink_metadata`: see
/// [`is_online_only`].
#[cfg(target_os = "macos")]
pub fn metadata_is_online_only(metadata: &std::fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;
    metadata.st_flags() & SF_DATALESS != 0
}

/// `SF_DATALESS` is Apple's; nothing elsewhere evicts a file behind our back.
#[cfg(not(target_os = "macos"))]
pub fn metadata_is_online_only(_metadata: &std::fs::Metadata) -> bool {
    false
}

/// Whether the item at `path` is online-only, read WITHOUT materializing it.
///
/// ❗ A metadata read never pulls the bytes down; ❌ never open or read a file to
/// find out, which is the very download this whole module exists to avoid.
///
/// `symlink_metadata`, so the LEAF symlink isn't followed: trashing a symlink
/// acts on the link, and a link in an ordinary folder pointing at an evicted
/// cloud file keeps ordinary behavior.
pub fn is_online_only(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata_is_online_only(&metadata))
}

/// Reads the facts a routing decision needs off one resolved path, or `None`
/// when it can't be stat'ed (already gone, a dead mount).
fn item_facts(path: &Path) -> Option<ItemFacts> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    Some(ItemFacts {
        is_directory: metadata.is_dir() && !metadata.is_symlink(),
        online_only: metadata_is_online_only(&metadata),
    })
}

/// The routing for a whole selection, against an injected `home`, paths that are
/// resolved here, and an injected per-item `lstat`.
///
/// **One online-only item routes the whole batch.** ❌ Not per item: splitting
/// one gesture into "these go to the Trash, those get deleted permanently" is two
/// mental models in one confirmation, and no readable copy comes out of it. An
/// empty selection is [`TrashRouting::Trash`].
pub fn routing_with(
    home: &Path,
    paths: &[PathBuf],
    facts_for: &dyn Fn(&Path) -> Option<ItemFacts>,
) -> TrashRoutingAnswer {
    let mut answer = TrashRoutingAnswer::TRASH;
    for path in paths {
        let Some(resolved) = resolve_through_symlinks(path) else {
            continue;
        };
        if !is_inside_known_cloud_drive(home, &resolved) {
            continue;
        }
        let Some(facts) = facts_for(&resolved) else {
            continue;
        };
        if facts.online_only {
            answer.routing = TrashRouting::PermanentDeleteCloudStorage;
        } else if facts.is_directory {
            answer.folder_may_hold_online_only = true;
        }
    }
    answer
}

/// [`routing_with`] against the real filesystem.
pub fn routing_for(home: &Path, paths: &[PathBuf]) -> TrashRoutingAnswer {
    routing_with(home, paths, &item_facts)
}

/// The routing for a selection, against the real home directory.
///
/// Answers [`TrashRouting::Trash`] when there's no readable home directory: an
/// unanswerable question means today's behavior, never a permanent delete.
pub fn routing_for_selection(paths: &[PathBuf]) -> TrashRoutingAnswer {
    let Some(home) = dirs::home_dir() else {
        return TrashRoutingAnswer::TRASH;
    };
    // `~` itself can be a symlink (or sit behind a firmlink), and the resolved
    // paths below are canonical, so the prefix they're tested against has to be too.
    let home = std::fs::canonicalize(&home).unwrap_or(home);
    routing_for(&home, paths)
}

/// Whether any of `paths` reaches into a cloud drive we know by name.
///
/// The scan preview asks this to decide whether to tally online-only files as it
/// walks, which is what keeps that tally off every ordinary scan. Path-only, no
/// disk beyond the symlink resolution the routing does anyway.
pub fn selection_touches_known_cloud_drive(paths: &[PathBuf]) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let home = std::fs::canonicalize(&home).unwrap_or(home);
    paths
        .iter()
        .filter_map(|path| resolve_through_symlinks(path))
        .any(|resolved| is_inside_known_cloud_drive(&home, &resolved))
}

/// Resolves `path` through symlinks without requiring the path itself to exist.
///
/// Canonicalizes the nearest existing ANCESTOR and re-attaches the names below it,
/// which handles both halves of the problem:
/// - A cloud folder is reachable by more than one path (Dropbox links `~/Dropbox`
///   at `~/Library/CloudStorage/Dropbox`), so a literal prefix test would miss
///   half the ways a person navigates there.
/// - The leaf may already be gone by the time we ask, and `fs::canonicalize` on a
///   missing path fails outright.
///
/// The leaf is deliberately NOT canonicalized: trashing a symlink acts on the link
/// itself, so a link that lives in an ordinary folder and points into a cloud drive
/// must keep the ordinary behavior.
///
/// `None` for anything that can't be resolved into an absolute, ordinary path (a
/// root with no file name, a `..` component below the existing ancestor, a virtual
/// MTP/SMB path), which the caller reads as "not a cloud path".
fn resolve_through_symlinks(path: &Path) -> Option<PathBuf> {
    let mut tail: Vec<OsString> = Vec::new();
    let mut cursor = path.to_path_buf();
    loop {
        // `file_name()` is `None` for `/`, for a bare prefix, and for a path ending
        // in `..` — all cases where re-attaching names would be a guess.
        tail.push(cursor.file_name()?.to_os_string());
        let parent = cursor.parent()?.to_path_buf();
        if let Ok(resolved) = std::fs::canonicalize(&parent) {
            return Some(tail.iter().rev().fold(resolved, |acc, name| acc.join(name)));
        }
        cursor = parent;
    }
}

/// macOS-only: everywhere else the predicate is a constant `false`, so these
/// assertions would be asserting the stub rather than the rule.
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::test_support::TestDir;
    use std::collections::HashMap;
    use std::fs;

    fn home() -> PathBuf {
        PathBuf::from("/Users/test")
    }

    /// Only a File Provider can set `SF_DATALESS`, so every test supplies the
    /// per-item facts instead of creating a file that carries it. Anything not
    /// listed is an ordinary, materialized file.
    fn facts_from(table: &[(&str, ItemFacts)]) -> impl Fn(&Path) -> Option<ItemFacts> + use<> {
        let table: HashMap<PathBuf, ItemFacts> = table
            .iter()
            .map(|(path, facts)| (PathBuf::from(path), *facts))
            .collect();
        move |path: &Path| {
            Some(table.get(path).copied().unwrap_or(ItemFacts {
                is_directory: false,
                online_only: false,
            }))
        }
    }

    const ONLINE_ONLY_FILE: ItemFacts = ItemFacts {
        is_directory: false,
        online_only: true,
    };
    const FOLDER: ItemFacts = ItemFacts {
        is_directory: true,
        online_only: false,
    };

    fn routing(paths: &[&str], table: &[(&str, ItemFacts)]) -> TrashRoutingAnswer {
        let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        routing_with(&home(), &paths, &facts_from(table))
    }

    #[test]
    fn an_online_only_item_inside_a_cloud_drive_routes_to_the_delete() {
        let path = "/Users/test/Library/CloudStorage/Dropbox/Work/emclient.pkg";
        assert_eq!(
            routing(&[path], &[(path, ONLINE_ONLY_FILE)]).routing,
            TrashRouting::PermanentDeleteCloudStorage
        );
    }

    /// The regression this whole rule exists to prevent: an ordinary Dropbox file
    /// trashes perfectly well, and routing it to a permanent delete turns a
    /// working trash into data loss.
    #[test]
    fn a_materialized_item_in_the_same_folder_keeps_the_trash() {
        let evicted = "/Users/test/Library/CloudStorage/Dropbox/Work/evicted.pkg";
        let here = "/Users/test/Library/CloudStorage/Dropbox/Work/here.pkg";
        let answer = routing(&[here], &[(evicted, ONLINE_ONLY_FILE)]);
        assert_eq!(answer.routing, TrashRouting::Trash);
        assert!(
            !answer.folder_may_hold_online_only,
            "a plain file's own lstat is the whole answer, so nothing waits on a walk"
        );
    }

    /// The flag is what routes; a cloud drive is only where it counts.
    #[test]
    fn an_online_only_item_outside_any_cloud_drive_keeps_the_trash() {
        for path in [
            "/Users/test/Documents/report.pdf",
            "/Users/test/Library/Mobile Documents/com~apple~CloudDocs/notes.md",
            "/Users/test/Library/CloudStorage/MacDroid-Pixel/DCIM/a.jpg",
            "/Users/test/Library/CloudStorage",
            "/Users/test/Library/CloudStorage/Dropbox",
            "/Volumes/USB/photo.jpg",
        ] {
            assert_eq!(
                routing(&[path], &[(path, ONLINE_ONLY_FILE)]).routing,
                TrashRouting::Trash,
                "{path} must keep today's trash behavior"
            );
        }
    }

    /// One online-only item takes the whole gesture with it: a confirmation that
    /// said "these go to the Trash, those get deleted" would be unreadable.
    #[test]
    fn one_online_only_item_routes_the_whole_mixed_selection() {
        let evicted = "/Users/test/Library/CloudStorage/Dropbox/a.pdf";
        let here = "/Users/test/Library/CloudStorage/Dropbox/b.pdf";
        let ordinary = "/Users/test/Documents/c.pdf";

        assert_eq!(
            routing(&[here, evicted], &[(evicted, ONLINE_ONLY_FILE)]).routing,
            TrashRouting::PermanentDeleteCloudStorage
        );
        assert_eq!(
            routing(&[ordinary, evicted], &[(evicted, ONLINE_ONLY_FILE)]).routing,
            TrashRouting::PermanentDeleteCloudStorage
        );
        assert_eq!(routing(&[here, ordinary], &[]).routing, TrashRouting::Trash);
        assert_eq!(routing(&[], &[]), TrashRoutingAnswer::TRASH, "nothing selected");
    }

    /// `SF_DATALESS` lives on files, so a selected folder can only be answered by
    /// the walk the confirmation dialog runs anyway.
    #[test]
    fn a_folder_in_a_cloud_drive_defers_to_its_walk() {
        let folder = "/Users/test/Library/CloudStorage/Dropbox/Work";
        let answer = routing(&[folder], &[(folder, FOLDER)]);
        assert_eq!(answer.routing, TrashRouting::Trash, "nothing found yet");
        assert!(answer.folder_may_hold_online_only);
    }

    /// A folder outside every cloud drive can't hold an evicted file, so its walk
    /// is never waited on.
    #[test]
    fn an_ordinary_folder_never_waits_on_a_walk() {
        let folder = "/Users/test/Documents/Work";
        assert!(!routing(&[folder], &[(folder, FOLDER)]).folder_may_hold_online_only);
    }

    /// Dropbox links `~/Dropbox` at its `CloudStorage` drive, so the same file is
    /// reachable by two paths and only one of them matches literally.
    #[test]
    fn a_symlinked_route_into_a_cloud_drive_still_routes() {
        let scratch = TestDir::new("cloud_trash_symlink");
        let home = fs::canonicalize(&*scratch).expect("canonical scratch home");
        let drive = home.join("Library/CloudStorage/Dropbox");
        fs::create_dir_all(drive.join("Work")).expect("cloud drive");
        fs::write(drive.join("Work/report.pdf"), b"x").expect("a file in the drive");
        std::os::unix::fs::symlink(&drive, home.join("Dropbox")).expect("the provider's home link");

        let evicted = drive.join("Work/report.pdf");
        let all_evicted = |path: &Path| {
            Some(ItemFacts {
                is_directory: false,
                online_only: path == evicted,
            })
        };
        assert_eq!(
            routing_with(&home, &[home.join("Dropbox/Work/report.pdf")], &all_evicted).routing,
            TrashRouting::PermanentDeleteCloudStorage
        );
    }

    /// Trashing a symlink acts on the LINK, which lives outside the cloud drive
    /// and trashes fine, so the target's location must not decide the routing.
    #[test]
    fn a_link_pointing_into_a_cloud_drive_keeps_the_trash() {
        let scratch = TestDir::new("cloud_trash_leaf_link");
        let home = fs::canonicalize(&*scratch).expect("canonical scratch home");
        let drive = home.join("Library/CloudStorage/Dropbox");
        fs::create_dir_all(&drive).expect("cloud drive");
        fs::write(drive.join("report.pdf"), b"x").expect("a file in the drive");
        fs::create_dir_all(home.join("Documents")).expect("an ordinary folder");
        let link = home.join("Documents/report-link.pdf");
        std::os::unix::fs::symlink(drive.join("report.pdf"), &link).expect("the link");

        // Even if the link's own leaf read came back evicted, the link is not in
        // the drive, so nothing routes.
        let everything_evicted = |_: &Path| {
            Some(ItemFacts {
                is_directory: false,
                online_only: true,
            })
        };
        assert_eq!(
            routing_with(&home, &[link], &everything_evicted).routing,
            TrashRouting::Trash
        );
    }

    /// A leaf that vanished between the pane and the keystroke has no flags to
    /// read, and an unanswerable question keeps today's behavior.
    #[test]
    fn an_item_we_cant_stat_keeps_the_trash() {
        let answer = routing_with(
            &home(),
            &[PathBuf::from("/Users/test/Library/CloudStorage/Dropbox/vanished.pdf")],
            &|_| None,
        );
        assert_eq!(answer, TrashRoutingAnswer::TRASH);
    }

    #[test]
    fn an_unresolvable_path_keeps_the_trash() {
        for path in ["/", "mtp-1234://Internal storage/DCIM/a.jpg"] {
            assert_eq!(
                routing(&[path], &[(path, ONLINE_ONLY_FILE)]),
                TrashRoutingAnswer::TRASH,
                "{path} must not route"
            );
        }
    }
}
