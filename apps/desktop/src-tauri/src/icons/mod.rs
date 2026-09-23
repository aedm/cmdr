//! Icon retrieval and caching for file types.
//!
//! Parallelism: Uses rayon's global thread pool (auto-detects CPU cores).
//! Benchmarked on M1 Mac: 10 files→3.7ms, 50→8ms, 100→12.8ms, 200→21ms.
//! Custom thread counts showed no improvement, so we use auto-detect.

mod disk_cache;
#[cfg(target_os = "macos")]
mod macos_workspace;
mod memory_cache;
pub mod per_path;
// The special-folder classifier moved to `cmdr-fs`, where `FileEntry::new` can
// reach it. Aliased so `icons::special_folders::…` keeps resolving.
pub use cmdr_fs::icons::special_folders;
pub use memory_cache::{clear_directory_icon_cache, clear_extension_icon_cache};

use crate::config::ICON_SIZE;
use base64::Engine;
use image::{DynamicImage, ImageFormat, imageops::FilterType};
use memory_cache::{PATH_KEY_PREFIX, cache_icon, get_cached_icon};
#[cfg(target_os = "macos")]
use objc2::rc::autoreleasepool;
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

// macOS asks NSWorkspace (`macos_workspace.rs`); Linux asks the XDG icon theme
// (`linux_icons.rs`), because a GTK lookup wants the main thread and fails silently
// from a rayon or tokio worker.
#[cfg(target_os = "macos")]
use macos_workspace::render_file_icon;

/// Converts an image to a base64 WebP data URL.
fn image_to_data_url(img: &DynamicImage) -> Option<String> {
    // Resize to configured size
    let resized = img.resize_exact(ICON_SIZE, ICON_SIZE, FilterType::Lanczos3);

    // Encode as WebP
    let mut buffer = Cursor::new(Vec::new());
    resized.write_to(&mut buffer, ImageFormat::WebP).ok()?;

    // Convert to base64 data URL
    let base64 = base64::engine::general_purpose::STANDARD.encode(buffer.into_inner());
    Some(format!("data:image/webp;base64,{}", base64))
}

/// Encodes a raw RGBA buffer as a WebP data URL at the standard icon size, the
/// same form `get_icons` hands the frontend.
///
/// For icons that were read straight out of a bundle rather than fetched from the
/// OS icon provider (the "open terminal here" app list).
///
/// macOS only, because its one caller is: `file_system::terminal` reads `.app`
/// bundles, which don't exist elsewhere.
#[cfg(target_os = "macos")]
pub(crate) fn rgba_to_data_url(rgba: &[u8], width: u32, height: u32) -> Option<String> {
    let img = image::RgbaImage::from_raw(width, height, rgba.to_vec())?;
    image_to_data_url(&DynamicImage::ImageRgba8(img))
}

/// Encodes a raw RGBA buffer as PNG bytes, the form `NSImage initWithData:` reads with its
/// alpha intact. `None` when the buffer doesn't hold `width` × `height` pixels.
///
/// For the bitmaps the context menu shows beside an item (`menu/context_menu_icons.rs`):
/// app icons read out of a bundle, and the tag circles.
#[cfg(target_os = "macos")]
pub(crate) fn rgba_to_png(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let img = image::RgbaImage::from_raw(width, height, rgba.to_vec())?;
    let mut buffer = Cursor::new(Vec::new());
    img.write_to(&mut buffer, ImageFormat::Png).ok()?;
    Some(buffer.into_inner())
}

/// Fetches icon for a specific file path via the OS icon provider (macOS).
#[cfg(target_os = "macos")]
fn fetch_icon_for_path(path: &Path) -> Option<String> {
    let img = render_file_icon(path, ICON_SIZE as u16)?;
    image_to_data_url(&DynamicImage::ImageRgba8(img))
}

