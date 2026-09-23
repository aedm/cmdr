//! Paths that enter from outside Cmdr, in their volume's own spelling.
//!
//! A Finder drag-in or a paste of files copied in Finder hands the app kernel-mount
//! paths (`/Volumes/<share>/…`), which the macOS kernel spells decomposed. When the
//! share is on a direct connection those paths route to the byte-exact SMB volume,
//! where an accented name the server stores composed would miss. The frontend asks
//! here ONCE, right where the paths enter, so the scan preview, the conflict check,
//! the transfer, and the journal it writes all carry the stored spelling.
//! `file_system/listing/DETAILS.md` § "A pane path the volume stores another way".

use std::path::PathBuf;

use tokio::time::Duration;

use crate::deadline::TimedOut;

/// A resolve is a stat plus, on a miss, a parent listing per wrong component. A
/// drop never waits longer than this for it: past the deadline the paths go as
/// given, which on a byte-exact share is at worst a "couldn't find".
const STORED_SPELLINGS_TIMEOUT: Duration = Duration::from_secs(10);

/// `paths` (index-aligned) in the spelling `volume_id` stores them under. A path
/// with no other stored spelling, an ambiguous one, or a volume that isn't
/// registered comes back as given.
#[tauri::command]
#[specta::specta]
pub async fn stored_spellings(volume_id: String, paths: Vec<String>) -> TimedOut<Vec<String>> {
    let as_given = paths.clone();
    let Some(volume) = crate::file_system::volume::manager::get_volume_manager().get(&volume_id) else {
        return TimedOut {
            data: as_given,
            timed_out: false,
        };
    };
    let paths = paths.into_iter().map(PathBuf::from).collect();
    let resolve = crate::file_system::listing::foreign_path::stored_spellings(volume.as_ref(), paths);
    match tokio::time::timeout(STORED_SPELLINGS_TIMEOUT, resolve).await {
        Ok(stored) => TimedOut {
            data: stored.into_iter().map(|p| p.to_string_lossy().into_owned()).collect(),
            timed_out: false,
        },
        Err(_) => TimedOut {
            data: as_given,
            timed_out: true,
        },
    }
}
