//! Context menus (file, breadcrumb, tab, network host, function key bar) and the
//! viewer-window menu. The main menu bar is `menu_bar.rs`.

use std::collections::HashMap;
#[cfg(target_os = "macos")]
use std::path::PathBuf;

use tauri::{
    AppHandle, Runtime, Wry,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
};

#[cfg(target_os = "macos")]
use crate::file_system::file_provider_actions::ProviderOffer;
#[cfg(target_os = "macos")]
use crate::file_system::google_drive::DriveItemLinks;
#[cfg(target_os = "macos")]
use crate::file_system::open_with::OpenWithChoices;
#[cfg(target_os = "macos")]
use crate::file_system::share::ShareService;
#[cfg(target_os = "macos")]
use crate::file_system::sync_status::SyncStatus;

use crate::intl::{menu_t, menu_t_with};

use super::context_menu_header::{ContextMenuTargetFacts, append_context_menu_header};

#[cfg(target_os = "macos")]
use super::OPEN_TERMINAL_HERE_ID;
use super::menu_bar::SHOW_IN_FILE_MANAGER_KEY;
#[cfg(target_os = "macos")]
use super::menu_items::APP_MENU_TITLE;
use super::menu_items::{
    COPY_FILENAME_MAX_CHARS, DetachWord, SameKindTarget, detach_label, pin_tab_label, truncate_for_menu_label,
};
use super::selection_submenu::build_selection_submenu;
#[cfg(target_os = "macos")]
use super::{
    CLOUD_MAKE_OFFLINE_ID, CLOUD_REMOVE_DOWNLOAD_ID, DRIVE_ASK_GEMINI_ID, DRIVE_COPY_LINK_ID, DRIVE_OPEN_ID,
    GET_INFO_ID, HELP_MENU_ID, QUICK_LOOK_ID,
};
use super::{
    COPY_CURRENT_DIR_PATH_ID, COPY_FILENAME_ID, COPY_PATH_ID, EDIT_ID, EDIT_MENU_ID, EJECT_VOLUME_ID,
    FAVORITE_REMOVE_ID, FAVORITE_RENAME_ID, FAVORITES_ADD_CONTEXT_ID, FILE_COPY_ID, FILE_DELETE_ID, FILE_DUPLICATE_ID,
    FILE_MOVE_ID, FILE_NEW_FILE_ID, FILE_NEW_FOLDER_ID, FILE_VIEW_ID, FUNCTION_KEY_BAR_HIDE_ID, ImageIndexMenuState,
    NETWORK_HOST_DISCONNECT_ID, NETWORK_HOST_FORGET_SECRET_ID, NETWORK_HOST_FORGET_SERVER_ID, OPEN_ID, RENAME_ID,
    SERVER_DISCONNECT_ID, SERVER_EDIT_ID, SERVER_FORGET_ID, SERVER_FORGET_SECRET_ID, SERVER_OPEN_ID, SERVER_PIN_ID,
    SERVER_UNPIN_ID, SHOW_IN_FINDER_ID, TAB_CLOSE_ID, TAB_CLOSE_OTHERS_ID, TAB_PIN_ID, VIEWER_WORD_WRAP_ID,
    ViewerMenuItems, image_index_menu_items,
};
use super::{frontend_shortcut_to_menu_text, menu_id_to_command};

/// Per-file information needed to build a fully-populated context menu.
///
/// On non-macOS this is empty; on macOS it carries the cloud sync status (used to
/// decide between "Make available offline" and "Remove download"), whether the file
/// lives in any File Provider domain (gates cloud actions), and the precomputed
/// "Open with" candidate apps.
#[cfg(target_os = "macos")]
#[derive(Default)]
pub struct FileContextInfo {
    pub sync_status: SyncStatus,
    /// Whether this path is in iCloud Drive specifically. Gates the cloud action menu
    /// items. Eviction / download work via `FileManager` ubiquity APIs, which only
    /// support iCloud (not third-party File Providers). See `cloud_actions.rs` for why.
    pub is_icloud_drive: bool,
    /// The Google Drive web URLs for this item, when it resolves to one. `Some`
    /// gates the Drive menu group, which is self-validating: no ID, no item; its
    /// `gemini_url` gates `Ask Gemini` alone, since folders have none. See
    /// `file_system/google_drive/` for why this isn't a path-prefix check (Drive's
    /// mirror mode puts real files outside `~/Library/CloudStorage`).
    pub google_drive_links: Option<DriveItemLinks>,
    /// The right-clicked rows' File Provider actions, when their provider offers any. Drawn
    /// as one flat group below the cloud items, and kept in `MenuContext` so a click can run
    /// one. `file_system/file_provider_actions/`.
    pub file_provider_offer: Option<ProviderOffer>,
    pub open_with: OpenWithChoices,
    /// The services macOS offers for this selection, in its own order, one `Share`
    /// submenu item each. EMPTY means macOS offers none and the whole item is left
    /// out: an empty share sheet holding only `Edit Extensions…` is the symptom the
    /// submenu replaced. Filled by `file_system::share::services_for`.
    pub share_services: Vec<ShareService>,
    /// Which of the seven Finder color tags (index 1..=7) the selection already carries.
    /// "Applied" = EVERY selected path has a tag of that color, so the menu shows a
    /// checked (checkmark-composited) circle and the click toggles it off. Index 0 is
    /// unused (colorless). Computed by reading each path's tags once at menu-build time.
    pub applied_tag_colors: [bool; 8],
}

