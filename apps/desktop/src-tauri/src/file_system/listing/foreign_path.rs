//! Opening a directory by a path that may not be its volume's own spelling.
//!
//! A pane's path usually came out of a listing, so it's the volume's exact
//! spelling. One that didn't (typed, pasted, a restored tab, a favorite, an MCP
//! `nav_to_path`, or a pane carried over from the macOS kernel mount, which
//! decomposes every name) misses on a backend that matches names byte-for-byte
//! (SMB, ERR-VETBX). So a pane listing asks as given and, only on `NotFound`,
//! asks the volume where that path is stored (`Volume::find_stored_spelling`)
//! and lists THAT. The listing then carries the stored spelling: its cache key,
//! its watcher key, and every child path are the volume's own bytes, and the
//! pane adopts the spelling from `listing-complete`.
//!
//! ❌ Only for a PANE opening a directory. A delete walker, a copy scan, or a
//! refresh lists paths that came out of a listing, and there a miss means the
//! directory is gone: resolving it could hand the walker a look-alike twin.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::file_system::listing::OverlayRows;
use crate::file_system::listing::metadata::FileEntry;
use crate::file_system::volume::{ListingProgress, Volume, VolumeError};

/// A directory listing, and the path it lists as its volume spells it.
pub(crate) struct Listed {
    /// `path` as asked, or the volume's own spelling when that one missed.
    pub path: PathBuf,
    pub entries: Vec<FileEntry>,
}

impl Listed {
    /// The volume's spelling when it differs from what was asked, for the
    /// frontend to adopt; `None` for the common case, a path that listed as given.
    pub(crate) fn stored_spelling_of(&self, asked: &Path) -> Option<String> {
        (self.path != asked).then(|| self.path.to_string_lossy().into_owned())
    }
}

/// Lists `path` for a pane, falling back to the volume's stored spelling of it
/// when `path` as given isn't there.
///
/// Costs nothing extra on the happy path. A miss costs the resolve (a round trip
/// or two, plus a parent listing per wrong component, free when a pane already
/// shows that parent), then the listing itself. `cancel` reaches both.
pub(crate) async fn list_as_stored(
    volume: &dyn Volume,
    path: &Path,
    on_progress: Option<&(dyn Fn(ListingProgress) + Sync)>,
    cancel: Option<&CancellationToken>,
) -> Result<Listed, VolumeError> {
    let missed = match volume.list_directory_with_cancel(path, on_progress, cancel).await {
        Ok(entries) => {
            return Ok(Listed {
                path: path.to_path_buf(),
                entries,
            });
        }
        Err(e @ VolumeError::NotFound(_)) => e,
        Err(e) => return Err(e),
    };
    let Some(stored) = volume.find_stored_spelling(path, cancel).await? else {
        return Err(missed);
    };
    log::debug!("list_as_stored: {} is stored as {}", path.display(), stored.display());
    let entries = volume.list_directory_with_cancel(&stored, on_progress, cancel).await?;
    Ok(Listed { path: stored, entries })
}

/// `listing-respelled`: an open listing now shows its directory under another
/// spelling of the same path, which the pane adopts as its own.
///
/// Sent by [`respell_listings_on_volume`] after a backend swap; a listing that
/// lands on a stored spelling when it's first opened says so in
/// `listing-complete` instead.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
#[tauri_specta(event_name = "listing-respelled")]
pub struct ListingRespelledEvent {
    pub listing_id: String,
    /// The directory in its volume's own spelling.
    pub path: String,
}

/// Re-reads every pane listing on `volume_id` through the backend now serving
/// it, landing each on its stored spelling.
///
/// For a backend swap, the macOS kernel mount handing a share to a direct SMB
/// connection: the open listings were read by the OLD backend, so their paths
/// and every entry path carry the kernel's spelling (decomposed), which the new
/// byte-exact backend misses. Until they're re-read, opening, copying, or
/// deleting an accented entry in an open pane goes nowhere. A listing whose own
/// path changed spelling is re-keyed, so the watcher's reports reach it, and its
/// pane is told (`listing-respelled`); the entries arrive as an ordinary diff.
///
/// Fire-and-forget, one task per listing, each holding its directory's refresh
/// turn so it can't interleave with a watcher's re-read of the same directory.
pub(crate) fn respell_listings_on_volume(volume_id: &str) {
    for (listing_id, path, ..) in super::caching::find_listings_on_volume(volume_id) {
        tauri::async_runtime::spawn(respell_listing(volume_id.to_string(), listing_id, path));
    }
}

/// One listing's respell; `pub(super)` so a test can await it directly.
pub(super) async fn respell_listing(volume_id: String, listing_id: String, path: PathBuf) {
    let _turn = super::caching::refresh_turn(&volume_id, &path).await;
    let resolved = crate::file_system::volume::manager::get_volume_manager()
        .resolve(&volume_id, &path)
        .await;
    let is_routed = resolved.is_routed();
    let Some(volume) = resolved.volume else {
        return;
    };
    let listed = match list_as_stored(volume.as_ref(), &path, None, None).await {
        Ok(listed) => listed,
        Err(e) => {
            // The pane keeps what it has; its next navigation or refresh asks again.
            log::debug!("respell_listing: couldn't re-read {}: {e}", path.display());
            return;
        }
    };
    let Listed {
        path: stored,
        mut entries,
    } = listed;
    if !is_routed {
        crate::index_host::index().enrich(&volume_id, &mut entries);
    }
    let overlay_rows = crate::listing_overlays::decorate(&volume, &stored, &mut entries).await;
    if stored == path {
        super::caching::publish_replacement(&listing_id, entries, overlay_rows);
        return;
    }
    // The diff tells the pane what came and went, but it matches rows by name, so
    // it can't see every path changing spelling: the entries are written whole on
    // top, and the pane refetches its rows when it adopts the new spelling.
    super::caching::publish_replacement(&listing_id, entries.clone(), overlay_rows);
    super::operations::update_listing_entries(&listing_id, entries, OverlayRows::Recounted(overlay_rows));
    rekey_listing(&listing_id, &volume_id, &stored);
}

/// Moves `listing_id` to `stored`, the same directory in its volume's spelling,
/// and tells its pane. A no-op when the pane closed meanwhile.
fn rekey_listing(listing_id: &str, volume_id: &str, stored: &Path) {
    use crate::file_system::listing::cached_listing::{LISTING_CACHE, ListingPath};
    use crate::ignore_poison::RwLockIgnorePoison;
    use tauri_specta::Event;

    // Built before taking the cache: it reads the volume registry.
    let key = ListingPath::on_volume(volume_id, stored);
    {
        let mut cache = LISTING_CACHE.write_ignore_poison();
        let Some(listing) = cache.get_mut(listing_id) else {
            return;
        };
        listing.path = key;
    }
    log::info!("listing {listing_id} respelled to the volume's own spelling");
    let app = crate::file_system::watcher::WATCHER_MANAGER
        .read_ignore_poison()
        .app_handle
        .clone();
    if let Some(app) = app {
        let event = ListingRespelledEvent {
            listing_id: listing_id.to_string(),
            path: stored.to_string_lossy().into_owned(),
        };
        if let Err(e) = event.emit(&app) {
            log::warn!("respell_listing: couldn't emit listing-respelled: {e}");
        }
    }
}

#[cfg(test)]
#[path = "foreign_path_test.rs"]
mod foreign_path_test;
