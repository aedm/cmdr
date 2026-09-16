//! What a record IS: the shapes the log holds, the kinds that decide what may
//! be done to each one, and the path space each lives in.
//!
//! Split out of `in_flight_temps.rs` so the vocabulary reads on its own: the
//! ledger next door owns the log and the store, `in_flight_sweep.rs` owns the
//! rules, and every one of them speaks these types.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::file_system::write_operations::state::WriteOperationState;

/// Which path space a partial lives in, so a later launch reaches it through the
/// same one that wrote it.
///
/// ❌ Never guess this from the path. An absolute-looking path means one thing
/// on the local filesystem and quite another inside a share's own namespace, and
/// resolving the wrong one is how a sweep silently does nothing on a NAS — or,
/// worse, removes a local file the ledger never meant.
#[derive(Clone, Copy, Debug)]
pub(in crate::file_system::write_operations) enum TempHome<'a> {
    /// The local filesystem, addressed by an absolute OS path.
    ///
    /// Nothing mints one any more — the local engine records kinded items
    /// instead — but the shape stays, because it's what a `+` line an earlier
    /// build wrote means, and the replay tests write those lines through the
    /// same door the old builds did.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "the shape a replayed `+` line speaks; only the replay tests mint one"
        )
    )]
    LocalFs,
    /// One volume's own path space, keyed by the volume ID — the identity that
    /// survives a remount, so a record written last week still names the same
    /// share today.
    Volume(&'a str),
}

/// One thing an operation left on disk, as the log holds it.
///
/// Two shapes on purpose; the module docs say why a kinded record can't just be
/// a wider `+` line.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::file_system::write_operations) enum Record {
    /// A `+`/`-` line: a staged partial, with no kind of its own.
    Legacy(RecordedTemp),
    /// An `A`/`a` line: something whose kind decides what may be done to it.
    Tracked(TrackedItem),
}

impl Record {
    /// The byte that ADDS this record to the log. Its retire byte is the same
    /// letter in the other case, which is what keeps the two halves of a shape
    /// impossible to mismatch.
    pub(in crate::file_system::write_operations) fn add_op(&self) -> u8 {
        match self {
            Self::Legacy(_) => b'+',
            Self::Tracked(_) => b'A',
        }
    }

    pub(in crate::file_system::write_operations) fn retire_op(&self) -> u8 {
        match self {
            Self::Legacy(_) => b'-',
            Self::Tracked(_) => b'a',
        }
    }

    /// The record as its own JSON shape. `None` for a path that isn't UTF-8.
    pub(in crate::file_system::write_operations) fn encode(&self) -> Option<String> {
        match self {
            Self::Legacy(temp) => serde_json::to_string(temp).ok(),
            Self::Tracked(item) => serde_json::to_string(item).ok(),
        }
    }

    /// The volume this record waits on, or `None` for one the local filesystem
    /// can answer for on its own.
    pub(in crate::file_system::write_operations) fn volume_id(&self) -> Option<&str> {
        match self {
            Self::Legacy(RecordedTemp::Local(_)) => None,
            Self::Legacy(RecordedTemp::OnVolume(temp)) => Some(&temp.volume_id),
            Self::Tracked(item) => item.home.volume_id(),
        }
    }

    /// The path the record names, as the record holds it (relative to its
    /// volume's root for a mount-homed [`Record::Tracked`]).
    ///
    /// For the tests alone: the sweep reaches a path through [`Located`], which
    /// resolves the home rather than reading this raw.
    ///
    /// [`Located`]: super::in_flight_sweep
    #[cfg(test)]
    pub(in crate::file_system::write_operations) fn path(&self) -> &Path {
        match self {
            Self::Legacy(RecordedTemp::Local(path)) => path,
            Self::Legacy(RecordedTemp::OnVolume(temp)) => &temp.path,
            Self::Tracked(item) => &item.path,
        }
    }
}

/// One partial as the old lines hold it.
///
/// Serialized untagged, which is what makes the two shapes tell themselves
/// apart on disk and keeps a local record byte-identical to what every earlier
/// build wrote.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub(in crate::file_system::write_operations) enum RecordedTemp {
    /// A path on the local filesystem.
    ///
    /// This is also what a BARE-PATH line means: that's all the old one-field
    /// format could express correctly, since a volume path recorded without its
    /// volume resolves against the local filesystem — usually as nothing, and
    /// occasionally as somebody else's file.
    Local(PathBuf),
    /// A path in one volume's own space.
    OnVolume(VolumeTemp),
}