/// Fetches icon for a specific file path via XDG icon theme lookup.
#[cfg(target_os = "linux")]
fn fetch_icon_for_path(path: &Path) -> Option<String> {
    let icon_id = if path.is_dir() {
        "dir".to_string()
    } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        format!("ext:{}", ext.to_lowercase())
    } else {
        "file".to_string()
    };
    crate::linux_icons::get_icon_for_id(&icon_id, ICON_SIZE as u16).and_then(|img| image_to_data_url(&img))
}

/// Gets icon for a path as base64 data URL.
/// Public API for use by volumes module.
pub fn get_icon_for_path(path: &str) -> Option<String> {
    fetch_icon_for_path(Path::new(path))
}

/// Gets the sample file path to use for fetching an icon by ID.
/// For extension-based icons, we create an actual temp file since the OS may need it to exist.
fn get_sample_path_for_icon_id(icon_id: &str) -> Option<PathBuf> {
    if icon_id == "dir" || icon_id == "symlink-dir" {
        // A Cmdr-owned empty folder, ❌ never `~` — see `folder_sample_path`.
        // (Symlinks to dirs sample the same plain folder; the FE draws the link badge.)
        return folder_sample_path();
    }
    if icon_id == "symlink-file" || icon_id == "symlink" || icon_id == "file" {
        // Generic file icon - use /etc/hosts which exists on all macOS systems
        return Some(PathBuf::from("/etc/hosts"));
    }
    if let Some(ext) = icon_id.strip_prefix("ext:") {
        return icon_sample_path(ext);
    }
    None
}

/// The empty stand-in DIRECTORY standing in for every plain folder (`dir` /
/// `symlink-dir`, ~99% of rows).
///
/// ❌ Never sample the home directory here. macOS bakes a house badge into `~`'s
/// icon bitmap, so sampling it stamps that house onto every folder in every
/// listing; worse, a user-assigned custom icon on `~` leaks onto all of them the
/// same way. A folder Cmdr just created carries neither badge nor custom icon, so
/// it yields the true generic folder — still correctly accent- and
/// appearance-tinted, because macOS tints from system state, not from the path.
///
/// Shares `cmdr-icon-samples` with the per-extension file samples, for the same
/// reasons (namespaced, removable as a unit, reused across runs).
fn folder_sample_path() -> Option<PathBuf> {
    let path = std::env::temp_dir().join("cmdr-icon-samples").join("sample-folder");
    std::fs::create_dir_all(&path).ok()?;
    Some(path)
}

/// The empty stand-in file whose EXTENSION Launch Services reads to pick a
/// document icon. Only the suffix matters; the file stays empty on purpose, so
/// nothing content-sniffs it.
///
/// Samples live in a Cmdr-owned subdirectory rather than loose in the temp root.
/// A bare `<temp>/cmdr_icon_sample.pdf` is a fixed name any other process can
/// occupy, and it leaves one stray file per extension ever seen sitting in the
/// temp root forever. Under a directory they stay namespaced and removable as a
/// unit. Reuse across runs is deliberate: re-creating an empty file per launch
/// buys nothing.
fn icon_sample_path(ext: &str) -> Option<PathBuf> {
    let dir = std::env::temp_dir().join("cmdr-icon-samples");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("sample.{ext}"));
    if !path.exists() {
        std::fs::File::create(&path).ok()?;
    }
    Some(path)
}

/// Resolves a real-folder icon id (one fetched from an actual filesystem path) to
/// that path. Covers all three Tier-B/C kinds:
///
/// - `special:{name}` → the special folder's resolved standard location.
/// - `pkg:{path}` / `path:{path}` → the embedded path verbatim.
///
/// Returns `None` for the bounded sample-based ids (`dir`, `ext:*`, `file`,
/// `symlink*`), which fetch from sample paths, not real user folders.
fn real_path_for_real_folder_id(icon_id: &str) -> Option<String> {
    if icon_id.starts_with(special_folders::SPECIAL_KEY_PREFIX) {
        return special_folders::real_path_for_icon_id(icon_id).map(|p| p.to_string_lossy().into_owned());
    }
    if let Some(path) = icon_id.strip_prefix(per_path::PKG_KEY_PREFIX) {
        return Some(path.to_string());
    }
    if let Some(path) = icon_id.strip_prefix(PATH_KEY_PREFIX) {
        return Some(path.to_string());
    }
    None
}

