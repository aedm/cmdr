//! Path → handle: the cache lookup every device op starts with, the heal that
//! lists a parent when the cache doesn't know a path, and the forced re-list
//! that refreshes a handle the device re-keyed.

use log::debug;
use mtp_rs::ObjectHandle;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use super::errors::MtpConnectionError;
use super::{MtpConnectionManager, normalize_mtp_path};

impl MtpConnectionManager {
    /// Re-resolves the object handles along `dir`'s path so a subsequent
    /// [`resolve_path_to_handle`](Self::resolve_path_to_handle) returns a FRESH
    /// handle for `dir`, not a stale cached one. Used by the upload path when
    /// the device rejects a cached parent handle (it re-keyed its handles since
    /// the folder was last listed).
    ///
    /// Walks `dir`'s ancestors root-first (excluding `dir` itself), forcing a
    /// real USB re-list of each (the listing cache is invalidated first so the
    /// 5 s TTL can't serve a stale listing). Listing an ancestor repopulates the
    /// path cache for its children, so by the time we list `dir`'s parent, the
    /// fresh handle for `dir` is cached. Root (`ObjectHandle::ROOT`) is a
    /// constant, so the common case (a top-level folder like `/Documents`) heals
    /// with a single re-list of `/`. Best-effort: a failed re-list is logged and
    /// the walk stops; the caller's retry then fails cleanly with a
    /// destination-correct error rather than looping.
    pub(super) async fn refresh_dir_handle(&self, device_id: &str, storage_id: u32, dir: &Path) {
        let dir = normalize_mtp_path(dir.to_string_lossy().as_ref());
        // Ancestors root-first, excluding `dir` itself: ["/", "/a", ...] up to
        // and including `parent(dir)`. Listing `parent(dir)` refreshes `dir`'s
        // own handle.
        let mut ancestors: Vec<PathBuf> = dir.ancestors().skip(1).map(Path::to_path_buf).collect();
        ancestors.reverse();
        for ancestor in ancestors {
            self.invalidate_listing_cache(device_id, storage_id, &ancestor).await;
            if let Err(e) = self
                .list_directory(device_id, storage_id, &ancestor.to_string_lossy())
                .await
            {
                debug!(
                    "refresh_dir_handle: re-list of {} failed while healing handle for {}: {:?}",
                    ancestor.display(),
                    dir.display(),
                    e
                );
                return;
            }
        }
    }

    /// Resolves a virtual path to an MTP object handle, listing its parent when
    /// the path cache doesn't know it.
    ///
    /// The cache only holds what some listing put there, and a path reaches an op
    /// by routes that never listed its parent: a pane restored after a reconnect,
    /// a search result, a go-to-path, a pane still showing a file another op
    /// removed. So a miss lists the parent (which resolves ITS parent the same
    /// way, up to the constant root) and looks again. A path still missing after
    /// that is gone from the device: [`MtpConnectionError::ObjectNotFound`]
    /// naming the path asked about, also when a folder above it is what's
    /// missing, because the user acted on the path, not on its folder.
    ///
    /// The parent listing goes through the 5 s listing cache on purpose. Both
    /// caches are written together (`finalize_listing`) and every mutation that
    /// drops a path also invalidates its parent's listing, so a fresh listing
    /// without the name is a real answer, and a copy of many already-gone files
    /// pays one listing rather than one per file.
    ///
    /// ❌ Never call this holding the device lock: the heal re-lists through it,
    /// and the tokio `Mutex` isn't reentrant.
    ///
    /// A named boxed future rather than an `async fn`: listing a folder resolves
    /// that folder through here, and without the `dyn` the recursion makes an
    /// infinitely sized future whose `Send` the compiler can't prove.
    pub(super) fn resolve_path_to_handle<'a>(
        &'a self,
        device_id: &'a str,
        storage_id: u32,
        path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<ObjectHandle, MtpConnectionError>> + Send + 'a>> {
        Box::pin(async move {
            let path = normalize_mtp_path(path);
            if path.as_os_str() == "/" {
                return Ok(ObjectHandle::ROOT);
            }

            if let Some(handle) = self.cached_handle(device_id, storage_id, &path).await? {
                return Ok(handle);
            }

            let parent = path.parent().unwrap_or(Path::new("/")).to_string_lossy().to_string();
            debug!(
                "MTP resolve: {} isn't cached on {device_id}:{storage_id}, listing {parent} to find it",
                path.display()
            );
            let not_found = || MtpConnectionError::ObjectNotFound {
                device_id: device_id.to_string(),
                path: path.to_string_lossy().to_string(),
            };
            match self.list_directory(device_id, storage_id, &parent).await {
                Ok(_) => {}
                Err(MtpConnectionError::ObjectNotFound { .. }) => return Err(not_found()),
                Err(e) => return Err(e),
            }

            self.cached_handle(device_id, storage_id, &path)
                .await?
                .ok_or_else(not_found)
        })
    }

    /// The handle the path cache holds for an already-normalized `path`, if any.
    /// `NotConnected` when the device isn't in the registry.
    async fn cached_handle(
        &self,
        device_id: &str,
        storage_id: u32,
        path: &Path,
    ) -> Result<Option<ObjectHandle>, MtpConnectionError> {
        use cmdr_fs::ignore_poison::RwLockIgnorePoison;

        let devices = self.devices.lock().await;
        let entry = devices.get(device_id).ok_or_else(|| MtpConnectionError::NotConnected {
            device_id: device_id.to_string(),
        })?;
        let cache_map = entry.path_cache.read_ignore_poison();
        Ok(cache_map
            .get(&storage_id)
            .and_then(|storage_cache| storage_cache.path_to_handle.get(path))
            .copied())
    }
}