#[cfg(not(target_os = "macos"))]
#[derive(Default)]
pub struct FileContextInfo;

/// Result of building a file context menu: the menu itself, plus (on macOS) a
/// `bundle_id → app_path` map that the caller stores in `MenuState.context.open_with_apps`
/// so `lib.rs::on_menu_event` can resolve `open-with:<bundle-id>` clicks back to an app URL.
pub struct ContextMenuResult<R: Runtime> {
    pub menu: Menu<R>,
    #[cfg(target_os = "macos")]
    pub open_with_apps: HashMap<String, PathBuf>,
}

/// What the PANE the right-click landed in contributes, as opposed to the file
/// under the cursor.
///
/// One struct rather than three trailing `bool`s, for the reason the frontend's
/// `PaneContextMenuFacts` gives: same-typed positional flags are exactly what binds
/// to the wrong slot when one is inserted. Every field's most restrictive answer is
/// its `Default`, so a surface that can't answer says nothing.
#[derive(Debug, Default, Clone, Copy)]
pub struct ContextMenuPaneFacts {
    /// Suppresses Rename, Duplicate, and the two create items, which only make
    /// sense on a real directory. `true` from the search-results virtual pane
    /// (`volumeId == "search-results"`, see `apps/desktop/src/lib/search/capabilities.ts`).
    /// Source-side actions (Open, Copy, Move, Delete, Show in Finder, Copy filename,
    /// Copy path) stay, because the underlying paths are real.
    pub restrict_destination_actions: bool,
    /// Whether "Open terminal here" is clickable. It acts on the pane's FOLDER, not
    /// this file, so a pane on MTP or ADB shows it greyed out; the snapshot pane and
    /// the Search dialog pass `false` too, having no folder of their own to open.
    pub can_open_terminal_here: bool,
    /// Whether `Share` and `Services` may appear at all. The pane's answer too, but to
    /// a different question: whether its ROWS are real OS paths, which is what both a
    /// share service and a macOS service need (each takes file URLs). The search-results
    /// snapshot says yes (its rows are real files) where `can_open_terminal_here` says
    /// no, so the two can't be folded into one flag. `Share` needs one more yes on top:
    /// macOS has to actually offer a service (`FileContextInfo::share_services`).
    pub can_share: bool,
    /// Whether "Add to favorites" may appear on a folder row at all: whether that row is a
    /// place a favorite could point back to next launch. The ROW's answer, like `can_share`,
    /// and the affordance half of Rust's own `add_favorite` gate — which refuses an archive's
    /// insides, a `.git`-portal folder, a phone, and a protocol-only server. Without it the
    /// item is offered where the add would be refused, and the user gets silence.
    pub can_favorite: bool,
}

/// The user's keyboard shortcuts as they stand right now: command-registry id → the
/// combo in the frontend's canonical spelling (`⌘⇧C`, `F5`, `Space`). The frontend
/// pushes it with every popup, straight out of the shortcut registry, so a command
/// nobody has bound is simply absent.
///
/// This is how a popup menu's accelerator labels stay true after a rebind. They were
/// literals once, and a literal starts lying the moment the user changes a key in
/// Settings > Shortcuts.
///
/// ❗ These accelerators never FIRE. A popup's key equivalents are never registered
/// with the app (only the menu bar's are), so every one of them is display text. That
/// is why the lookup goes through `frontend_shortcut_to_menu_text` and ❌ never
/// `frontend_shortcut_to_accelerator`: the menu bar's modifier floor would drop a bare
/// `Space` or `F5`, which is right there and wrong here.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(transparent)]
pub struct ContextMenuShortcuts(HashMap<String, String>);

impl ContextMenuShortcuts {
    /// The accelerator to show beside the item with this MENU id, in Tauri's format.
    ///
    /// Menu id in, not command id: `menu_id_to_command` is already the one table
    /// binding the two vocabularies, and taking the same id the item is built with
    /// means the label can't end up describing a different command.
    ///
    /// `None` when nothing is bound — and also for a menu id that maps to no command
    /// at all, which is a caller bug rather than a user state, and shows up as a
    /// missing label rather than a wrong one.
    pub fn for_menu_item(&self, menu_id: &str) -> Option<String> {
        let (command_id, _scope) = menu_id_to_command(menu_id)?;
        frontend_shortcut_to_menu_text(self.0.get(command_id)?)
    }
}

/// One context-menu item, labelled with whatever its command is bound to right now.
///
/// The menu id is used twice on purpose: as the item's own id, and as the key the
/// accelerator is looked up by. One argument, so the label can't drift from the item.
pub(super) fn context_item<R: Runtime>(
    app: &AppHandle<R>,
    shortcuts: &ContextMenuShortcuts,
    menu_id: &str,
    label: String,
    enabled: bool,
) -> tauri::Result<MenuItem<R>> {
    MenuItem::with_id(
        app,
        menu_id,
        label,
        enabled,
        shortcuts.for_menu_item(menu_id).as_deref(),
    )
}