/// A partial living in a volume's path space.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(in crate::file_system::write_operations) struct VolumeTemp {
    /// The volume ID the path belongs to (`cmdr_fs::volume::ids`).
    pub(in crate::file_system::write_operations) volume_id: String,
    /// The path, in that volume's own space.
    pub(in crate::file_system::write_operations) path: PathBuf,
}

/// Something on disk that isn't the user's file under its own name yet, with the
/// kind that decides what a sweep may do to it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(in crate::file_system::write_operations) struct TrackedItem {
    /// What it is, and whatever that kind needs to decide its fate.
    #[serde(flatten)]
    pub(in crate::file_system::write_operations) kind: ItemKind,
    /// Which path space [`path`](Self::path) is in.
    pub(in crate::file_system::write_operations) home: ItemHome,
    /// Where it is: the absolute path for [`ItemHome::Local`], and a path
    /// relative to the volume's root otherwise, so a drive that comes back at a
    /// different mount point is still found.
    pub(in crate::file_system::write_operations) path: PathBuf,
}

/// What a recorded thing IS, which is what decides whether a sweep may remove
/// it, must put it back, or must leave it exactly where it is.
///
/// ❗ Every `destination` here is stored in the same path space as its item's
/// own path, because an aside is always a sibling of what it displaced.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(in crate::file_system::write_operations) enum ItemKind {
    /// A staged write: bytes on their way in, which nothing else has. A leftover
    /// is garbage.
    Temp,
    /// The file `overwrite::stage_and_land_file` renamed aside so a replacement
    /// could take its name, with the size that replacement was to end up at.
    /// The only kind whose destination can be checked against a number.
    FileAside {
        destination: PathBuf,
        /// The complete replacement's size in bytes, so the sweep can tell a
        /// landing that finished from one that stopped part-way.
        expected_size: u64,
    },
    /// The file `overwrite::displace_with_directory` moved aside so a folder
    /// could be built at its name, leaf by leaf over the rest of the copy.
    DisplacedFile { destination: PathBuf },
    /// The entry `overwrite::safe_overwrite_dir` set aside while its closure
    /// materialized the replacement.
    DirOverwriteAside { destination: PathBuf },
    /// The destination a same-volume Overwrite renamed aside to free the name
    /// for the rename that replaces it
    /// (`transfer/volume/displaced_destination.rs`).
    VolumeAside { destination: PathBuf },
    /// A cross-filesystem move's `.cmdr-staging-<op>` folder. ❗ The sweep only
    /// ever `remove_dir`s one: a destination that left before Phase 3 leaves a
    /// whole staged tree in here, and it can be the only copy.
    StagingDir,
}

impl ItemKind {
    /// Where this item belongs if it has to go back, for the kinds that
    /// displaced something.
    pub(in crate::file_system::write_operations) fn destination(&self) -> Option<&Path> {
        match self {
            Self::Temp | Self::StagingDir => None,
            Self::FileAside { destination, .. }
            | Self::DisplacedFile { destination }
            | Self::DirOverwriteAside { destination }
            | Self::VolumeAside { destination } => Some(destination),
        }
    }

    /// The same kind with its destination stated in another path space, for the
    /// hop from a producer's absolute paths into a record's.
    pub(in crate::file_system::write_operations) fn with_destination(self, moved: PathBuf) -> Self {
        match self {
            Self::FileAside { expected_size, .. } => Self::FileAside {
                destination: moved,
                expected_size,
            },
            Self::DisplacedFile { .. } => Self::DisplacedFile { destination: moved },
            Self::DirOverwriteAside { .. } => Self::DirOverwriteAside { destination: moved },
            Self::VolumeAside { .. } => Self::VolumeAside { destination: moved },
            other => other,
        }
    }
}

