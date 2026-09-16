//! Where "move to trash" has to become a permanent delete: cloud-storage folders.
//!
//! A third-party File Provider (Dropbox, Google Drive, OneDrive, …) mounts its
//! drive under `~/Library/CloudStorage/<domain>/` and may implement no trash at
//! all. `trashItemAtURL` there fails with `NSCocoaErrorDomain` 3328
//! (`NSFeatureUnsupportedError`), which macOS words as «the volume "Macintosh HD"
//! doesn't have one» — misleading, since the boot volume's trash is fine and it's
//! the provider that won't take the item.
//!
//! So F8 asks this module first, and routes to the permanent delete when every
//! selected item lives in such a drive. That delete carries its own confirmation
//! dialog (visibly different from the trash one) and the provider keeps its own
//! server-side retention, so the file is recoverable from the service.
//!
//! ## ❌ Don't widen this to "anywhere without a trash"
//!
//! A freshly formatted USB stick answers "no trash" from a volume probe purely
//! because nobody has trashed anything on it yet, and routing that to a permanent
//! delete would destroy data the OS would happily have kept. The rule is narrow on
//! purpose: a path strictly inside `~/Library/CloudStorage/<domain>/`, Apple's
//! documented location for File Provider storage since macOS 12.3. Everything else
//! keeps attempting the trash and lets the typed `TrashRefusalKind` refusal speak.
//!
//! ❌ Don't gate on the volume either. A File Provider folder is NOT its own
//! volume: `~/Library/CloudStorage/Dropbox` and `~` report the same device on the
//! same `/dev/disk3s5` (verified on macOS 27.0, `stat -f '%d'`, 2026-09-17), so
//! `trash_dir_for_path()` answers `~/.Trash` for these paths and can't see this.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
use crate::file_system::cloud_provider::{CloudProvider, locate};

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
    /// Every selected item sits in a cloud-storage drive with no trash of its own.
    /// Run the permanent-delete flow, whose dialog says why.
    PermanentDeleteCloudStorage,
}

/// Whether `path` sits strictly inside a third-party cloud drive under
/// `~/Library/CloudStorage/<domain>/`.
///
/// Pure (path comparison only, no disk) and `home`-injected, so it's unit-testable
/// without a real cloud folder. `path` must already be symlink-resolved; see
/// [`routing_for`].
///
/// Three deliberate exclusions:
/// - **iCloud Drive** (`~/Library/Mobile Documents/`): Finder trashes from there
///   fine, so it keeps today's behavior. If a provider ever refuses, the typed
///   refusal path already catches it.
/// - **The `CloudStorage` container itself**, which holds no user data.
/// - **A drive's own root** (`…/CloudStorage/Dropbox`): deleting a whole cloud
///   root permanently would take the account's entire local copy with it, so that
///   one stays with the OS trash and its refusal.
#[cfg(target_os = "macos")]
pub fn is_inside_trashless_cloud_drive(home: &Path, path: &Path) -> bool {
    let Some(found) = locate(home, path) else {
        return false;
    };
    found.provider != CloudProvider::ICloudDrive && path != found.root
}

/// `~/Library/CloudStorage` is macOS's layout, so everywhere else this is simply
/// "an ordinary location": attempt the trash and let it answer.
#[cfg(not(target_os = "macos"))]
pub fn is_inside_trashless_cloud_drive(_home: &Path, _path: &Path) -> bool {
    false
}

/// The routing for a whole selection, against an injected `home` and paths that
/// are resolved here.
///
/// **All or nothing**: a mixed selection keeps today's behavior, so a person never
/// gets a permanent delete for the items that would have trashed fine. An empty
/// selection is [`TrashRouting::Trash`] for the same reason.
pub fn routing_for(home: &Path, paths: &[PathBuf]) -> TrashRouting {
    let all_trashless = !paths.is_empty()
        && paths.iter().all(|path| {
            resolve_through_symlinks(path).is_some_and(|resolved| is_inside_trashless_cloud_drive(home, &resolved))
        });
    if all_trashless {
        TrashRouting::PermanentDeleteCloudStorage
    } else {
        TrashRouting::Trash
    }
}