/// Detects which of the given VISIBLE directory paths carry a Finder custom-icon
/// flag, and returns the `path:{dir}` icon id for each that does.
///
/// This is the deferred half of Tier-C custom-icon detection: the `getxattr`
/// check is too costly to run for every entry during a bulk listing, so the
/// frontend calls this only for the bounded set of directory rows actually on
/// screen. Each returned id can then be fed straight into `get_icons` to fetch
/// the real icon (FDA-gated, 8 MB thread).
///
/// Packages aren't included here — they're detected during listing by the pure
/// suffix check in `get_icon_id` and already arrive as `pkg:` ids.
pub fn custom_folder_icon_ids(directory_paths: Vec<String>) -> Vec<String> {
    directory_paths
        .into_iter()
        .filter(|path| per_path::has_custom_folder_icon(Path::new(path)))
        .map(|path| format!("{PATH_KEY_PREFIX}{path}"))
        .collect()
}

/// Fetches icons for the given icon IDs that are not already cached.
///
/// When `use_app_icons_for_documents` is true and on macOS, extension-based icons
/// are fetched from app bundles (showing the app's icon as fallback). When false,
/// the system's default document icons are used (Finder-style with app badge).
///
/// Returns a map of icon_id -> data URL.
pub fn get_icons(icon_ids: Vec<String>, use_app_icons_for_documents: bool) -> HashMap<String, String> {
    let mut result = HashMap::new();

    // Real-folder icon ids (`special:downloads`, `pkg:/Applications/Safari.app`,
    // `path:/Users/x/CustomFolder`) all fetch their icon from a REAL filesystem
    // path, which can be a cloud-synced location (Desktop/Documents iCloud sync)
    // whose NSWorkspace lookup descends into `fileproviderd`. Route every one of
    // them through the dedicated 8 MB-stack fetch (`fetch_path_icons`), NOT the
    // generic per-id loop below, which runs on the calling thread with a normal
    // stack. Each result is re-keyed back to its ORIGINAL icon id (the bounded
    // `special:{name}` or the per-path `pkg:`/`path:` key), not the raw path.
    //
    // Before the cold NSWorkspace fetch, consult the on-disk persistent cache
    // (keyed by path + folder mtime): a folder whose icon we fetched in a prior
    // session reloads instantly and skips NSWorkspace entirely until the user
    // re-icons it (which bumps the folder mtime → cache miss → re-fetch).
    let mut remaining = Vec::with_capacity(icon_ids.len());
    // (original_icon_id, real_path) pairs to fetch via the 8 MB threads.
    let mut per_path_to_fetch: Vec<(String, String)> = Vec::new();
    for icon_id in icon_ids {
        if let Some(cached) = get_cached_icon(&icon_id) {
            result.insert(icon_id, cached);
            continue;
        }
        if let Some(real_path) = real_path_for_real_folder_id(&icon_id) {
            if let Some(url) = disk_cache::load(&icon_id, &real_path) {
                // Warm-tier hit: promote into the hot in-memory cache and return.
                cache_icon(icon_id.clone(), url.clone());
                result.insert(icon_id, url);
            } else {
                per_path_to_fetch.push((icon_id, real_path));
            }
            continue;
        }
        if icon_id.starts_with(special_folders::SPECIAL_KEY_PREFIX) {
            // A `special:` id whose standard location didn't resolve on this
            // platform: skip; the frontend keeps the `dir` fallback.
            continue;
        }
        remaining.push(icon_id);
    }

    if !per_path_to_fetch.is_empty() {
        let paths: Vec<String> = per_path_to_fetch.iter().map(|(_, path)| path.clone()).collect();
        let fetched = fetch_path_icons(paths);
        // `fetch_path_icons` returns `(path:{real_path}, data_url)` in input
        // order; re-key each back to its original id, cache it (memory + on-disk),
        // and return it.
        for ((original_id, real_path), (_, data_url)) in per_path_to_fetch.into_iter().zip(fetched) {
            if let Some(url) = data_url {
                cache_icon(original_id.clone(), url.clone());
                disk_cache::store(&original_id, &real_path, &url);
                result.insert(original_id, url);
            }
        }
    }

    for icon_id in remaining {
        // Cache was already checked above for this batch.

        // macOS: drain autoreleased ObjC objects per iteration
        // (fetch_fresh_extension_icon and fetch_icon_for_path call ObjC APIs)
        #[cfg(target_os = "macos")]
        let fetched = autoreleasepool(|_| {
            if use_app_icons_for_documents
                && let Some(ext) = icon_id.strip_prefix("ext:")
                && let Some(data_url) = fetch_fresh_extension_icon(ext, true)
            {
                return Some(data_url);
            }

            if let Some(sample_path) = get_sample_path_for_icon_id(&icon_id)
                && let Some(data_url) = fetch_icon_for_path(&sample_path)
            {
                return Some(data_url);
            }
            None
        });

        #[cfg(not(target_os = "macos"))]
        let fetched = {
            // Silence unused variable warning when not on macOS
            let _ = use_app_icons_for_documents;

            // Linux: look up directly from XDG icon theme (no temp files needed)
            #[cfg(target_os = "linux")]
            if let Some(img) = crate::linux_icons::get_icon_for_id(&icon_id, ICON_SIZE as u16)
                && let Some(data_url) = image_to_data_url(&img)
            {
                Some(data_url)
            } else if let Some(sample_path) = get_sample_path_for_icon_id(&icon_id)
                && let Some(data_url) = fetch_icon_for_path(&sample_path)
            {
                Some(data_url)
            } else {
                None
            }

            #[cfg(not(target_os = "linux"))]
            if let Some(sample_path) = get_sample_path_for_icon_id(&icon_id)
                && let Some(data_url) = fetch_icon_for_path(&sample_path)
            {
                Some(data_url)
            } else {
                None
            }
        };

        if let Some(data_url) = fetched {
            cache_icon(icon_id.clone(), data_url.clone());
            result.insert(icon_id, data_url);
        }
    }

    result
}