/// Builds a context menu for a specific file.
///
/// `pane` is what the surface the click landed in contributes; see
/// [`ContextMenuPaneFacts`] for each answer and who gives it. `shortcuts` is the live
/// shortcut registry every accelerator label in here reads; see
/// [`ContextMenuShortcuts`].
#[allow(
    clippy::too_many_arguments,
    reason = "each one is a separate question about the right-click; folding the four that describe the ROW (`filename`, `is_directory`, and the two the caller's `path` / `paths` become) into `ContextMenuTargetFacts` is the consolidation this wants, and it reaches across the IPC boundary"
)]
pub fn build_context_menu<R: Runtime>(
    app: &AppHandle<R>,
    filename: &str,
    is_directory: bool,
    #[cfg_attr(
        not(target_os = "macos"),
        allow(unused_variables, reason = "all reads of `info` sit inside macOS-gated branches")
    )]
    info: &FileContextInfo,
    pane: ContextMenuPaneFacts,
    // Media-index image-search facts about the right-clicked folder; `image_index_menu_items`
    // turns them into the folder-only chosen/exclusion items (empty when the master toggle
    // is off).
    image_index: ImageIndexMenuState,
    // What the right-clicked ROW(S) are, for the header line at the very top; see
    // `ContextMenuTargetFacts`.
    target: ContextMenuTargetFacts<'_>,
    // Every accelerator label in this menu, as the registry has them right now.
    shortcuts: &ContextMenuShortcuts,
    // What "Select all of the same kind" would select from the right-clicked row, computed by
    // the frontend as this menu opens. ❗ Live, ❌ never the menu bar's debounced value.
    same_kind: Option<&SameKindTarget>,
) -> tauri::Result<ContextMenuResult<R>> {
    let ContextMenuPaneFacts {
        restrict_destination_actions,
        can_open_terminal_here,
        can_share,
        can_favorite,
    } = pane;
    // Both gate macOS-only items, so on Linux they're read nowhere.
    #[cfg(not(target_os = "macos"))]
    let _ = (can_open_terminal_here, can_share);
    let menu = Menu::new(app)?;

    // What this menu will act on, first line, above everything. Cmdr acts on the whole
    // selection or on the one right-clicked row depending on whether the click landed
    // inside the selection, and this is the only place that says which.
    append_context_menu_header(app, &menu, filename, target)?;

    // Open / View / Edit group (files only)
    #[cfg(target_os = "macos")]
    let mut open_with_apps: HashMap<String, PathBuf> = HashMap::new();
    if !is_directory {
        let open_item = MenuItem::with_id(app, OPEN_ID, menu_t("menu.file.open"), true, None::<&str>)?;
        let view_item = context_item(app, shortcuts, FILE_VIEW_ID, menu_t("menu.file.view"), true)?;
        let edit_item = context_item(app, shortcuts, EDIT_ID, menu_t("menu.context.edit"), true)?;
        menu.append(&open_item)?;
        #[cfg(target_os = "macos")]
        {
            // Open with submenu: Finder convention, shown for files, not directories.
            let (submenu, map) = super::open_with::build_open_with_submenu(app, &info.open_with.candidates)?;
            menu.append(&submenu)?;
            open_with_apps = map;
        }
        menu.append(&view_item)?;
        menu.append(&edit_item)?;
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }

    // Everything that changes WHAT is selected, in its own group between activation
    // (Open / View / Edit) and the operations (Copy / Move / Rename) that act on it.
    // Toggle selection leads it, which is the only place Space is discoverable.
    // `super::selection_submenu` owns the rows.
    menu.append(&build_selection_submenu(app, shortcuts, same_kind)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;

    // Finder tag colors (macOS): seven circles that toggle the system color tags on the
    // selection. Shown for files and folders (Finder tags both).
    #[cfg(target_os = "macos")]
    append_tag_color_group(app, &menu, info)?;

    // Copy / Move / Duplicate / Rename group. Rename and Duplicate are omitted on the
    // search-results virtual pane: the underlying file CAN be renamed, but doing it from
    // the snapshot view splits the file (snapshot keeps the old name, disk has the new)
    // which is confusing, and a duplicate would have to land in each item's own real
    // folder, which one transfer can't express. The user can navigate to the real folder
    // and do either there.
    let copy_item = context_item(app, shortcuts, FILE_COPY_ID, menu_t("menu.file.copy"), true)?;
    let move_item = context_item(app, shortcuts, FILE_MOVE_ID, menu_t("menu.file.move"), true)?;
    menu.append(&copy_item)?;
    menu.append(&move_item)?;
    if !restrict_destination_actions {
        let duplicate_item = context_item(app, shortcuts, FILE_DUPLICATE_ID, menu_t("menu.file.duplicate"), true)?;
        menu.append(&duplicate_item)?;
        let rename_item = context_item(app, shortcuts, RENAME_ID, menu_t("menu.file.rename"), true)?;
        menu.append(&rename_item)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;

    // New folder / New file — also omitted on search-results panes (no destination
    // folder to create into; the pane IS the snapshot, not a directory).
    if !restrict_destination_actions {
        let new_folder_item = context_item(app, shortcuts, FILE_NEW_FOLDER_ID, menu_t("menu.file.newFolder"), true)?;
        let new_file_item = context_item(app, shortcuts, FILE_NEW_FILE_ID, menu_t("menu.file.newFile"), true)?;
        menu.append(&new_folder_item)?;
        menu.append(&new_file_item)?;
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }

    // Delete
    let delete_item = context_item(app, shortcuts, FILE_DELETE_ID, menu_t("menu.file.delete"), true)?;
    menu.append(&delete_item)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;

    // Utility group: Show in Finder, Copy filename, Copy path
    let show_in_finder_item = context_item(
        app,
        shortcuts,
        SHOW_IN_FINDER_ID,
        menu_t(SHOW_IN_FILE_MANAGER_KEY.current()),
        true,
    )?;
    let copy_filename_item = context_item(
        app,
        shortcuts,
        COPY_FILENAME_ID,
        menu_t_with(
            "menu.context.copyNamed",
            &[("name", &truncate_for_menu_label(filename, COPY_FILENAME_MAX_CHARS))],
        ),
        true,
    )?;
    let copy_path_item = context_item(app, shortcuts, COPY_PATH_ID, menu_t("menu.edit.copyPath"), true)?;
    menu.append(&show_in_finder_item)?;
    // "Open terminal here" rides beside Show in Finder, same gesture aimed at a
    // different app. macOS only, like the launch module behind it.
    #[cfg(target_os = "macos")]
    {
        let open_terminal_here_item = context_item(
            app,
            shortcuts,
            OPEN_TERMINAL_HERE_ID,
            menu_t("menu.file.openTerminalHere"),
            can_open_terminal_here,
        )?;
        menu.append(&open_terminal_here_item)?;
        // `Share` rides with them for the same reason: all three hand the selection to
        // something outside Cmdr. It's ABSENT, never greyed, on two counts, and the
        // second is why it's a submenu at all:
        // - the pane's rows don't live on the OS filesystem (a phone, an archive's
        //   insides), so there are no file URLs to hand over, and no wording of a greyed
        //   item explains "this row isn't a file yet" better than its absence does;
        // - macOS offers no service for this selection (a path that vanished, a broken
        //   symlink), which only an enumeration can answer. The system popover can't:
        //   it comes up empty but for `Edit Extensions…`, which is the bug that put the
        //   list in a submenu.
        if can_share && !info.share_services.is_empty() {
            menu.append(&super::share_submenu::build_share_submenu(app, &info.share_services)?)?;
        }
    }
    menu.append(&copy_filename_item)?;
    menu.append(&copy_path_item)?;

    // Add to favorites — directories only (favorites are folders), and only where a favorite
    // could point back: `can_favorite` is the caller's reading of the row, matching the gate
    // `add_favorite` enforces. Favorites the right-clicked folder's path, which
    // `on_menu_event` reads from `MenuState.context.path`.
    if is_directory && can_favorite {
        let add_favorite_item = MenuItem::with_id(
            app,
            FAVORITES_ADD_CONTEXT_ID,
            menu_t("menu.context.addToFavorites"),
            true,
            None::<&str>,
        )?;
        menu.append(&PredefinedMenuItem::separator(app)?)?;
        menu.append(&add_favorite_item)?;
    }

    // Image-search group (media_index): folder-only, and only while image indexing is
    // enabled. `image_index_menu_items` decides the labels and which items are clickable.
    // Handled specially in `handle_menu_event` (they act on the right-clicked folder and
    // drive a FE persist path), never via `menu_id_to_command`.
    if is_directory {
        let items = image_index_menu_items(image_index);
        if !items.is_empty() {
            menu.append(&PredefinedMenuItem::separator(app)?)?;
            for item in items {
                let menu_item = MenuItem::with_id(app, item.id, menu_t(item.label_key), item.enabled, None::<&str>)?;
                menu.append(&menu_item)?;
            }
        }
    }

    // Cloud group (macOS). Provider-aware: each provider contributes only the
    // actions it can actually carry out, so the group is a concatenation rather
    // than one iCloud-shaped block.
    //
    // Google Drive: open on the web / copy the link / ask Gemini about it. These build
    // web URLs, so they work in mirror mode too; Drive's own File Provider actions (Share
    // among them) come in the provider group below, minus these three.
    // `file_system/google_drive/` has the full story.
    #[cfg(target_os = "macos")]
    if let Some(links) = &info.google_drive_links {
        let open_item = MenuItem::with_id(
            app,
            DRIVE_OPEN_ID,
            menu_t("menu.context.openInGoogleDrive"),
            true,
            None::<&str>,
        )?;
        let copy_link_item = MenuItem::with_id(
            app,
            DRIVE_COPY_LINK_ID,
            menu_t("menu.context.copyGoogleDriveLink"),
            true,
            None::<&str>,
        )?;
        menu.append(&PredefinedMenuItem::separator(app)?)?;
        menu.append(&open_item)?;
        menu.append(&copy_link_item)?;
        // Files only: Gemini's `?di=` names a document, and a folder resolves no
        // Gemini URL at all.
        if links.gemini_url.is_some() {
            let ask_gemini_item = MenuItem::with_id(
                app,
                DRIVE_ASK_GEMINI_ID,
                menu_t("menu.context.askGemini"),
                true,
                None::<&str>,
            )?;
            menu.append(&ask_gemini_item)?;
        }
    }

    // Eviction pair: iCloud Drive ONLY, and gated by sync status. The
    // `FileManager` ubiquity APIs behind these accept iCloud URLs and nothing
    // else; a third-party provider's pin/unpin is a File Provider custom action
    // reserved for the app that bundles the extension. ❌ Don't widen this to
    // other providers — see `file_system/cloud_actions.rs`.
    #[cfg(target_os = "macos")]
    if info.is_icloud_drive {
        let cloud_item = match info.sync_status {
            SyncStatus::OnlineOnly => Some(MenuItem::with_id(
                app,
                CLOUD_MAKE_OFFLINE_ID,
                menu_t("menu.context.makeAvailableOffline"),
                true,
                None::<&str>,
            )?),
            SyncStatus::Synced => Some(MenuItem::with_id(
                app,
                CLOUD_REMOVE_DOWNLOAD_ID,
                menu_t("menu.context.removeDownload"),
                true,
                None::<&str>,
            )?),
            // Uploading/Downloading: action already in flight, don't offer either.
            // Unknown: status query failed, hide to avoid confusion.
            _ => None,
        };
        if let Some(item) = cloud_item {
            menu.append(&PredefinedMenuItem::separator(app)?)?;
            menu.append(&item)?;
        }
    }

    // The provider's own actions (Dropbox, Google Drive, MacDroid, …), evaluated the way
    // Finder does and in the provider's order and words, below Cmdr's own cloud items.
    #[cfg(target_os = "macos")]
    if let Some(offer) = &info.file_provider_offer {
        super::file_provider_items::append_file_provider_group(app, &menu, offer)?;
    }

    // Quick Look and Get Info are macOS-only
    #[cfg(target_os = "macos")]
    {
        let get_info_item = context_item(app, shortcuts, GET_INFO_ID, menu_t("menu.file.getInfo"), true)?;
        let quick_look_item = MenuItem::with_id(app, QUICK_LOOK_ID, menu_t("menu.file.quickLook"), true, None::<&str>)?;
        menu.append(&PredefinedMenuItem::separator(app)?)?;
        menu.append(&get_info_item)?;
        menu.append(&quick_look_item)?;
        // `Services` goes last, where Finder puts it, and rides on the same fact as
        // "Share…": AppKit hands a service file URLs, so a pane whose rows aren't OS
        // paths has nothing to offer. Only the item is built here — AppKit's own menu
        // is borrowed while the menu is up (`services_context.rs`).
        if can_share {
            super::services_context::append_services_submenu(app, &menu)?;
        }
    }

    Ok(ContextMenuResult {
        menu,
        #[cfg(target_os = "macos")]
        open_with_apps,
    })
}

/// Appends the seven Finder-tag color items (macOS) plus a trailing separator.
///
/// Each item is an `IconMenuItem` showing its color circle (open_with.rs pattern); the
/// "applied" colors (every selected file already carries them) get the checkmark-
/// composited variant. IDs are `tag-color:<index>`, prefix-routed in
/// `handle_menu_event`. Colors run in Finder's order (Red … Gray). These are the
/// FALLBACK look: once the menu tracks, `tag_row` folds them into Finder's single row of
/// circles and fires these same items on a click. The label carries the color's NAME,
/// which the row matches on and VoiceOver reads, which is why the names are translated
/// alongside everything else. macOS-only — Linux menus carry no icons.
#[cfg(target_os = "macos")]
fn append_tag_color_group<R: Runtime>(app: &AppHandle<R>, menu: &Menu<R>, info: &FileContextInfo) -> tauri::Result<()> {
    use tauri::menu::IconMenuItem;

    for swatch in &super::tag_row::SWATCHES {
        let id = format!("{}{}", super::TAG_COLOR_ID_PREFIX, swatch.color);
        let checked = info.applied_tag_colors[usize::from(swatch.color)];
        // `IconMenuItem` with `Some(image)` falls back to a text-only item if the image
        // build fails, so the menu still works without the circle.
        let icon = super::tag_icons::tag_circle_image(swatch.color, checked);
        let item = IconMenuItem::with_id(app, &id, menu_t(swatch.name_key), true, icon, None::<&str>)?;
        menu.append(&item)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    Ok(())
}

/// Builds the minimal context menu for the `..` parent row: a single "Add to favorites" item that
/// favorites the parent directory. The full file context menu (Copy / Move / Delete, etc.) makes no
/// sense on `..`, so this is its own one-item menu. The caller stashes the parent dir in
/// `MenuState.context.path`; `on_menu_event` reads it back for the `FAVORITES_ADD_CONTEXT_ID` click.
pub fn build_parent_row_context_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    let add_favorite_item = MenuItem::with_id(
        app,
        FAVORITES_ADD_CONTEXT_ID,
        menu_t("menu.context.addToFavorites"),
        true,
        None::<&str>,
    )?;
    menu.append(&add_favorite_item)?;
    Ok(menu)
}

/// Builds the minimal context menu for the function key bar: a single "Hide function key bar"
/// item. Unlike the parent-row favorite, the action needs no right-clicked context to stash, so
/// `on_menu_event` routes the click straight to the frontend via the `FunctionKeyBarHideRequested`
/// event rather than intercepting it with stashed state.
pub fn build_function_key_bar_context_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    let hide_item = MenuItem::with_id(
        app,
        FUNCTION_KEY_BAR_HIDE_ID,
        menu_t("menu.context.hideFunctionKeyBar"),
        true,
        None::<&str>,
    )?;
    menu.append(&hide_item)?;
    Ok(menu)
}

/// Builds a context menu for the breadcrumb path bar.
///
/// Its one command's accelerator comes from `shortcuts` like the file menu's do; see
/// [`ContextMenuShortcuts`]. `eject_volume_name`, when present, appends the detach item that lets the user
/// leave the volume the breadcrumb represents: `Eject ({name})` for a disk,
/// `Disconnect` for a phone (`detach_word`). The caller is responsible for
/// stashing the matching `volume_id` in `MenuState.volume_eject_context` so
/// `on_menu_event` can dispatch the click.
///
/// When `eject_busy` is true, the item is rendered disabled with a ` (busy)`
/// suffix, so a volume with a write op reading from / writing to it can't be
/// ejected mid-transfer (mirrors the disabled eject button in the picker).
pub fn build_breadcrumb_context_menu<R: Runtime>(
    app: &AppHandle<R>,
    shortcuts: &ContextMenuShortcuts,
    eject_volume_name: Option<&str>,
    eject_busy: bool,
    detach_word: DetachWord,
) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    let copy_path_item = context_item(
        app,
        shortcuts,
        COPY_CURRENT_DIR_PATH_ID,
        menu_t("menu.breadcrumb.copyPath"),
        true,
    )?;
    menu.append(&copy_path_item)?;
    if let Some(name) = eject_volume_name {
        let eject_item = MenuItem::with_id(
            app,
            EJECT_VOLUME_ID,
            detach_label(name, eject_busy, detach_word),
            !eject_busy,
            None::<&str>,
        )?;
        menu.append(&eject_item)?;
    }
    Ok(menu)
}