/// The routing for a selection, against the real home directory.
///
/// Answers [`TrashRouting::Trash`] when there's no readable home directory: an
/// unanswerable question means today's behavior, never a permanent delete.
pub fn routing_for_selection(paths: &[PathBuf]) -> TrashRouting {
    let Some(home) = dirs::home_dir() else {
        return TrashRouting::Trash;
    };
    // `~` itself can be a symlink (or sit behind a firmlink), and the resolved
    // paths below are canonical, so the prefix they're tested against has to be too.
    let home = std::fs::canonicalize(&home).unwrap_or(home);
    routing_for(&home, paths)
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
    use std::fs;

    fn home() -> PathBuf {
        PathBuf::from("/Users/test")
    }

    #[test]
    fn an_item_inside_a_cloud_drive_is_trashless() {
        assert!(is_inside_trashless_cloud_drive(
            &home(),
            Path::new("/Users/test/Library/CloudStorage/Dropbox/Work/emclient.pkg")
        ));
        assert!(is_inside_trashless_cloud_drive(
            &home(),
            Path::new("/Users/test/Library/CloudStorage/GoogleDrive-me@gmail.com/My Drive/notes.md")
        ));
        // An unrecognized provider is still a File Provider drive.
        assert!(is_inside_trashless_cloud_drive(
            &home(),
            Path::new("/Users/test/Library/CloudStorage/MacDroid-Pixel/DCIM/a.jpg")
        ));
    }

    #[test]
    fn ordinary_locations_keep_the_trash() {
        for path in [
            "/Users/test/Documents/report.pdf",
            "/Users/test/Library/Preferences/x.plist",
            "/Volumes/USB/photo.jpg",
            "/",
        ] {
            assert!(
                !is_inside_trashless_cloud_drive(&home(), Path::new(path)),
                "{path} must keep today's trash behavior"
            );
        }
    }

    /// The container holds no user data, and a whole cloud root is too much to
    /// delete permanently on a keystroke.
    #[test]
    fn neither_the_container_nor_a_drives_own_root_routes() {
        assert!(!is_inside_trashless_cloud_drive(
            &home(),
            Path::new("/Users/test/Library/CloudStorage")
        ));
        assert!(!is_inside_trashless_cloud_drive(
            &home(),
            Path::new("/Users/test/Library/CloudStorage/Dropbox")
        ));
    }

    /// Finder trashes from iCloud Drive fine, so it's deliberately out of scope.
    #[test]
    fn icloud_drive_is_left_alone() {
        assert!(!is_inside_trashless_cloud_drive(
            &home(),
            Path::new("/Users/test/Library/Mobile Documents/com~apple~CloudDocs/Projects/notes.md")
        ));
    }

    #[test]
    fn a_selection_routes_only_when_every_item_is_in_a_cloud_drive() {
        let cloud_a = PathBuf::from("/Users/test/Library/CloudStorage/Dropbox/a.pdf");
        let cloud_b = PathBuf::from("/Users/test/Library/CloudStorage/Dropbox/b.pdf");
        let ordinary = PathBuf::from("/Users/test/Documents/c.pdf");

        assert_eq!(
            routing_for(&home(), &[cloud_a.clone(), cloud_b.clone()]),
            TrashRouting::PermanentDeleteCloudStorage
        );
        assert_eq!(
            routing_for(&home(), &[cloud_a.clone(), ordinary.clone()]),
            TrashRouting::Trash,
            "a mixed selection keeps today's behavior"
        );
        assert_eq!(routing_for(&home(), &[ordinary]), TrashRouting::Trash);
        assert_eq!(routing_for(&home(), &[]), TrashRouting::Trash, "nothing selected");
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

        assert_eq!(
            routing_for(&home, &[home.join("Dropbox/Work/report.pdf")]),
            TrashRouting::PermanentDeleteCloudStorage
        );
        // And a leaf that's already gone resolves through its parent.
        assert_eq!(
            routing_for(&home, &[home.join("Dropbox/Work/vanished.pdf")]),
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

        assert_eq!(routing_for(&home, &[link]), TrashRouting::Trash);
    }

    #[test]
    fn an_unresolvable_path_keeps_the_trash() {
        for path in ["/", "mtp-1234://Internal storage/DCIM/a.jpg"] {
            assert_eq!(
                routing_for(&home(), &[PathBuf::from(path)]),
                TrashRouting::Trash,
                "{path} must not route"
            );
        }
    }
}