/// Fetches a fresh icon for an extension, bypassing any OS cache.
/// On macOS, this goes directly to the app bundle. On other platforms, falls back to temp files.
///
/// When `use_app_icons_for_documents` is true, falls back to app icons for files without
/// document-specific icons. When false, uses Finder-style document icons.
fn fetch_fresh_extension_icon(ext: &str, use_app_icons_for_documents: bool) -> Option<String> {
    // On macOS, try to get the icon directly from the default app's bundle
    // This bypasses the Launch Services icon cache
    #[cfg(target_os = "macos")]
    {
        if let Some(img) = crate::macos_icons::fetch_fresh_icon_for_extension(ext, use_app_icons_for_documents) {
            return image_to_data_url(&img);
        }
    }

    // Silence unused variable warning on non-macOS platforms
    #[cfg(not(target_os = "macos"))]
    let _ = use_app_icons_for_documents;

    // Fallback: use temp file approach (works on all platforms, but may use cached icons)
    icon_sample_path(ext).and_then(|sample_path| fetch_icon_for_path(&sample_path))
}

/// Refreshes icons for a directory listing.
/// Fetches icons in parallel for:
/// 1. All unique extensions (checking for file association changes)
/// 2. All directory paths (for custom folder icons)
///
/// On macOS, extension icons are fetched directly from app bundles to bypass
/// the Launch Services icon cache, ensuring we always show the current association.
///
/// When `use_app_icons_for_documents` is true, falls back to app icons for files without
/// document-specific icons. When false, uses Finder-style document icons.
///
/// Returns only the icons that were successfully fetched, regardless of cache state.
/// This allows the frontend to detect changes by comparing with its cached icons.
pub fn refresh_icons_for_directory(
    directory_paths: Vec<String>,
    extensions: Vec<String>,
    use_app_icons_for_documents: bool,
) -> HashMap<String, String> {
    let mut result = HashMap::new();

    // Fetch extension icons in parallel (uses rayon's global pool)
    if !extensions.is_empty() {
        let ext_results: Vec<(String, Option<String>)> = extensions
            .par_iter()
            .map(|ext| {
                // macOS: drain autoreleased ObjC objects per rayon thread iteration
                // (UTType/Launch Services/NSWorkspace calls accumulate otherwise)
                #[cfg(target_os = "macos")]
                {
                    autoreleasepool(|_| {
                        let icon_id = format!("ext:{}", ext.to_lowercase());
                        let data_url = fetch_fresh_extension_icon(ext, use_app_icons_for_documents);
                        (icon_id, data_url)
                    })
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let icon_id = format!("ext:{}", ext.to_lowercase());
                    let data_url = fetch_fresh_extension_icon(ext, use_app_icons_for_documents);
                    (icon_id, data_url)
                }
            })
            .collect();

        for (icon_id, data_url) in ext_results {
            if let Some(url) = data_url {
                cache_icon(icon_id.clone(), url.clone());
                result.insert(icon_id, url);
            }
        }
    }

    // Fetch directory icons by exact REAL path. These descend into NSWorkspace on
    // real user folders, which for iCloud/Dropbox folders make synchronous XPC
    // round-trips into `fileproviderd` with deep override chains — enough to
    // overflow rayon's default 2 MB worker stack. So this branch runs on dedicated
    // 8 MB-stack OS threads (same pattern as `file_system/sync_status.rs` and
    // `open_with.rs`), NOT rayon. The extension branch above stays on rayon because
    // it fetches from sample temp paths that never descend into a cloud provider.
    if !directory_paths.is_empty() {
        let dir_results = fetch_path_icons(directory_paths);
        for (icon_id, data_url) in dir_results {
            if let Some(url) = data_url {
                // Update cache
                cache_icon(icon_id.clone(), url.clone());
                result.insert(icon_id, url);
            }
        }
    }

    result
}

