//! Attached-volume enumeration: snapshotting the kernel mount table via the
//! non-blocking `getfsstat`, filtering it to the mounts a user should see a row
//! for, and enriching only local mounts (network mounts stay non-blocking so a
//! hung mount can't stall discovery). See `DETAILS.md` § "Hung mounts" and
//! § "Which mounts get a row".

use super::{
    LocationCategory, LocationInfo, SmbMountInfo, disk_image, get_bool_resource, get_icon_for_path, get_volume_name,
    get_volume_uuid, is_network_fs_type, is_smb_fs_type, parse_smb_mount_source, supports_trash_for_fs_type,
    volume_id_for, volume_name_from_path,
};
use cmdr_fs::volume::canonical_root::collapse_by_volume_id;
use std::path::Path;

/// One entry from the kernel mount table, as returned by `getfsstat(MNT_NOWAIT)`.
///
/// Every field comes straight out of `statfs` with no follow-up syscall, so
/// building one never talks to the backing filesystem. That is the whole reason
/// discovery uses `getfsstat` instead of NSFileManager's volume enumeration: the
/// enumeration `getattrlist`s every mount, which blocks 30s–forever on a hung
/// network mount and froze the app at launch. See `DETAILS.md` § "Hung mounts".
struct MountEntry {
    /// Mount point, e.g. `/Volumes/naspi` (`f_mntonname`).
    mount_point: String,
    /// Filesystem type, e.g. `apfs`, `exfat`, `smbfs` (`f_fstypename`).
    fs_type: String,
    /// Mount source, e.g. `//david@192.168.1.111/naspi` for SMB (`f_mntfromname`).
    mount_from: String,
    /// Whether the mount carries the `MNT_RDONLY` flag.
    is_read_only: bool,
    /// Whether the OS says this mount belongs in a file manager's sidebar, i.e.
    /// it does NOT carry `MNT_DONTBROWSE`. Finder's own rule, and the one that
    /// separates real drives from the plumbing (`devfs`, `/System/Volumes/*`,
    /// `/Volumes/Recovery`, autofs triggers) without naming any of them.
    is_browsable: bool,
    /// The mounted filesystem's identity (`f_fsid`, its two words packed high then
    /// low). Renaming a mounted volume moves the mount point and keeps this.
    fsid: u64,
}

/// Snapshot the kernel mount table without blocking on any mount.
///
/// `getfsstat(MNT_NOWAIT)` returns cached mount metadata and never round-trips to
/// a filesystem, so a wedged network mount can't stall it — unlike `MNT_WAIT`,
/// which is what makes plain `df` hang.
///
/// ❗ **`None` when the syscall wouldn't answer, ❌ never an empty list.** A live
/// system always lists `/`, so an empty table is a failed read wearing a positive
/// answer's clothes: every caller above reads "nothing is mounted", and the one that
/// groups a disk's volumes then lets an unmount take a sibling down under its live
/// FSEvents watcher. Callers name the third answer (`is_mount_point`,
/// `has_mount_identity`, `mount_sources`) or fold it deliberately.
fn enumerate_mounts() -> Option<Vec<MountEntry>> {
    // First pass: ask how many mounts exist (null buffer writes nothing).
    // SAFETY: `getfsstat(NULL, 0, flags)` is the documented count query; with a
    // null buffer and zero size the kernel only returns the mount count.
    let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if count <= 0 {
        return None;
    }

    // A few slots of slack in case a mount appears between the two calls.
    let capacity = count as usize + 4;
    let mut buf: Vec<libc::statfs> = Vec::with_capacity(capacity);
    let bufsize = (capacity * size_of::<libc::statfs>()) as libc::c_int;
    // SAFETY: `buf` has room for `capacity` `statfs` records and `bufsize` matches
    // that byte length, so the kernel fills at most `capacity` records and returns
    // how many it wrote.
    let filled = unsafe { libc::getfsstat(buf.as_mut_ptr(), bufsize, libc::MNT_NOWAIT) };
    if filled <= 0 {
        return None;
    }
    // SAFETY: the kernel initialized `filled` records; clamp to our capacity in
    // case the mount table grew past the slack between the two calls.
    unsafe { buf.set_len((filled as usize).min(capacity)) };

    Some(
        buf.iter()
            .map(|s| MountEntry {
                mount_point: cstr_field_to_string(&s.f_mntonname),
                fs_type: cstr_field_to_string(&s.f_fstypename),
                mount_from: cstr_field_to_string(&s.f_mntfromname),
                is_read_only: (s.f_flags & libc::MNT_RDONLY as u32) != 0,
                is_browsable: (s.f_flags & libc::MNT_DONTBROWSE as u32) == 0,
                fsid: packed_fsid(s),
            })
            .collect(),
    )
}