/// What a SERVER row's context menu offers, as the caller sees the row.
///
/// ❗ The caller decides which items apply, ❌ never this builder:
/// `show_volume_row_context_menu` is a synchronous command. ❗ And "is a secret
/// stored for this?" is asked by NOBODY on this path: it costs a Keychain read,
/// every read of one can raise a system prompt, and a right-click is not a moment
/// to spend one, so "Forget saved password" is offered unconditionally and the
/// command it runs reports whether there was one.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ServerRowMenu {
    /// Whether there is a session to drop (`showsDisconnect` in
    /// `navigation/connection-state.ts`: a `direct` or `disconnected` place).
    pub shows_disconnect: bool,
    /// Whether a saved entry exists, so "Forget server" has something to forget.
    pub is_saved: bool,
    /// Whether the place is in the volume switcher right now, which decides
    /// whether the row offers "Pin to switcher" or "Unpin".
    pub pinned: bool,
    /// Whether a write operation is touching the volume right now. ❗ Disables
    /// every destructive item exactly like the eject item, because dropping the
    /// session or the credential under a running copy breaks it.
    pub busy: bool,
}

/// Appends a server row's items, in the order
/// `apps/desktop/src/lib/file-explorer/navigation/DETAILS.md` § "Eject button +
/// row context menu" records: Open, Edit…, Disconnect (when live), Pin to
/// switcher / Unpin, Forget saved password, Forget server (when it is saved).
///
/// ❗ A server row shows Disconnect, ❌ never Eject: "Eject" promises
/// safe-to-unplug, and a server has nothing to unplug.
fn append_server_row_items<R: Runtime>(
    app: &AppHandle<R>,
    menu: &Menu<R>,
    server: &ServerRowMenu,
) -> tauri::Result<()> {
    // Open and Edit… lead, the way the row's own two purposes rank: going there,
    // and changing what "there" means. ❗ Neither is gated by `busy`, unlike the
    // three destructive items below: navigating into a server a copy is reading
    // from is fine, and editing its settings touches no session.
    let open = MenuItem::with_id(app, SERVER_OPEN_ID, menu_t("menu.network.open"), true, None::<&str>)?;
    menu.append(&open)?;
    if server.is_saved {
        // ❌ Only for a SAVED server: the sheet edits a store entry, and a live
        // volume nothing saved has none to open.
        let edit = MenuItem::with_id(app, SERVER_EDIT_ID, menu_t("menu.network.edit"), true, None::<&str>)?;
        menu.append(&edit)?;
    }
    if server.shows_disconnect {
        let key = if server.busy {
            "menu.volume.disconnectBusy"
        } else {
            "menu.network.disconnect"
        };
        let item = MenuItem::with_id(app, SERVER_DISCONNECT_ID, menu_t(key), !server.busy, None::<&str>)?;
        menu.append(&item)?;
    }
    // ❗ Never disabled by `busy`, unlike the three below it: a pin is a view
    // preference the switcher reads, so moving it while a copy runs breaks
    // nothing.
    let (pin_id, pin_key) = if server.pinned {
        (SERVER_UNPIN_ID, "menu.network.unpin")
    } else {
        (SERVER_PIN_ID, "menu.network.pinToSwitcher")
    };
    let pin_item = MenuItem::with_id(app, pin_id, menu_t(pin_key), true, None::<&str>)?;
    menu.append(&pin_item)?;
    // ❗ Offered on every server row, ❌ never gated on "is a secret stored?":
    // answering that costs a Keychain read, and every read of one can raise a
    // system prompt. A right-click is not a moment to spend one, which is the
    // rule SMB's host menu already follows. The COMMAND answers instead:
    // `forget_server_secret` reports whether an entry was there, and the caller
    // words a `false` (`navigation/server-row-actions.ts::forgetSavedSecret`).
    let key = if server.busy {
        "menu.volume.forgetSavedPasswordBusy"
    } else {
        "menu.network.forgetSavedPassword"
    };
    let item = MenuItem::with_id(app, SERVER_FORGET_SECRET_ID, menu_t(key), !server.busy, None::<&str>)?;
    menu.append(&item)?;
    if server.is_saved {
        let key = if server.busy {
            "menu.volume.forgetServerBusy"
        } else {
            "menu.network.forgetServer"
        };
        let item = MenuItem::with_id(app, SERVER_FORGET_ID, menu_t(key), !server.busy, None::<&str>)?;
        menu.append(&item)?;
    }
    Ok(())
}