/// Which path space a [`TrackedItem`] lives in, and therefore which primitives
/// may be pointed at it.
///
/// ❗ The [`Mount`](Self::Mount) / [`VolumeSpace`](Self::VolumeSpace) split is
/// load-bearing, ❌ never derivable from the path or from the volume's root. A
/// direct SMB session roots at `/` in its OWN namespace, so a sweep that read
/// that as "the local filesystem, mounted" would point `std::fs::remove_file` at
/// `/photos/holiday.raw` on the user's Mac. The producer knows which one it
/// holds, and says so.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::file_system::write_operations) enum ItemHome {
    /// The local filesystem, at a path that's always there: the boot volume, so
    /// the launch sweep can act on it before the volume registry exists.
    Local,
    /// A local MOUNT that can go away (a USB drive, a kernel-mounted share).
    /// The path is relative to the mount root, so a remount at
    /// `/Volumes/Backups 1` still finds it, and the sweep uses `std::fs` once
    /// the volume is back in the registry.
    Mount { volume_id: String },
    /// A volume's OWN namespace, where this machine's filesystem calls mean
    /// nothing. The sweep goes through the `Volume` that wrote it, ❌ never
    /// `std::fs`.
    VolumeSpace { volume_id: String },
}

impl ItemHome {
    /// The volume this home waits on, or `None` for the boot disk.
    pub(in crate::file_system::write_operations) fn volume_id(&self) -> Option<&str> {
        match self {
            Self::Local => None,
            Self::Mount { volume_id } | Self::VolumeSpace { volume_id } => Some(volume_id),
        }
    }
}

/// The path space one producer's absolute paths get recorded in, and the root
/// they're stated relative to.
pub(in crate::file_system::write_operations) struct RecordHome {
    pub(in crate::file_system::write_operations) home: ItemHome,
    /// `Some` for a mount home: the root paths are stored relative to.
    root: Option<PathBuf>,
}

impl RecordHome {
    /// The local filesystem, for a path on a volume that can't leave.
    pub(in crate::file_system::write_operations) fn local() -> Self {
        Self {
            home: ItemHome::Local,
            root: None,
        }
    }

    /// A volume's own namespace, whose paths are already stated the way that
    /// volume wants them.
    pub(in crate::file_system::write_operations) fn volume_space(volume_id: &str) -> Self {
        Self {
            home: ItemHome::VolumeSpace {
                volume_id: volume_id.to_string(),
            },
            root: None,
        }
    }

    /// How the record should state `absolute`.
    pub(in crate::file_system::write_operations) fn record_path(&self, absolute: &Path) -> PathBuf {
        match &self.root {
            // A path that isn't under the root after all is stored whole rather
            // than dropped: a record that names the wrong place is still a
            // record a sweep will refuse, where no record is a leftover nobody
            // ever looks for again.
            Some(root) => absolute.strip_prefix(root).unwrap_or(absolute).to_path_buf(),
            None => absolute.to_path_buf(),
        }
    }
}

/// Where a LOCAL-filesystem path this operation writes belongs in the ledger.
///
/// A path under the transfer's destination volume, when that volume isn't the
/// boot disk, is recorded against that VOLUME: the module docs say why a
/// removable drive's leftovers can't be local-homed. Everything else is local.
///
/// ❗ Reads the operation's typed sides (`transfer_sides.rs`), ❌ never a
/// prefix match against the mount table: the transfer was HANDED both volumes,
/// and the prefix test here is only asking whether this particular path is on
/// the side we already know the id of.
pub(in crate::file_system::write_operations) fn home_for(state: &WriteOperationState, path: &Path) -> RecordHome {
    let Some(sides) = state.sides.as_ref() else {
        return RecordHome::local();
    };
    let destination = &sides.destination;
    if destination.root == Path::new("/") || !path.starts_with(&destination.root) {
        return RecordHome::local();
    }
    RecordHome {
        home: ItemHome::Mount {
            volume_id: destination.volume_id.clone(),
        },
        root: Some(destination.root.clone()),
    }
}

/// A recorded item, held by whoever created the thing it names so they can say
/// how it ended.
///
/// ❗ Not a `Drop` guard. What happens to an aside when its operation stops is a
/// data-safety decision with three different answers (`in_flight_temps::retire`,
/// `in_flight_temps::keep_for_arrival`, or leaving it recorded for the next
/// launch), and a guard would silently pick one of them on every early return.
#[derive(Clone, Debug)]
pub(in crate::file_system::write_operations) struct TrackedRecord {
    pub(in crate::file_system::write_operations) record: Record,
    /// Where the thing is on THIS machine right now, so the producer doesn't
    /// have to re-derive it from the record's path space.
    pub(in crate::file_system::write_operations) absolute: PathBuf,
}

impl TrackedRecord {
    /// Where the thing is on this machine.
    pub(in crate::file_system::write_operations) fn absolute(&self) -> &Path {
        &self.absolute
    }
}