/// Convert a NUL-terminated `c_char` array from `statfs` into a `String`
/// (UTF-8 lossy, since mount points and volume names can be non-ASCII).
fn cstr_field_to_string(field: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = field.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A `statfs` record's `f_fsid`, its two words packed high then low into one `u64`.
fn packed_fsid(stat: &libc::statfs) -> u64 {
    // SAFETY: libc declares `fsid_t` `#[repr(C)]` around exactly one `[i32; 2]` and
    // only keeps that field private, so reading the value as the array reads the field.
    let [high, low]: [i32; 2] = unsafe { std::mem::transmute(stat.f_fsid) };
    (u64::from(high.cast_unsigned()) << 32) | u64::from(low.cast_unsigned())
}

/// Whether `path` is a mount point in the kernel mount table. Reads the same
/// non-blocking `getfsstat` snapshot discovery does, so a hung mount can't stall
/// it. `None` when the table couldn't be read (a live system always lists `/`).
pub(crate) fn is_mount_point(path: &str) -> Option<bool> {
    let mounts = enumerate_mounts()?;
    let path = Path::new(path);
    Some(mounts.iter().any(|m| Path::new(&m.mount_point) == path))
}

/// The identity of the filesystem mounted exactly at `path`, from the same
/// non-blocking table: `None` when nothing is mounted there or the table couldn't be
/// read. With mounts stacked on one path, the last one listed is the one a lookup
/// reaches.
pub(crate) fn mount_identity_at(path: &str) -> Option<u64> {
    let path = Path::new(path);
    enumerate_mounts()?
        .into_iter()
        .rev()
        .find(|m| Path::new(&m.mount_point) == path)
        .map(|m| m.fsid)
}

/// Every mount point the kernel currently lists, from the same non-blocking snapshot.
///
/// What the volume REGISTRY sweeps (`file_system::volume::mount_registration`), which is a
/// different question from what the switcher shows: resolution can mint an ID for any of these,
/// so registration has to cover all of them. ❗ `None` when the table couldn't be read, ❌ never an
/// empty list, or the sweep would read a machine with no mounts and register nothing.
pub(crate) fn mount_roots() -> Option<Vec<String>> {
    Some(enumerate_mounts()?.into_iter().map(|mount| mount.mount_point).collect())
}

/// A mount's point and the source it was mounted from (`f_mntfromname`), for code that maps
/// mounts to the disks under them. Straight from the non-blocking snapshot: no syscall per entry.
pub(crate) struct MountSource {
    /// Mount point, e.g. `/Volumes/naspi`.
    pub(crate) mount_point: std::path::PathBuf,
    /// Mount source, e.g. `/dev/disk5s1` for a local disk.
    pub(crate) mount_from: String,
}

/// Every mount's point and source, from the same non-blocking `getfsstat` snapshot discovery reads.
///
/// ❗ `None` when the table couldn't be read, ❌ never an empty list: this is what maps mounts to the
/// disks under them, so an empty answer reads as "nothing is on this disk" and lets a whole-disk
/// unmount past a sibling nobody stopped.
pub(crate) fn mount_sources() -> Option<Vec<MountSource>> {
    Some(
        enumerate_mounts()?
            .into_iter()
            .map(|mount| MountSource {
                mount_point: std::path::PathBuf::from(mount.mount_point),
                mount_from: mount.mount_from,
            })
            .collect(),
    )
}

/// Every SMB mount the kernel lists, as `(mount point, what the mount source says)`,
/// from the same non-blocking `getfsstat` snapshot: no syscall per entry, so a hung
/// share can't stall it and nothing goes out on the network.
///
/// The SMB upgrade paths ask this, ❌ never the volume registry: the registry fills on
/// a background thread that `statfs`es every mount, so at launch it lags the kernel
/// by seconds, and a pass that asked it read "no SMB mounts" with four of them up.
/// `None` when the table couldn't be read.
pub(crate) fn smb_mounts() -> Option<Vec<(String, SmbMountInfo)>> {
    Some(
        enumerate_mounts()?
            .into_iter()
            .filter_map(|mount| {
                let info = smb_info(&mount)?;
                Some((mount.mount_point, info))
            })
            .collect(),
    )
}

/// Whether any filesystem in the kernel mount table has identity `fsid`: `None` when
/// the table couldn't be read. The index asks this, ❌ never [`is_mount_point`], to
/// tell a drive that's gone from one a rename moved.
pub(crate) fn has_mount_identity(fsid: u64) -> Option<bool> {
    let mounts = enumerate_mounts()?;
    Some(mounts.iter().any(|m| m.fsid == fsid))
}

/// Whether a mount should surface as a drive in the switcher.
///
/// ❗ A DISPLAY question, and the strict one. The volume REGISTRY sweeps the whole
/// mount table regardless (`file_system::volume::mount_registration`), because
/// resolution can mint an ID for any row of it; a mount this drops is still
/// openable, it just doesn't get a row of its own.
///
/// Two ways in, and the first is the OS's own answer:
///
/// 1. **Browsable**: the mount doesn't carry `MNT_DONTBROWSE`, which is exactly
///    what keeps `devfs`, `/System/Volumes/*`, `/Volumes/Recovery`, and autofs
///    triggers out of Finder's sidebar. Reading the flag beats the `/Volumes/`
///    prefix test it replaces: that one both let `/Volumes/Recovery` through (a
///    name check caught it) and hid every drive mounted anywhere else.
/// 2. **Mounted inside the user's home folder**, browsable or not. That's where
///    cloud clients put their drives (pCloud's `~/pCloud Drive`), some of which
///    mark the mount unbrowsable and add their own Finder sidebar shortcut
///    instead. No system mount lives under `$HOME`, so this can't readmit
///    plumbing. `$HOME` ITSELF is excluded: a network-homed Mac mounts it, and
///    the home folder is not a drive.
///
/// Never the boot volume (it has its own row), never a dot-prefixed mount, and
/// never a `~/Library/CloudStorage` path, which the cloud-drive arm publishes and
/// would otherwise appear twice. Pure, so it's unit-testable.
fn is_user_facing_mount(path: &str, is_browsable: bool, home: &Path) -> bool {
    let mount = Path::new(path);
    if mount == Path::new("/") {
        return false;
    }
    if path.contains("/Library/CloudStorage") {
        return false;
    }
    // Hidden mount (leading dot on the last component), e.g. `/Volumes/.timemachine`.
    if let Some(name) = mount.file_name().and_then(|n| n.to_str())
        && name.starts_with('.')
    {
        return false;
    }
    if !is_browsable && !(mount.starts_with(home) && mount != home) {
        return false;
    }
    true
}

/// Metadata for a LOCAL mount, resolved via blocking macOS APIs (NSURL +
/// DiskArbitration + NSWorkspace). Only computed for local mounts — network
/// mounts skip it so a hung mount never blocks discovery.
struct LocalVolumeMeta {
    name: String,
    is_ejectable: bool,
    icon: Option<String>,
    is_disk_image: bool,
    /// The volume's filesystem UUID, what its ID keys on. Gathered here because
    /// this struct is exactly the "blocking, local mounts only" seam the NSURL
    /// lookup belongs behind.
    uuid: Option<String>,
}

/// The SMB mount info for a network mount, or `None` if it isn't an SMB mount we
/// can parse. Read straight out of the `getfsstat` snapshot, no syscall.
fn smb_info(mount: &MountEntry) -> Option<SmbMountInfo> {
    is_smb_fs_type(Some(&mount.fs_type))
        .then(|| parse_smb_mount_source(&mount.mount_from))
        .flatten()
}

/// The display name for a network mount, from the non-blocking `f_mntfromname`.
/// SMB mounts read as "share on server"; other network mounts fall back to the
/// path. No syscalls: everything comes from the `getfsstat` snapshot. Pure.
fn network_name(mount: &MountEntry) -> String {
    match smb_info(mount) {
        Some(info) => {
            let display = crate::network::smb_server_address::friendly_server_name(&info.server);
            format!("{} on {}", info.share, display)
        }
        None => volume_name_from_path(&mount.mount_point),
    }
}

/// Classify one mount-table entry into a switcher [`LocationInfo`], or `None` if
/// it isn't a user-facing attached volume.
///
/// Network mounts (SMB, NFS, WebDAV, …) are built purely from the non-blocking
/// `statfs` data already in `mount`; `resolve_local` is NOT called for them. That
/// is the guarantee a hung network mount can't block discovery of the other
/// volumes. Local mounts call `resolve_local` for their name, ejectability, icon,
/// and disk-image status (safe: local disks don't hang). Splitting the blocking
/// enrichment behind a closure also keeps the classification unit-testable.
fn build_attached_location(
    mount: &MountEntry,
    home: &Path,
    resolve_local: impl FnOnce(&str) -> LocalVolumeMeta,
) -> Option<LocationInfo> {
    let path = mount.mount_point.as_str();
    if !is_user_facing_mount(path, mount.is_browsable, home) {
        return None;
    }
    let fs_type = mount.fs_type.clone();
    let supports_trash = supports_trash_for_fs_type(Some(&fs_type));

    let (id, name, is_ejectable, icon, is_disk_image) = if is_network_fs_type(Some(&fs_type)) {
        // Network mount: derive everything from the non-blocking snapshot. Never
        // touch NSURL / NSWorkspace / DiskArbitration here — those are exactly the
        // calls that hang on a dead mount (which is also why the ID gets no UUID
        // to key on). `is_ejectable` is cosmetically moot for network mounts (the
        // eject affordance keys on `connectionState`, and the eject flow forces
        // it true), so a safe `false` costs nothing.
        let id = volume_id_for(path, Some(&fs_type), smb_info(mount).as_ref(), None);
        (id, network_name(mount), false, None, false)
    } else {
        // Local mount: safe to run the blocking enrichment.
        let meta = resolve_local(path);
        (
            volume_id_for(path, Some(&fs_type), None, meta.uuid.as_deref()),
            meta.name,
            meta.is_ejectable,
            meta.icon,
            meta.is_disk_image,
        )
    };

    // A mount a cloud provider serves is grouped with the other cloud drives and
    // kept out of the index affordances, whether it sits in `/Volumes` or in the
    // home folder. `is_cloud_mount` travels as its own typed field: ❌ never
    // re-derive it downstream from the category or the fs type.
    let is_cloud_mount = super::is_cloud_provider_mount(path);
    let category = match is_cloud_mount {
        true => LocationCategory::CloudDrive,
        false => LocationCategory::AttachedVolume,
    };

    Some(LocationInfo {
        id,
        name,
        path: path.to_string(),
        category,
        icon,
        is_ejectable,
        fs_type: Some(fs_type),
        supports_trash,
        mount_is_read_only: mount.is_read_only,
        is_disk_image,
        is_cloud_mount,
        connection_state: None,
        pinned: None,
        landing_path: None,
        device_readiness: None,
        usb_speed: None,
        capabilities: None,
        favorite_shortcut: None,
    })
}

/// Get attached volumes (external drives, USB, network mounts, etc.).
///
/// Enumerates via the non-blocking `getfsstat` snapshot, then enriches only LOCAL
/// mounts through blocking macOS APIs. A hung network mount contributes its
/// getfsstat-derived entry and never blocks the others. See `DETAILS.md`
/// § "Hung mounts".
pub fn get_attached_volumes() -> Vec<LocationInfo> {
    use objc2::rc::autoreleasepool;
    use objc2_foundation::{NSString, NSURL};

    let home = dirs::home_dir().unwrap_or_default();

    // Drain autoreleased ObjC objects from the per-local-mount NSURL enrichment.
    // Called from spawn_blocking / helper threads that lack AppKit's pool.
    autoreleasepool(|_| {
        // A table nobody could read means no rows this pass. The switcher keeps what it has and the
        // next discovery asks again; ❌ nothing downstream may read this list as "the disk is empty".
        let discovered: Vec<LocationInfo> = enumerate_mounts()
            .unwrap_or_default()
            .iter()
            .filter_map(|mount| {
                build_attached_location(mount, &home, |path| {
                    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
                    LocalVolumeMeta {
                        name: get_volume_name(&url, path),
                        is_ejectable: get_bool_resource(&url, "NSURLVolumeIsEjectableKey").unwrap_or(false),
                        icon: get_icon_for_path(path),
                        is_disk_image: disk_image::is_disk_image_mount(path),
                        uuid: get_volume_uuid(&url),
                    }
                })
            })
            .collect();

        let mut volumes = collapse_by_volume_id(discovered);
        // Sort alphabetically
        volumes.sort_by_key(|a| a.name.to_lowercase());
        volumes
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // Hung-mount guard: getfsstat-based discovery (Bug: dead mount froze launch)
    // ========================================================================

    /// A browsable mount, which is what every drive a user sees is. The
    /// unbrowsable case has its own helper, since it's the interesting one.
    fn mount(mount_point: &str, fs_type: &str, mount_from: &str, is_read_only: bool) -> MountEntry {
        MountEntry {
            mount_point: mount_point.to_string(),
            fs_type: fs_type.to_string(),
            mount_from: mount_from.to_string(),
            is_read_only,
            is_browsable: true,
            fsid: 0,
        }
    }

    /// A mount macOS marks `MNT_DONTBROWSE`: the plumbing, and a cloud client's
    /// drive that hides itself from Finder's sidebar.
    fn unbrowsable(mount_point: &str) -> MountEntry {
        MountEntry {
            is_browsable: false,
            ..mount(mount_point, "apfs", "x", false)
        }
    }

    /// The home folder these tests reason about. A literal, so the rule is
    /// pinned rather than re-derived from whoever runs the suite.
    fn home() -> &'static Path {
        Path::new("/Users/sven")
    }

    /// A `resolve_local` that fails the test if invoked. Used to prove a network
    /// mount is classified WITHOUT any blocking NSURL/DiskArbitration/NSWorkspace
    /// call — the guarantee that a hung mount can't stall discovery.
    fn forbidden_resolver(_path: &str) -> LocalVolumeMeta {
        panic!("resolve_local must NOT run for a network mount");
    }

    /// The OS's own sidebar rule decides, so the plumbing stays out without this
    /// module naming a single system path.
    #[test]
    fn the_browsable_flag_is_what_separates_drives_from_plumbing() {
        let browsable = |path: &str| is_user_facing_mount(path, true, home());
        let hidden = |path: &str| is_user_facing_mount(path, false, home());

        assert!(browsable("/Volumes/MyDrive"));
        assert!(browsable("/Volumes/naspi"));
        // Everything macOS marks `MNT_DONTBROWSE`: `/System/Volumes/*`, `devfs`,
        // the Recovery volume, autofs triggers. The old prefix filter needed a
        // name check for Recovery and missed the rest.
        assert!(!hidden("/System/Volumes/Data"));
        assert!(!hidden("/Volumes/Recovery"));
        assert!(!hidden("/dev"));
        // The boot volume has its own row, browsable or not.
        assert!(!browsable("/"));
        // A `~/Library/CloudStorage` folder is published by the cloud arm.
        assert!(!browsable("/Volumes/Foo/Library/CloudStorage/Dropbox"));
        // Hidden by name (NSFileManager's old SkipHiddenVolumes).
        assert!(!browsable("/Volumes/.timemachine"));
    }

    /// The pCloud case: a cloud client mounts its drive into the home folder, and
    /// some mark it unbrowsable and add their own Finder shortcut instead. It's
    /// still the user's drive, so the switcher shows it.
    #[test]
    fn a_mount_inside_the_home_folder_shows_even_when_unbrowsable() {
        assert!(is_user_facing_mount("/Users/sven/pCloud Drive", false, home()));
        assert!(is_user_facing_mount("/Users/sven/vaults/work", false, home()));
        // The home folder ITSELF is not a drive, and a network-homed Mac mounts it.
        assert!(!is_user_facing_mount("/Users/sven", false, home()));
        // Another account's home is not this user's drive either.
        assert!(!is_user_facing_mount("/Users/dori/pCloud Drive", false, home()));
        // A name-hidden mount stays hidden wherever it sits.
        assert!(!is_user_facing_mount("/Users/sven/.hidden-vault", false, home()));
    }

    #[test]
    fn smb_mount_classifies_without_blocking_enrichment() {
        // A wedged SMB mount must be classified purely from getfsstat data; the
        // blocking resolver must never run, so a dead NAS can't stall discovery.
        let m = mount("/Volumes/naspi", "smbfs", "//david@192.168.1.111/naspi", false);
        let loc = build_attached_location(&m, home(), forbidden_resolver).expect("SMB mount is an attached volume");

        assert_eq!(
            loc.id,
            crate::file_system::volume::smb_volume_id("192.168.1.111", 445, "naspi")
        );
        assert!(loc.name.contains("naspi"), "name shows the share: {}", loc.name);
        assert!(loc.name.contains(" on "), "name shows 'share on server': {}", loc.name);
        assert_eq!(loc.fs_type.as_deref(), Some("smbfs"));
        assert_eq!(loc.category, LocationCategory::AttachedVolume);
        assert!(!loc.is_ejectable, "network mounts take the safe non-blocking default");
        assert!(loc.icon.is_none());
        assert!(!loc.is_disk_image);
    }

    #[test]
    fn nfs_mount_classifies_without_blocking_enrichment() {
        let m = mount("/Volumes/export", "nfs", "server:/export", true);
        let loc = build_attached_location(&m, home(), forbidden_resolver).expect("NFS mount is an attached volume");
        assert_eq!(loc.id, crate::file_system::volume::path_volume_id("/Volumes/export"));
        assert_eq!(loc.name, "export");
        assert!(loc.mount_is_read_only, "MNT_RDONLY flag propagates from getfsstat");
        assert_eq!(loc.fs_type.as_deref(), Some("nfs"));
    }

    #[test]
    fn local_mount_runs_the_enrichment_closure() {
        // Local mounts DO get the (safe) blocking enrichment; here we inject a
        // fake so the test stays hermetic and asserts the values flow through.
        // The UUID coming back through this closure is what the ID keys on, so
        // the same disk keeps its ID when macOS remounts it as `/Volumes/USB 1`.
        let m = mount("/Volumes/USB", "exfat", "/dev/disk4s1", false);
        let resolve = |path: &str| {
            assert!(path.starts_with("/Volumes/USB"));
            LocalVolumeMeta {
                name: "My USB".to_string(),
                is_ejectable: true,
                icon: Some("icon-data".to_string()),
                is_disk_image: false,
                uuid: Some("A1B2-C3D4".to_string()),
            }
        };
        let loc = build_attached_location(&m, home(), resolve).expect("local mount is an attached volume");

        assert_eq!(
            loc.id,
            crate::file_system::volume::local_volume_id(Some("A1B2-C3D4"), "/Volumes/USB")
        );
        let remounted = mount("/Volumes/USB 1", "exfat", "/dev/disk4s1", false);
        let remounted_loc =
            build_attached_location(&remounted, home(), resolve).expect("local mount is an attached volume");
        assert_eq!(
            loc.id, remounted_loc.id,
            "the same disk keeps its ID at a new mount point"
        );

        assert_eq!(loc.name, "My USB");
        assert!(loc.is_ejectable);
        assert_eq!(loc.icon.as_deref(), Some("icon-data"));
        assert_eq!(loc.fs_type.as_deref(), Some("exfat"));
    }

    #[test]
    fn filtered_mount_yields_no_location() {
        // The boot volume and system mounts are dropped before any enrichment.
        assert!(
            build_attached_location(&mount("/", "apfs", "/dev/disk3s1", false), home(), forbidden_resolver).is_none()
        );
        assert!(build_attached_location(&unbrowsable("/System/Volumes/Data"), home(), forbidden_resolver).is_none());
    }

    // ========================================================================
    // One volume ID means one published mount root
    // ========================================================================

    #[test]
    fn two_mount_roots_for_one_share_collapse_to_the_shortest_path() {
        // What this covers is the WIRING: a real pair of mount-table entries for
        // one share derives one volume ID (a share keys on `(server, port,
        // share)`), and `get_attached_volumes`'s collapse turns that into one
        // published location at the original path, while a different volume stays
        // its own row. The collapse rule itself is proved on a toy struct in
        // `cmdr_fs::volume::canonical_root`.
        let first = mount("/Volumes/naspi", "smbfs", "//david@192.168.1.111/naspi", false);
        let second = mount("/Volumes/naspi-1", "smbfs", "//david@192.168.1.111/naspi", false);
        let nfs = mount("/Volumes/export", "nfs", "server:/export", true);
        let locations: Vec<LocationInfo> = [&second, &first, &nfs]
            .iter()
            .filter_map(|m| build_attached_location(m, home(), forbidden_resolver))
            .collect();
        assert_eq!(locations.len(), 3, "every mount starts out as its own location");

        let collapsed = collapse_by_volume_id(locations);
        assert_eq!(collapsed.len(), 2, "one volume ID publishes one location");
        assert_eq!(
            collapsed[0].path, "/Volumes/naspi",
            "the canonical root is the original mount, whatever order they arrive in"
        );
        assert_eq!(
            collapsed[1].path, "/Volumes/export",
            "a different volume stays separate"
        );
    }

    #[test]
    fn enumerate_mounts_finds_the_boot_volume() {
        // getfsstat should always return at least the root mount on a live system,
        // and it must never block (this test would hang if it did).
        let mounts = enumerate_mounts().expect("getfsstat answers on a live system");
        assert!(!mounts.is_empty(), "getfsstat returned no mounts");
        assert!(mounts.iter().any(|m| m.mount_point == "/"), "root mount missing");
    }

    #[test]
    fn is_mount_point_answers_from_the_mount_table() {
        assert_eq!(is_mount_point("/"), Some(true), "the boot volume is a mount point");
        // A folder ON a mount is not one: an eject can't mistake it for a live mount.
        assert_eq!(is_mount_point("/usr/bin"), Some(false));
    }
}