/// Builds a menu for viewer windows (built from scratch on all platforms).
///
/// Returns the menu plus the `Word wrap` CheckMenuItem ref so the caller can flip its checked state
/// in O(1) (see `ViewerMenuItems`). On macOS the menu is installed app-level via `app.set_menu()`;
/// on Linux it's a per-window menu (`window.set_menu()`).
pub fn build_viewer_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<ViewerMenuItems<R>> {
    let menu = Menu::new(app)?;

    #[cfg(target_os = "macos")]
    {
        // --- cmdr app menu (minimal for viewer) ---
        let viewer_app_menu = Submenu::with_items(
            app,
            APP_MENU_TITLE,
            true,
            &[
                &PredefinedMenuItem::hide(app, Some(&menu_t("menu.app.hide")))?,
                &PredefinedMenuItem::hide_others(app, Some(&menu_t("menu.app.hideOthers")))?,
                &PredefinedMenuItem::show_all(app, Some(&menu_t("menu.app.showAll")))?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::quit(app, Some(&menu_t("menu.app.quit")))?,
            ],
        )?;
        menu.append(&viewer_app_menu)?;
    }

    // --- File menu ---
    let file_menu = Submenu::with_items(
        app,
        menu_t("menu.bar.file"),
        true,
        &[&PredefinedMenuItem::close_window(
            app,
            Some(&menu_t("menu.viewer.close")),
        )?],
    )?;
    menu.append(&file_menu)?;

    // --- Edit menu ---
    // Predefined items carry the native cut:/copy:/paste:/selectAll: selectors, which
    // macOS routes to the focused text field (the search box) through the responder
    // chain. All four are needed: without Cut/Paste, ⌘X/⌘V are dead in the viewer's
    // search input (the viewer menu is the active app menu while a viewer is focused).
    // Carries the same ID as the main bar's Edit menu: `cleanup_macos_menus` runs against whichever
    // bar is installed, and AppKit injects its Writing Tools / AutoFill / Dictation items into this
    // one too. Only one of the two bars is ever installed at a time, so the shared ID never collides.
    let edit_menu = Submenu::with_id_and_items(
        app,
        EDIT_MENU_ID,
        menu_t("menu.bar.edit"),
        true,
        &[
            &PredefinedMenuItem::cut(app, Some(&menu_t("menu.edit.cut")))?,
            &PredefinedMenuItem::copy(app, Some(&menu_t("menu.edit.copy")))?,
            &PredefinedMenuItem::paste(app, Some(&menu_t("menu.edit.paste")))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::select_all(app, Some(&menu_t("menu.select.all")))?,
        ],
    )?;
    menu.append(&edit_menu)?;

    // --- View menu ---
    // `word_wrap` is returned so the caller (and `viewer_set_word_wrap`) can flip its checked state
    // directly without a tree walk.
    let word_wrap = CheckMenuItem::with_id(
        app,
        VIEWER_WORD_WRAP_ID,
        menu_t("menu.viewer.wordWrap"),
        true,
        false,
        None::<&str>,
    )?;
    let view_submenu = Submenu::with_items(app, menu_t("menu.bar.view"), true, &[&word_wrap])?;
    menu.append(&view_submenu)?;

    #[cfg(target_os = "macos")]
    {
        // --- Window menu ---
        let window_menu = Submenu::with_items(
            app,
            menu_t("menu.bar.window"),
            true,
            &[
                &PredefinedMenuItem::minimize(app, Some(&menu_t("menu.window.minimize")))?,
                &PredefinedMenuItem::maximize(app, Some(&menu_t("menu.window.zoom")))?,
            ],
        )?;
        menu.append(&window_menu)?;

        // --- Help menu ---
        // Empty, but it still needs the ID: `cleanup_macos_menus` hands it to
        // `NSApplication.setHelpMenu:` so the viewer bar gets the search field too.
        let help_menu = Submenu::with_id_and_items(app, HELP_MENU_ID, menu_t("menu.bar.help"), true, &[])?;
        menu.append(&help_menu)?;
    }

    Ok(ViewerMenuItems { menu, word_wrap })
}

