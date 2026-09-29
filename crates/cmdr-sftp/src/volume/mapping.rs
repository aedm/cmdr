//! Pure type mapping: SFTP metadata into `FileEntry`.
use cmdr_fs::entry::FileEntry;
use openssh_sftp_client::metadata::{MetaData, RawFileType};

/// Builds a [`FileEntry`] from one SFTP stat answer.
///
/// ❗ `app_path` is the APP spelling, prefix and all
/// (`sftp://ada@nas.local:22/srv/data/photos`), ❌ never the bare server path.
/// A pane holds what a listing hands it and passes it straight back, and the
/// app anchors it against the volume root on the way (`root_anchored`), so a
/// bare server path would come back doubled. `RemoteRoot::to_app_path` is where
/// it is made.
pub(super) fn metadata_to_file_entry(name: &str, app_path: &str, meta: &MetaData) -> FileEntry {
    let mut entry = entry_of_type(name, app_path, meta.file_type().map(|t| t.as_raw()));
    entry.size = if entry.is_directory { None } else { meta.len() };
    entry.modified_at = meta.modified().and_then(unix_secs);
    // SFTP v3 carries access and modify times and no creation time, so this stays
    // `None` rather than repeating the modify time and calling it a birth date.
    entry
}

/// The kind half of the mapping, split out because only the server can build a
/// `MetaData` that carries a file type.
///
/// ❗ `permissions` carries the `st_mode` FILE-TYPE bits and nothing else. The
/// type is what lets a walker skip a FIFO, socket, or device without opening
/// it (a read on a FIFO blocks the single-threaded `sftp-server` every
/// operation on this volume shares). The permission bits stay unset: copies
/// carry the low nine as a mode (`landed_mode`), and SFTP doesn't report one.
fn entry_of_type(name: &str, app_path: &str, file_type: Option<RawFileType>) -> FileEntry {
    let is_directory = file_type == Some(RawFileType::Directory);
    let is_symlink = file_type == Some(RawFileType::Symlink);
    let mut entry = FileEntry::new(name.to_string(), app_path.to_string(), is_directory, is_symlink);
    // `RawFileType` is `repr(u32)` over the `S_IF*` constants themselves.
    entry.permissions = file_type.map_or(0, |raw| raw as u32);
    entry
}

/// Seconds since the epoch, matching `FileEntry`'s own unit.
fn unix_secs(stamp: openssh_sftp_client::UnixTimeStamp) -> Option<u64> {
    stamp
        .as_system_time()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    const S_IFMT: u32 = 0o170_000;

    #[test]
    fn special_files_carry_their_file_type_bits() {
        // A planner that can't tell a FIFO from an empty file opens it, and a
        // read on a FIFO blocks the single-threaded `sftp-server` that every
        // operation on the volume shares.
        for (raw, bits) in [
            (RawFileType::FIFO, 0o010_000),
            (RawFileType::Socket, 0o140_000),
            (RawFileType::CharacterDevice, 0o020_000),
            (RawFileType::BlockDevice, 0o060_000),
        ] {
            let entry = entry_of_type("x", "sftp://h/x", Some(raw));
            assert_eq!(entry.permissions & S_IFMT, bits, "{raw:?}");
            assert!(!entry.is_directory && !entry.is_symlink);
        }
    }

    #[test]
    fn the_permission_bits_stay_unset() {
        // Copies read the low nine bits as a mode to carry (`landed_mode`), and
        // SFTP doesn't report one yet: only the type travels.
        for raw in [RawFileType::RegularFile, RawFileType::Directory, RawFileType::Symlink] {
            assert_eq!(entry_of_type("x", "sftp://h/x", Some(raw)).permissions & 0o7777, 0);
        }
        assert_eq!(entry_of_type("x", "sftp://h/x", None).permissions, 0);
    }
}
