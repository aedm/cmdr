//! What `hdiutil` and `diskutil` say about attached images and disk nodes, and the
//! two pure decisions the runner makes from it: does a target belong to OUR image,
//! and may we force-detach it.
//!
//! Everything here is decided on plist fields: an image's `image-path`, a node's
//! `dev-entry`, `ParentWholeDisk`, `APFSPhysicalStores`, and `MountPoint`. ❌ Never on
//! a tool's stdout text or a node name's shape.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// `hdiutil info -plist`: every image attached on the machine, ours or not.
#[derive(Debug, Deserialize)]
pub(super) struct AttachedImages {
    #[serde(default)]
    pub images: Vec<AttachedImage>,
}

/// One image in `hdiutil info -plist`.
#[derive(Debug, Deserialize)]
pub(super) struct AttachedImage {
    /// The backing file, spelled exactly as it was passed to `hdiutil attach`.
    #[serde(rename = "image-path")]
    pub image_path: PathBuf,
    #[serde(rename = "system-entities", default)]
    pub entities: Vec<SystemEntity>,
}

/// One node of an image, as both `hdiutil attach -plist` and `hdiutil info -plist`
/// list it.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct SystemEntity {
    /// `/dev/diskN` or `/dev/diskNsM`.
    #[serde(rename = "dev-entry")]
    pub dev_entry: String,
    #[serde(rename = "mount-point")]
    pub mount_point: Option<PathBuf>,
}

/// The fields of `diskutil info -plist` the harness reads.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct DiskFacts {
    /// `diskN` or `diskNsM`, no `/dev/`.
    pub device_identifier: String,
    /// The whole disk this node is part of; a whole disk names itself. For an APFS
    /// volume it's the SYNTHESIZED container, not the image's physical disk.
    pub parent_whole_disk: String,
    /// Present on an APFS container and its volumes: the partition the container
    /// lives on.
    #[serde(rename = "APFSPhysicalStores", default)]
    pub apfs_physical_stores: Vec<PhysicalStore>,
    /// The container of an APFS volume.
    #[serde(rename = "APFSContainerReference")]
    pub apfs_container_reference: Option<String>,
    /// Empty when the node isn't mounted.
    #[serde(default)]
    pub mount_point: String,
    #[serde(default)]
    pub volume_name: String,
}

/// One entry of `APFSPhysicalStores`.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct PhysicalStore {
    #[serde(rename = "APFSPhysicalStore")]
    pub node: String,
}