/// Builds a context menu for a tab.
pub fn build_tab_context_menu(
    app: &AppHandle<Wry>,
    is_pinned: bool,
    can_close: bool,
    has_other_unpinned_tabs: bool,
) -> tauri::Result<Menu<Wry>> {
    let menu = Menu::new(app)?;

    let pin_item = MenuItem::with_id(app, TAB_PIN_ID, pin_tab_label(is_pinned), true, None::<&str>)?;
    let close_others_item = MenuItem::with_id(
        app,
        TAB_CLOSE_OTHERS_ID,
        menu_t("menu.tab.closeOtherTabs"),
        has_other_unpinned_tabs,
        None::<&str>,
    )?;
    let close_item = MenuItem::with_id(app, TAB_CLOSE_ID, menu_t("menu.tab.closeTab"), can_close, None::<&str>)?;

    menu.append(&pin_item)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&close_others_item)?;
    menu.append(&close_item)?;

    Ok(menu)
}

/// Builds a context menu for a network host.
/// Always includes "Disconnect". Conditionally adds "Forget server" (manual hosts)
/// and "Forget saved password" (hosts with stored credentials).
pub fn build_network_host_context_menu(
    app: &AppHandle<Wry>,
    is_manual: bool,
    has_credentials: bool,
) -> tauri::Result<Menu<Wry>> {
    let menu = Menu::new(app)?;

    // "Disconnect" is always shown. If nothing is mounted, the backend handles it gracefully.
    let disconnect = MenuItem::with_id(
        app,
        NETWORK_HOST_DISCONNECT_ID,
        menu_t("menu.network.disconnect"),
        true,
        None::<&str>,
    )?;
    menu.append(&disconnect)?;

    if is_manual {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
        let forget_server = MenuItem::with_id(
            app,
            NETWORK_HOST_FORGET_SERVER_ID,
            menu_t("menu.network.forgetServer"),
            true,
            None::<&str>,
        )?;
        menu.append(&forget_server)?;
    }

    if has_credentials {
        if !is_manual {
            menu.append(&PredefinedMenuItem::separator(app)?)?;
        }
        let forget_secret = MenuItem::with_id(
            app,
            NETWORK_HOST_FORGET_SECRET_ID,
            menu_t("menu.network.forgetSavedPassword"),
            true,
            None::<&str>,
        )?;
        menu.append(&forget_secret)?;
    }

    Ok(menu)
}