/// 8 MB stack per thread: enough for deep FileProvider XPC chains that
/// NSWorkspace's per-path icon lookup descends into on cloud folders.
#[cfg(target_os = "macos")]
const ICON_THREAD_STACK_SIZE: usize = 8 * 1024 * 1024;

/// Fetches per-path folder icons, keyed `path:{path}`, on dedicated 8 MB-stack OS
/// threads (macOS) to survive `fileproviderd` XPC depth on cloud folders. The
/// `data_url` is `None` when the OS returned no icon.
#[cfg(target_os = "macos")]
fn fetch_path_icons(paths: Vec<String>) -> Vec<(String, Option<String>)> {
    let num_threads = paths
        .len()
        .min(std::thread::available_parallelism().map_or(4, |n| n.get()));

    std::thread::scope(|scope| {
        let chunk_size = paths.len().div_ceil(num_threads);
        let handles: Vec<_> = paths
            .chunks(chunk_size)
            .map(|chunk| {
                let chunk = chunk.to_vec();
                std::thread::Builder::new()
                    .stack_size(ICON_THREAD_STACK_SIZE)
                    .name("icon_path_fetch".into())
                    .spawn_scoped(scope, move || {
                        chunk
                            .into_iter()
                            .map(|path| {
                                // Drain autoreleased ObjC objects per path (NSWorkspace
                                // calls accumulate otherwise) on these threads, which
                                // lack AppKit's autorelease pool.
                                autoreleasepool(|_| {
                                    let data_url = fetch_icon_for_path(&PathBuf::from(&path));
                                    (format!("path:{}", path), data_url)
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .expect("failed to spawn icon path-fetch thread")
            })
            .collect();

        let mut results = Vec::with_capacity(paths.len());
        for handle in handles {
            results.extend(handle.join().expect("icon path-fetch thread panicked"));
        }
        results
    })
}

/// Non-macOS path-icon fetch. Linux resolves icons via the XDG theme lookup, which
/// makes no XPC calls and can't descend into a cloud provider, so rayon's pool is
/// fine here.
#[cfg(not(target_os = "macos"))]
fn fetch_path_icons(paths: Vec<String>) -> Vec<(String, Option<String>)> {
    paths
        .par_iter()
        .map(|path| {
            let data_url = fetch_icon_for_path(&PathBuf::from(path));
            (format!("path:{}", path), data_url)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the sample out of the shared temp ROOT, where a fixed
    /// `cmdr_icon_sample.<ext>` is a name any other process can occupy.
    #[test]
    fn extension_samples_live_in_a_cmdr_owned_directory() {
        let sample = get_sample_path_for_icon_id("ext:cmdrtest").expect("a sample path for an extension id");

        assert_eq!(
            sample.parent().and_then(|p| p.file_name()),
            Some(std::ffi::OsStr::new("cmdr-icon-samples")),
            "samples belong in their own directory, not loose in the temp root"
        );
        assert_eq!(sample.extension(), Some(std::ffi::OsStr::new("cmdrtest")));
        assert!(sample.is_file(), "Launch Services needs the sample to exist");
        assert_eq!(
            std::fs::metadata(&sample).expect("sample metadata").len(),
            0,
            "the sample stays empty so nothing content-sniffs it"
        );
    }

    /// Pins the generic folder sample OFF the home directory. macOS bakes a house
    /// badge into `~`'s icon, so sampling it stamped that house onto every folder
    /// row in every listing, and leaked a custom icon assigned to `~` the same way.
    #[test]
    fn the_generic_folder_sample_is_a_cmdr_owned_directory_not_the_home_dir() {
        for icon_id in ["dir", "symlink-dir"] {
            let sample = get_sample_path_for_icon_id(icon_id).expect("a sample path for the generic folder id");

            assert_ne!(
                Some(sample.as_path()),
                dirs::home_dir().as_deref(),
                "{icon_id} must not sample ~: its badge and any custom icon would leak onto every folder"
            );
            assert_eq!(
                sample.parent().and_then(|p| p.file_name()),
                Some(std::ffi::OsStr::new("cmdr-icon-samples")),
                "the folder sample shares the samples directory with the extension samples"
            );
            assert!(
                sample.is_dir(),
                "the OS needs a real directory to hand back a folder icon"
            );
        }
    }

    #[test]
    fn real_path_resolves_per_path_and_pkg_keys() {
        assert_eq!(
            real_path_for_real_folder_id("pkg:/Applications/Safari.app").as_deref(),
            Some("/Applications/Safari.app")
        );
        assert_eq!(
            real_path_for_real_folder_id("path:/Users/x/Custom Folder").as_deref(),
            Some("/Users/x/Custom Folder")
        );
        // Sample-based ids have no real folder path.
        assert_eq!(real_path_for_real_folder_id("dir"), None);
        assert_eq!(real_path_for_real_folder_id("ext:txt"), None);
        assert_eq!(real_path_for_real_folder_id("file"), None);
    }

    // `special:*` resolves to a real standard location only on macOS; in a
    // headless Linux CI container `dirs::download_dir()` returns `None`, so the
    // key never resolves there. This is macOS-only; Linux falls back to the XDG
    // theme path and never produces `special:*` ids.
    #[cfg(target_os = "macos")]
    #[test]
    fn real_path_resolves_special_keys() {
        let downloads = dirs::download_dir().expect("download_dir resolves");
        assert_eq!(
            real_path_for_real_folder_id("special:downloads").as_deref(),
            Some(downloads.to_string_lossy().as_ref())
        );
    }
}