/// What a mutating call is aimed at.
#[derive(Debug, Clone, Copy)]
pub(super) enum Target<'a> {
    /// A BSD node without `/dev/`: `disk5`, `disk6s2`.
    Node(&'a str),
    /// A volume's mount point.
    MountPoint(&'a Path),
}

/// Why the runner refused to run a mutating call.
///
/// DiskArbitration hands a freed BSD unit to the next disk at once, so a node this
/// test stored a second ago can already be someone's Time Machine drive: every
/// refusal here is the harness declining to touch a disk it can't prove is ours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// No attached image has our backing file: it's already detached.
    ImageNotAttached,
    /// `diskutil` doesn't know the target.
    UnknownTarget {
        /// The node or path asked about.
        target: String,
    },
    /// The path isn't a mount point any more, so `diskutil` answered for whatever
    /// volume holds it.
    NotAMountPoint {
        /// The path asked about.
        target: String,
        /// Where the volume `diskutil` answered for is mounted.
        mounted_at: String,
    },
    /// The walk from the target to its physical whole disk hit a node `diskutil`
    /// doesn't know.
    UnresolvedWhole {
        /// The node or path asked about.
        target: String,
        /// The node the walk couldn't read.
        stuck_at: String,
    },
    /// The target's node, or the physical whole disk under it, isn't listed under
    /// our image.
    NotOurs {
        /// The node or path asked about.
        target: String,
        /// The target's own node.
        node: String,
        /// The physical whole disk it resolved to.
        whole: String,
    },
    /// `detach -force` on an image whose backing file lives on another attached
    /// image. Detach inner images first, without force.
    ForceOnNestedImage,
    /// `detach -force` on an image that another of this session's images is stored
    /// on. ❌ Never force an outer image.
    ForceOnOuterImage {
        /// The attached image stored on this one.
        inner: PathBuf,
    },
    /// The volume holding a backing file couldn't be read, so nesting is unknown.
    HostUnknown {
        /// The backing file asked about.
        image: PathBuf,
    },
    /// `hdiutil info` couldn't be read, so nothing can be proven ours.
    Unverifiable {
        /// What went wrong reading it.
        detail: String,
    },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

fn dev_node(identifier: &str) -> String {
    format!("/dev/{identifier}")
}

fn lists(image: &AttachedImage, dev_entry: &str) -> bool {
    image.entities.iter().any(|entity| entity.dev_entry == dev_entry)
}

/// Whether `target` belongs to the image backed by `our_image`: `diskutil` resolves
/// it to a node and that node's physical whole disk, and `hdiutil` must list BOTH
/// under our `image-path`. A mount-point target must also still be a mount point.
pub(super) fn check_ownership(
    target: Target<'_>,
    our_image: &Path,
    attached: &AttachedImages,
    disk_facts: &mut dyn FnMut(&str) -> Option<DiskFacts>,
) -> Result<(), Refusal> {
    let ours = attached
        .images
        .iter()
        .find(|image| image.image_path == our_image)
        .ok_or(Refusal::ImageNotAttached)?;
    let asked = match target {
        Target::Node(node) => node.to_string(),
        Target::MountPoint(path) => path.to_string_lossy().into_owned(),
    };
    let facts = disk_facts(&asked).ok_or_else(|| Refusal::UnknownTarget { target: asked.clone() })?;
    if let Target::MountPoint(path) = target
        && Path::new(&facts.mount_point) != path
    {
        return Err(Refusal::NotAMountPoint {
            target: asked,
            mounted_at: facts.mount_point,
        });
    }
    let whole = physical_whole(&facts, disk_facts).map_err(|stuck_at| Refusal::UnresolvedWhole {
        target: asked.clone(),
        stuck_at,
    })?;
    if lists(ours, &dev_node(&facts.device_identifier)) && lists(ours, &dev_node(&whole)) {
        Ok(())
    } else {
        Err(Refusal::NotOurs {
            target: asked,
            node: facts.device_identifier,
            whole,
        })
    }
}

/// The physical whole disk under `facts`: a partition's parent, or for an APFS
/// container and its volumes, the parent of the container's physical store.
/// `Err` names the node `diskutil` couldn't read.
pub(super) fn physical_whole(
    facts: &DiskFacts,
    disk_facts: &mut dyn FnMut(&str) -> Option<DiskFacts>,
) -> Result<String, String> {
    let whole = if facts.parent_whole_disk == facts.device_identifier {
        facts.clone()
    } else {
        disk_facts(&facts.parent_whole_disk).ok_or_else(|| facts.parent_whole_disk.clone())?
    };
    let Some(store) = whole.apfs_physical_stores.first() else {
        return Ok(whole.device_identifier);
    };
    let store_facts = disk_facts(&store.node).ok_or_else(|| store.node.clone())?;
    Ok(store_facts.parent_whole_disk)
}

/// Whether `detach -force` may run on the image backed by `our_image`: it's
/// attached, it isn't stored on an attached image, and none of `session_images`
/// is stored on it. `host_node` answers the `/dev/` node of the volume holding a
/// backing file (`statfs`), and is only ever asked about this session's own files.
pub(super) fn check_force_detach(
    our_image: &Path,
    attached: &AttachedImages,
    session_images: &[PathBuf],
    host_node: &mut dyn FnMut(&Path) -> Option<String>,
) -> Result<(), Refusal> {
    let ours = attached
        .images
        .iter()
        .find(|image| image.image_path == our_image)
        .ok_or(Refusal::ImageNotAttached)?;
    let host = host_node(our_image).ok_or_else(|| Refusal::HostUnknown {
        image: our_image.to_path_buf(),
    })?;
    if attached.images.iter().any(|image| lists(image, &host)) {
        return Err(Refusal::ForceOnNestedImage);
    }
    let attached_inner = session_images
        .iter()
        .filter(|path| path.as_path() != our_image)
        .filter(|path| attached.images.iter().any(|image| image.image_path == **path));
    for inner in attached_inner {
        let inner_host = host_node(inner).ok_or_else(|| Refusal::HostUnknown { image: inner.clone() })?;
        if lists(ours, &inner_host) {
            return Err(Refusal::ForceOnOuterImage { inner: inner.clone() });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const OURS: &str = "/var/folders/xy/T/cmdr_disk_image_a/image.sparseimage";

    /// Trimmed from a real `hdiutil info -plist` (macOS 26.6.2, 2026-09-14): a
    /// Finder-opened DMG, then a two-volume APFS sparse image of ours.
    const HDIUTIL_INFO: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>images</key><array>
    <dict>
      <key>image-path</key><string>/Users/someone/Downloads/App.dmg</string>
      <key>system-entities</key><array>
        <dict><key>dev-entry</key><string>/dev/disk4</string></dict>
        <dict><key>dev-entry</key><string>/dev/disk4s1</string><key>mount-point</key><string>/Volumes/App</string></dict>
      </array>
    </dict>
    <dict>
      <key>image-path</key><string>/var/folders/xy/T/cmdr_disk_image_a/image.sparseimage</string>
      <key>system-entities</key><array>
        <dict><key>dev-entry</key><string>/dev/disk5</string></dict>
        <dict><key>dev-entry</key><string>/dev/disk5s1</string></dict>
        <dict><key>dev-entry</key><string>/dev/disk6</string></dict>
        <dict><key>dev-entry</key><string>/dev/disk6s1</string><key>mount-point</key><string>/Volumes/CMDR1</string></dict>
        <dict><key>dev-entry</key><string>/dev/disk6s2</string></dict>
      </array>
    </dict>
  </array>
</dict></plist>"#;

    /// Trimmed from a real `diskutil info -plist disk6s1` (same run).
    const DISKUTIL_VOLUME: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>APFSContainerReference</key><string>disk6</string>
  <key>APFSPhysicalStores</key><array><dict><key>APFSPhysicalStore</key><string>disk5s1</string></dict></array>
  <key>BusProtocol</key><string>Disk Image</string>
  <key>DeviceIdentifier</key><string>disk6s1</string>
  <key>MountPoint</key><string>/Volumes/CMDR1</string>
  <key>ParentWholeDisk</key><string>disk6</string>
  <key>VolumeName</key><string>CMDR1</string>
  <key>WholeDisk</key><false/>
</dict></plist>"#;

    fn attached() -> AttachedImages {
        plist::from_bytes(HDIUTIL_INFO.as_bytes()).expect("the recorded hdiutil info parses")
    }

    fn facts(node: &str, parent: &str, store: Option<&str>, mount_point: &str) -> DiskFacts {
        DiskFacts {
            device_identifier: node.to_string(),
            parent_whole_disk: parent.to_string(),
            apfs_physical_stores: store
                .map(|node| PhysicalStore { node: node.to_string() })
                .into_iter()
                .collect(),
            apfs_container_reference: None,
            mount_point: mount_point.to_string(),
            volume_name: String::new(),
        }
    }

    /// The disks of the recorded `hdiutil info`, as `diskutil` describes them.
    fn disks() -> HashMap<String, DiskFacts> {
        [
            facts("disk4", "disk4", None, ""),
            facts("disk4s1", "disk4", None, "/Volumes/App"),
            facts("disk5", "disk5", None, ""),
            facts("disk5s1", "disk5", None, ""),
            facts("disk6", "disk6", Some("disk5s1"), ""),
            facts("disk6s1", "disk6", Some("disk5s1"), "/Volumes/CMDR1"),
            facts("disk6s2", "disk6", Some("disk5s1"), ""),
            // The Mac's own Data volume, which answers for a plain folder under /Volumes.
            facts("disk3s5", "disk3", Some("disk0s2"), "/System/Volumes/Data"),
            facts("disk3", "disk3", Some("disk0s2"), ""),
            facts("disk0s2", "disk0", None, ""),
        ]
        .into_iter()
        .map(|facts| (facts.device_identifier.clone(), facts))
        .collect()
    }

    fn lookup(disks: HashMap<String, DiskFacts>) -> impl FnMut(&str) -> Option<DiskFacts> {
        move |target| {
            disks
                .get(target)
                .cloned()
                .or_else(|| disks.values().find(|facts| facts.mount_point == target).cloned())
        }
    }

    #[test]
    fn the_recorded_plists_parse_into_the_fields_the_harness_reads() {
        let attached = attached();
        assert_eq!(attached.images.len(), 2);
        assert_eq!(attached.images[1].image_path, Path::new(OURS));
        assert_eq!(
            attached.images[1].entities[3].mount_point.as_deref(),
            Some(Path::new("/Volumes/CMDR1"))
        );

        let volume: DiskFacts =
            plist::from_bytes(DISKUTIL_VOLUME.as_bytes()).expect("the recorded diskutil info parses");
        assert_eq!(volume.device_identifier, "disk6s1");
        assert_eq!(volume.parent_whole_disk, "disk6");
        assert_eq!(volume.apfs_physical_stores[0].node, "disk5s1");
        assert_eq!(volume.apfs_container_reference.as_deref(), Some("disk6"));
        assert_eq!(volume.mount_point, "/Volumes/CMDR1");
        assert_eq!(volume.volume_name, "CMDR1");
    }

    #[test]
    fn an_apfs_volume_resolves_through_its_container_and_store_to_the_image_disk() {
        let mut lookup = lookup(disks());
        let volume = lookup("disk6s2").expect("known");
        assert_eq!(physical_whole(&volume, &mut lookup), Ok("disk5".to_string()));
        let partition = lookup("disk4s1").expect("known");
        assert_eq!(physical_whole(&partition, &mut lookup), Ok("disk4".to_string()));
    }

    #[test]
    fn every_node_of_our_image_is_ours() {
        for node in ["disk5", "disk5s1", "disk6", "disk6s1", "disk6s2"] {
            let result = check_ownership(Target::Node(node), Path::new(OURS), &attached(), &mut lookup(disks()));
            assert_eq!(result, Ok(()), "{node}");
        }
        let result = check_ownership(
            Target::MountPoint(Path::new("/Volumes/CMDR1")),
            Path::new(OURS),
            &attached(),
            &mut lookup(disks()),
        );
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn a_node_of_someone_elses_image_is_refused() {
        let result = check_ownership(
            Target::Node("disk4s1"),
            Path::new(OURS),
            &attached(),
            &mut lookup(disks()),
        );
        assert!(matches!(result, Err(Refusal::NotOurs { .. })), "got {result:?}");
    }

    #[test]
    fn a_node_our_image_no_longer_lists_is_refused_even_on_the_same_whole_disk_number() {
        // DA reused disk6s3 for someone else's volume on a container that happens to
        // share our store's numbering: hdiutil doesn't list it under our image.
        let mut disks = disks();
        disks.insert("disk6s3".to_string(), facts("disk6s3", "disk6", Some("disk5s1"), ""));
        let result = check_ownership(
            Target::Node("disk6s3"),
            Path::new(OURS),
            &attached(),
            &mut lookup(disks),
        );
        assert!(matches!(result, Err(Refusal::NotOurs { .. })), "got {result:?}");
    }

    #[test]
    fn a_detached_image_refuses_everything() {
        let result = check_ownership(
            Target::Node("disk5"),
            Path::new("/var/folders/xy/T/cmdr_disk_image_b/image.dmg"),
            &attached(),
            &mut lookup(disks()),
        );
        assert_eq!(result, Err(Refusal::ImageNotAttached));
    }

    #[test]
    fn a_path_that_stopped_being_a_mount_point_is_refused() {
        // `diskutil info /Volumes/CMDR2` on a plain folder answers for the Data volume.
        let mut lookup = {
            let disks = disks();
            move |target: &str| {
                if target == "/Volumes/CMDR2" {
                    disks.get("disk3s5").cloned()
                } else {
                    disks.get(target).cloned()
                }
            }
        };
        let result = check_ownership(
            Target::MountPoint(Path::new("/Volumes/CMDR2")),
            Path::new(OURS),
            &attached(),
            &mut lookup,
        );
        assert!(matches!(result, Err(Refusal::NotAMountPoint { .. })), "got {result:?}");
    }

    #[test]
    fn a_target_diskutil_doesnt_know_is_refused() {
        let result = check_ownership(
            Target::Node("disk9"),
            Path::new(OURS),
            &attached(),
            &mut lookup(disks()),
        );
        assert!(matches!(result, Err(Refusal::UnknownTarget { .. })), "got {result:?}");
    }

    #[test]
    fn a_plain_image_of_ours_may_be_forced() {
        let mut host = |_: &Path| Some("/dev/disk3s5".to_string());
        assert_eq!(
            check_force_detach(Path::new(OURS), &attached(), &[PathBuf::from(OURS)], &mut host),
            Ok(())
        );
    }

    #[test]
    fn an_image_stored_on_an_attached_image_is_never_forced() {
        let mut host = |_: &Path| Some("/dev/disk4s1".to_string());
        assert_eq!(
            check_force_detach(Path::new(OURS), &attached(), &[PathBuf::from(OURS)], &mut host),
            Err(Refusal::ForceOnNestedImage)
        );
    }

    #[test]
    fn an_image_another_of_ours_is_stored_on_is_never_forced() {
        let inner = PathBuf::from("/Volumes/CMDR1/inner.dmg");
        let mut attached = attached();
        attached.images.push(AttachedImage {
            image_path: inner.clone(),
            entities: vec![SystemEntity {
                dev_entry: "/dev/disk7".to_string(),
                mount_point: None,
            }],
        });
        let mut host = |image: &Path| Some(if image == inner { "/dev/disk6s1" } else { "/dev/disk3s5" }.to_string());
        assert_eq!(
            check_force_detach(
                Path::new(OURS),
                &attached,
                &[PathBuf::from(OURS), inner.clone()],
                &mut host
            ),
            Err(Refusal::ForceOnOuterImage { inner })
        );
    }

    #[test]
    fn force_is_refused_when_the_host_volume_is_unknown() {
        let mut host = |_: &Path| None;
        assert!(matches!(
            check_force_detach(Path::new(OURS), &attached(), &[PathBuf::from(OURS)], &mut host),
            Err(Refusal::HostUnknown { .. })
        ));
    }
}