/// Builds the context menu for a VOLUME row in the volume switcher.
///
/// An ejectable volume gets its detach item, `Eject ({name})` for a disk and
/// `Disconnect` for a phone (`detach_word`), disabled with a ` (busy)` suffix while a
/// write op touches it, mirroring the breadcrumb menu and the inline control. A SERVER
/// row gets its own items instead. The caller stashes the target id + name in
/// `MenuState.volume_row_context` so `on_menu_event` can dispatch the click.
///
/// A FAVORITE row is a different surface with a different menu:
/// [`build_favorite_context_menu`].
pub fn build_volume_row_context_menu<R: Runtime>(
    app: &AppHandle<R>,
    eject_volume_name: Option<&str>,
    eject_busy: bool,
    detach_word: DetachWord,
    server: Option<&ServerRowMenu>,
) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;

    if let Some(server) = server {
        append_server_row_items(app, &menu, server)?;
        return Ok(menu);
    }

    if let Some(name) = eject_volume_name {
        let eject_item = MenuItem::with_id(
            app,
            EJECT_VOLUME_ID,
            detach_label(name, eject_busy, detach_word),
            !eject_busy,
            None::<&str>,
        )?;
        menu.append(&eject_item)?;
    }

    Ok(menu)
}

/// Builds the context menu for a FAVORITE row: `Rename` + `Remove from favorites`.
///
/// Both items are always enabled: a favorite is a stored `{ path, name }` pair, so
/// neither action can be refused by anything the popup could read. A favorite is never
/// ejectable and never a server, which is why this shares nothing with
/// [`build_volume_row_context_menu`] beyond the `MenuState.volume_row_context` stash and
/// the `volume-context-action` event both picks ride home on.
pub fn build_favorite_context_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;

    let rename_item = MenuItem::with_id(
        app,
        FAVORITE_RENAME_ID,
        menu_t("menu.volume.renameFavorite"),
        true,
        None::<&str>,
    )?;
    menu.append(&rename_item)?;
    let remove_item = MenuItem::with_id(
        app,
        FAVORITE_REMOVE_ID,
        menu_t("menu.volume.removeFavorite"),
        true,
        None::<&str>,
    )?;
    menu.append(&remove_item)?;

    Ok(menu)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::{FAVORITES_ADD_CONTEXT_ID, FILE_COPY_ID, FILE_VIEW_ID, TOGGLE_SELECTION_ID};

    fn shortcuts(pairs: &[(&str, &str)]) -> ContextMenuShortcuts {
        ContextMenuShortcuts(
            pairs
                .iter()
                .map(|(id, combo)| ((*id).to_string(), (*combo).to_string()))
                .collect(),
        )
    }

    /// The default binding reaches the item through the registry, not through a
    /// literal in the builder.
    #[test]
    fn a_bound_command_labels_its_menu_item() {
        let shortcuts = shortcuts(&[("file.copy", "F5")]);
        assert_eq!(shortcuts.for_menu_item(FILE_COPY_ID), Some("F5".to_string()));
    }

    /// The whole point of the milestone: rebind Copy in Settings and the right-click
    /// menu says the new key.
    #[test]
    fn a_rebound_command_shows_the_new_key() {
        let shortcuts = shortcuts(&[("file.copy", "⌘⇧K")]);
        assert_eq!(shortcuts.for_menu_item(FILE_COPY_ID), Some("Cmd+Shift+K".to_string()));
    }

    /// A bare key stays a real label here, where the menu bar would refuse one: a
    /// popup accelerator is never registered, so it can't swallow the key app-wide.
    /// Space is the only place the toggle-selection shortcut is discoverable.
    #[test]
    fn a_bare_key_still_labels_a_popup_item() {
        let shortcuts = shortcuts(&[("selection.toggle", "Space")]);
        assert_eq!(shortcuts.for_menu_item(TOGGLE_SELECTION_ID), Some("Space".to_string()));
    }

    /// A command the user has unbound shows no label, rather than the one it shipped
    /// with.
    #[test]
    fn an_unbound_command_shows_nothing() {
        let shortcuts = shortcuts(&[("file.copy", "F5")]);
        assert_eq!(shortcuts.for_menu_item(FILE_VIEW_ID), None);
    }

    /// An item with no command behind it (this one favorites the right-clicked path
    /// in `on_menu_event`) can't resolve a shortcut, and says nothing.
    #[test]
    fn an_item_outside_the_command_map_shows_nothing() {
        let shortcuts = shortcuts(&[("favorites.add", "⌘D")]);
        assert_eq!(shortcuts.for_menu_item(FAVORITES_ADD_CONTEXT_ID), None);
    }
}
