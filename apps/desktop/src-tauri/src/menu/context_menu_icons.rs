//! Every image on the file context menu (macOS): SF Symbols on Cmdr's own items, each File
//! Provider's logo on its actions, the app icons in "Open with", each service's own icon in
//! "Share", and the tag items' fallback circles.
//!
//! The menu BAR gets its icons at build time (`macos_appkit.rs`'s `MENU_BAR_ICONS`),
//! because Tauri hands out the installed bar's `NSMenu`. A context menu's it does not
//! (muda's `ns_menu()` sits behind Tauri's sealed `ContextMenuBase`), so this reaches
//! the items through `NSMenuDidBeginTrackingNotification`, exactly as
//! `services_context.rs` does and for exactly the same reason. AppKit posts it before
//! the menu is laid out, which is why an image set here still gets its gutter. The
//! submenus ("Open with", "Share") already hang off the root `NSMenu` by then, so they're
//! reached from the root's notification and done before either one opens.
//!
//! ## Why not `IconMenuItem`, which needs no AppKit at all
//!
//! Because the image it sets never reaches `set_menu_item_image`, the one door that opts
//! an item into staying visible on macOS 27, and there's no `NSMenuItem` to opt in
//! afterwards. Linked against the macOS 27 SDK, every `IconMenuItem` image draws nothing.
//! It also can't render a template image (muda hands `NSImage` a PNG and never calls
//! `setTemplate:`), so a monochrome glyph would vanish in one appearance anyway. Clippy
//! refuses `IconMenuItem` crate-wide (`clippy.toml`), so nobody reaches for it by habit.
//!
//! So the menu is built from plain items, and [`image_runs`] says which image each one
//! carries. Images are keyed by item ID and resolved to live titles at arm time, the house
//! rule for crossing into AppKit.
//!
//! ## Why runs, not single titles
//!
//! A title is only a title. Two apps or two share extensions can carry the same name, and
//! the header line carries the bare filename, so a folder named `Mail` would take the Mail
//! service's icon. Each group of items is matched as ONE contiguous run of titles inside
//! the menu that holds it ([`find_title_run`]), which a stray lookalike can't satisfy, and
//! duplicates inside a run pair up by position.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{NSImage, NSMenu, NSMenuItem};
use objc2_foundation::{NSCopying, NSData, NSNotification, NSSize};
use tauri::Runtime;
use tauri::menu::{Menu, MenuItemKind};

use super::file_context_menu::FileContextInfo;
use super::file_provider_items::file_provider_action_id;
use super::macos_appkit::{
    find_ns_submenu, menu_item_text, observe_menu_tracking, plain_title, set_menu_item_image, sf_symbol_image,
    tracking_menu,
};
use super::open_with::{OPEN_WITH_ID_PREFIX, OPEN_WITH_SUBMENU_ID};
use super::provider_logos::{ProviderLogo, logo_for_provider};
use super::share_submenu::{SHARE_SUBMENU_ID, share_service_id};
use super::tag_row::SWATCHES;
use super::{DRIVE_ASK_GEMINI_ID, DRIVE_COPY_LINK_ID, DRIVE_OPEN_ID, TAG_COLOR_ID_PREFIX};
use crate::file_system::open_with::AppIcon;

/// `(menu item ID, SF Symbol name)` for the file context menu.
///
/// Everything here is an ID, never a label: a title is user-facing text that
/// translation moves, and an icon that stops matching disappears without a sound.
/// Items with no entry show no icon, which is the norm: icons mark the actions worth
/// spotting at a glance, not every line.
///
/// The three Drive items are the whole list today. `link` is the same symbol the menu
/// bar's `Copy path` carries, on purpose: the same concept gets the same glyph, which is
/// already how `Copy` shares `document.on.document` across two menus. `sparkles` is what
/// Apple and Google both spell AI with, so `Ask Gemini` reads as one at a glance.
/// Provider actions (`file_provider_items.rs`) get no symbol: their labels are the
/// provider's, and a glyph Cmdr picked would claim to know what each one does. They get
/// their provider's logo instead ([`image_runs`]), which only says whose action it is.
const FILE_CONTEXT_ICONS: &[(&str, &str)] = &[
    (DRIVE_OPEN_ID, "arrow.up.forward.app"),
    (DRIVE_COPY_LINK_ID, "link"),
    (DRIVE_ASK_GEMINI_ID, "sparkles"),
];

/// The side of every non-symbol image except the tag circles, in points: the box the SF
/// Symbols beside them take.
///
/// At the menu's 13 pt font, `link` is 17 × 17, `sparkles` 15 × 17, and
/// `arrow.up.forward.app` 15 × 14 (verified on macOS 26.6, `NSImage.size` from a Swift
/// probe, 2026-09-12). A share service's own image is 16 × 16 too (macOS 26.6.2,
/// `image.size` on all nine services offered for a text file, 2026-09-09).
const IMAGE_SIDE_POINTS: f64 = 16.0;

/// What one item shows. No AppKit in here, so which item gets what stays unit-testable;
/// [`render`] turns it into an `NSImage` at arm time.
#[derive(Clone)]
enum ItemImage {
    /// An SF Symbol, which AppKit tints for light, dark, and highlight.
    Symbol(&'static str),
    /// A provider's colored logo, from its SVG.
    Logo(&'static ProviderLogo),
    /// An app's own icon, read from its bundle off the main thread (`load_app_icon`).
    AppIcon(AppIcon),
    /// A Finder tag's circle, with the check composited in when every row carries it.
    TagCircle { color: u8, applied: bool },
    /// The icon of the service at this index in the live Share offer, macOS's own image.
    ShareService(usize),
}

/// Which menu a run's items sit in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunHost {
    /// The context menu itself.
    Menu,
    /// The submenu with this ID, directly inside the context menu.
    Submenu(&'static str),
}

/// A group of items that sit next to each other, matched as one.
///
/// Items with no image still belong to the run (an app whose bundle has no readable icon),
/// because contiguity is what the match leans on.
#[derive(Clone)]
struct ImageRun {
    host: RunHost,
    /// `(menu item ID, what it shows)`, in menu order.
    items: Vec<(String, Option<ItemImage>)>,
}

/// Every image the file context menu for `info` can carry, grouped into runs.
///
/// Which of these the menu actually built is the arm step's question: the Drive items exist
/// only for a Drive row, "Open with" only for a file, "Share" only when macOS offers a
/// service, and an offer's lines only when File Provider vouched for them. A run whose items
/// the menu doesn't have is dropped whole.
fn image_runs(info: &FileContextInfo) -> Vec<ImageRun> {
    let symbols = FILE_CONTEXT_ICONS.iter().map(|&(id, symbol)| ImageRun {
        host: RunHost::Menu,
        items: vec![(id.to_string(), Some(ItemImage::Symbol(symbol)))],
    });
    let logos = info
        .file_provider_offer
        .as_ref()
        .filter(|offer| !offer.actions.is_empty())
        .and_then(|offer| Some((offer.actions.len(), logo_for_provider(&offer.provider_id)?)))
        .map(|(count, logo)| ImageRun {
            host: RunHost::Menu,
            items: (0..count)
                .map(|index| (file_provider_action_id(index), Some(ItemImage::Logo(logo))))
                .collect(),
        });
    let tag_circles = ImageRun {
        host: RunHost::Menu,
        items: SWATCHES
            .iter()
            .map(|swatch| {
                let applied = info.applied_tag_colors[usize::from(swatch.color)];
                let circle = ItemImage::TagCircle {
                    color: swatch.color,
                    applied,
                };
                (format!("{TAG_COLOR_ID_PREFIX}{}", swatch.color), Some(circle))
            })
            .collect(),
    };
    let open_with = (!info.open_with.candidates.is_empty()).then(|| ImageRun {
        host: RunHost::Submenu(OPEN_WITH_SUBMENU_ID),
        items: info
            .open_with
            .candidates
            .iter()
            .map(|app| {
                let id = format!("{OPEN_WITH_ID_PREFIX}{}", app.bundle_id);
                (id, app.icon.clone().map(ItemImage::AppIcon))
            })
            .collect(),
    });
    let share = (!info.share_services.is_empty()).then(|| ImageRun {
        host: RunHost::Submenu(SHARE_SUBMENU_ID),
        items: (0..info.share_services.len())
            .map(|index| (share_service_id(index), Some(ItemImage::ShareService(index))))
            .collect(),
    });
    symbols
        .chain(std::iter::once(tag_circles))
        .chain(logos)
        .chain(open_with)
        .chain(share)
        .collect()
}

/// Where `run` starts as one contiguous stretch of `titles`, or `None` when it doesn't.
///
/// Titles compare without the display-accelerator run `display_accelerators.rs` may have
/// put after a TAB. An empty run, or one holding an empty title, matches nothing: a
/// separator's title is empty, and a match on it would be a match on nothing.
fn find_title_run<S: AsRef<str>>(titles: &[S], run: &[String]) -> Option<usize> {
    if run.is_empty() || run.iter().any(String::is_empty) {
        return None;
    }
    titles.windows(run.len()).position(|window| {
        window
            .iter()
            .zip(run)
            .all(|(title, want)| plain_title(title.as_ref()) == want)
    })
}

/// A run ready for the tracking menu: live titles, and the images already made.
#[derive(Clone)]
struct ArmedRun {
    /// The live title of the submenu the run sits in, or `None` for the context menu itself.
    submenu_title: Option<String>,
    titles: Vec<String>,
    images: Vec<Option<Retained<NSImage>>>,
}

/// Images armed for the next context menu to open.
///
/// A title rather than an ID, because AppKit has never heard of a Tauri menu ID: the
/// title is read off the live Tauri item at arm time, which is the house rule for
/// crossing that boundary and what keeps the match working once labels are translated.
pub struct IconLoan(MainThreadMarker);

impl Drop for IconLoan {
    fn drop(&mut self) {
        ARMED.with(|slot| slot.borrow_mut().clear());
    }
}

thread_local! {
    /// The runs for the menu currently going up. Main-thread-only by construction: every
    /// reader runs from the menu thread, and it holds `NSImage`s.
    static ARMED: RefCell<Vec<ArmedRun>> = const { RefCell::new(Vec::new()) };
    /// Whether the tracking observer is registered. Registering it lazily keeps the
    /// cost with the first right-click instead of every launch.
    static OBSERVING: Cell<bool> = const { Cell::new(false) };
}

/// Arms every image the file context menu `menu` carries, per [`image_runs`]. The menu must
/// be about to `popup()`.
///
/// ❗ Hold the returned guard until `popup()` returns, the way `ServicesLoan` is held:
/// `popup()` runs AppKit's tracking loop, so the menu only starts tracking inside it.
/// Dropping the guard early would disarm before the images ever landed.
///
/// Answers `None` when there is nothing to do: off the main thread, or the menu built none
/// of the items that carry an image.
pub fn lend_context_menu_icons<R: Runtime>(menu: &Menu<R>, info: &FileContextInfo) -> Option<IconLoan> {
    let Some(mtm) = MainThreadMarker::new() else {
        log::warn!(target: "menu", "Not on the main thread; the context menu's items show no images");
        return None;
    };
    let armed: Vec<ArmedRun> = image_runs(info)
        .into_iter()
        .filter_map(|run| arm(mtm, menu, run))
        .collect();
    if armed.is_empty() {
        return None;
    }
    ensure_observing(mtm);
    ARMED.with(|slot| *slot.borrow_mut() = armed);
    Some(IconLoan(mtm))
}

/// Resolves a run's IDs to the titles AppKit will show, and makes its images.
///
/// `None` when the menu didn't build the run, or built only part of it: a partial run can't
/// be matched as one, and guessing which part is there would be the single-title match this
/// replaced.
fn arm<R: Runtime>(mtm: MainThreadMarker, menu: &Menu<R>, run: ImageRun) -> Option<ArmedRun> {
    let (submenu_title, titles) = match run.host {
        RunHost::Menu => (None, live_titles(&run, |id| menu.get(id))?),
        RunHost::Submenu(submenu_id) => {
            let submenu = menu.get(submenu_id)?.as_submenu()?.clone();
            let title = submenu.text().ok()?;
            (Some(title), live_titles(&run, |id| submenu.get(id))?)
        }
    };
    let images = run
        .items
        .iter()
        .map(|(_, image)| image.as_ref().and_then(|image| render(mtm, image)))
        .collect();
    Some(ArmedRun {
        submenu_title,
        titles,
        images,
    })
}

/// The live title of every item in `run`, or `None` if any of them isn't built.
fn live_titles<R: Runtime>(run: &ImageRun, get: impl Fn(&str) -> Option<MenuItemKind<R>>) -> Option<Vec<String>> {
    run.items
        .iter()
        .map(|(id, _)| menu_item_text(&get(id.as_str())?))
        .collect()
}

/// Registers the tracking observer, once per process.
fn ensure_observing(mtm: MainThreadMarker) {
    if OBSERVING.replace(true) {
        return;
    }
    observe_menu_tracking(mtm, "put images on the right-click menu", |mtm, note| {
        on_menu_did_begin_tracking(mtm, note);
    });
}

/// Puts the armed images on the tracking menu's items.
///
/// Fires for every menu the app tracks (the menu bar included), so it does nothing
/// unless images are armed. It acts on a ROOT menu only: the runs in "Open with" and
/// "Share" are reached through the root, and a submenu posting its own notification when it
/// opens would otherwise be matched against the root's runs.
fn on_menu_did_begin_tracking(mtm: MainThreadMarker, notification: &NSNotification) {
    let armed = ARMED.with(|slot| slot.borrow().clone());
    if armed.is_empty() {
        return;
    }
    let Some(menu) = tracking_menu(mtm, notification) else {
        return;
    };
    // SAFETY: `supermenu` is unsafe only because the reference is unretained; it is only
    // tested for presence here, inside this synchronous main-thread call.
    if unsafe { menu.supermenu() }.is_some() {
        return;
    }
    for run in &armed {
        match &run.submenu_title {
            None => apply(&menu, run),
            Some(title) => {
                if let Some(submenu) = find_ns_submenu(&menu, title) {
                    apply(&submenu, run);
                }
            }
        }
    }
}

/// Sets a run's images on its items in `menu`, if `menu` holds the whole run.
///
/// A title that isn't there costs the run its images and nothing else, which is the same
/// bargain the menu bar's pass makes.
fn apply(menu: &NSMenu, run: &ArmedRun) {
    let items: Vec<Retained<NSMenuItem>> = (0..menu.numberOfItems())
        .filter_map(|index| menu.itemAtIndex(index))
        .collect();
    let titles: Vec<String> = items.iter().map(|item| item.title().to_string()).collect();
    let Some(start) = find_title_run(&titles, &run.titles) else {
        log::debug!(target: "menu", "No run of {} items titled {:?} here, so they show no images", run.titles.len(), run.titles.first());
        return;
    };
    for (item, image) in items[start..].iter().zip(&run.images) {
        let Some(image) = image else {
            continue;
        };
        // A view draws this item (the tag row), and AppKit would still reserve the image
        // column for its image, pushing every title in the menu right. `tag_row` clears the
        // image when it installs; this keeps it cleared whichever observer runs first.
        if item.view().is_some() {
            continue;
        }
        set_menu_item_image(item, image);
    }
}

/// Makes the `NSImage` an item shows. `None` costs that item its image and nothing else.
fn render(mtm: MainThreadMarker, image: &ItemImage) -> Option<Retained<NSImage>> {
    match image {
        ItemImage::Symbol(symbol) => sf_symbol_image(symbol),
        ItemImage::Logo(logo) => logo_image(logo),
        ItemImage::AppIcon(icon) => {
            let png = crate::icons::rgba_to_png(&icon.rgba, icon.width, icon.height)?;
            data_image(&png, IMAGE_SIDE_POINTS)
        }
        ItemImage::TagCircle { color, applied } => data_image(
            super::tag_icons::tag_circle_png(*color, *applied)?,
            super::tag_icons::SIDE_POINTS,
        ),
        ItemImage::ShareService(index) => {
            // A copy, because sizing it would resize macOS's own image wherever else it's shown.
            let image = crate::file_system::share::offered_image(mtm, *index)?.copy();
            image.setSize(NSSize::new(IMAGE_SIDE_POINTS, IMAGE_SIDE_POINTS));
            Some(image)
        }
    }
}

/// A provider's logo, sized like the SF Symbols beside it.
///
/// No version gate, unlike `sf_symbol_image`: `initWithData:` is as old as `NSImage`, so an
/// OS whose image loader can't read SVG answers nil rather than raising, and nil costs the
/// logo and nothing else. Every logo loads as `_NSSVGImageRep` on macOS 26.6 (Swift probe,
/// 2026-09-12); older releases are unverified.
fn logo_image(logo: &ProviderLogo) -> Option<Retained<NSImage>> {
    let image = data_image(logo.svg, IMAGE_SIDE_POINTS);
    if image.is_none() {
        log::debug!(target: "menu", "NSImage can't read {}'s SVG logo here, so its actions show none", logo.provider);
    }
    image
}

/// An `NSImage` read from encoded bytes (SVG or PNG), sized `side` × `side` points. Every
/// image fed here is square, so sizing it to a square can't skew one.
fn data_image(bytes: &[u8], side: f64) -> Option<Retained<NSImage>> {
    let data = NSData::with_bytes(bytes);
    let image = NSImage::initWithData(NSImage::alloc(), &data)?;
    image.setSize(NSSize::new(side, side));
    Some(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_system::file_provider_actions::{OfferedAction, ProviderOffer};
    use crate::file_system::open_with::AppCandidate;
    use crate::file_system::share::ShareService;
    use std::collections::HashSet;
    use std::path::PathBuf;

    /// Every ID here is one the file context menu actually builds.
    ///
    /// The icons land through AppKit, which resolves a Tauri ID to the title it
    /// currently carries and matches on that, so a stale ID costs an icon with no
    /// crash and no log line anyone reads. Building a real menu needs AppKit on the main
    /// thread, so the source is what we can check here.
    #[test]
    fn every_icon_names_an_item_the_context_menu_builds() {
        let source = include_str!("file_context_menu.rs");
        let ids: HashSet<&str> = FILE_CONTEXT_ICONS.iter().map(|&(id, _)| id).collect();
        for id in ids {
            // The constant's NAME, since that's what the builder call spells.
            let name = constant_named(id).expect("every context-menu icon id is a `command_map.rs` constant");
            assert!(
                source.contains(&name),
                "`file_context_menu.rs` builds no item with `{name}`, so its icon never lands"
            );
        }
    }

    /// The two submenus a run can sit in are built with the IDs the runs name. `Submenu::new`
    /// would mint a random one, and `arm` would find no host and drop the run silently.
    #[test]
    fn the_submenus_holding_runs_are_built_with_their_ids() {
        assert!(include_str!("open_with.rs").contains("Submenu::with_id(app, OPEN_WITH_SUBMENU_ID"));
        assert!(include_str!("share_submenu.rs").contains("Submenu::with_id(app, SHARE_SUBMENU_ID"));
    }

    /// A symbol name is a string AppKit looks up at runtime, so a typo is silent. Pin
    /// the ones we ship so a rename has to be deliberate. The logos are pinned in
    /// `provider_logos.rs`.
    #[test]
    fn the_symbols_are_the_ones_we_chose() {
        assert_eq!(
            FILE_CONTEXT_ICONS,
            &[
                (DRIVE_OPEN_ID, "arrow.up.forward.app"),
                (DRIVE_COPY_LINK_ID, "link"),
                (DRIVE_ASK_GEMINI_ID, "sparkles"),
            ]
        );
    }

    fn offer(provider_id: &str, actions: usize) -> ProviderOffer {
        ProviderOffer {
            provider_domain_id: format!("{provider_id}/domain"),
            provider_id: provider_id.to_string(),
            item_identifiers: vec!["item".to_string()],
            actions: (0..actions)
                .map(|index| OfferedAction {
                    identifier: format!("action.{index}"),
                    label: format!("Action {index}"),
                })
                .collect(),
        }
    }

    fn app(bundle_id: &str, with_icon: bool) -> AppCandidate {
        AppCandidate {
            bundle_id: bundle_id.to_string(),
            display_name: bundle_id.to_string(),
            app_path: PathBuf::from(format!("/Applications/{bundle_id}.app")),
            icon: with_icon.then(|| AppIcon {
                rgba: vec![0; 4],
                width: 1,
                height: 1,
            }),
        }
    }

    fn service(title: &str) -> ShareService {
        ShareService {
            title: title.to_string(),
        }
    }

    /// One run as text: where it sits, then `ID = what it shows` per item.
    fn described(runs: Vec<ImageRun>) -> Vec<String> {
        runs.into_iter()
            .map(|run| {
                let host = match run.host {
                    RunHost::Menu => "menu".to_string(),
                    RunHost::Submenu(id) => format!("submenu {id}"),
                };
                let items: Vec<String> = run
                    .items
                    .into_iter()
                    .map(|(id, image)| {
                        let shown = match image {
                            None => "nothing".to_string(),
                            Some(ItemImage::Symbol(symbol)) => format!("symbol {symbol}"),
                            Some(ItemImage::Logo(logo)) => format!("{} logo", logo.provider),
                            Some(ItemImage::AppIcon(_)) => "app icon".to_string(),
                            Some(ItemImage::TagCircle { color, applied }) => {
                                format!("circle {color}{}", if applied { " checked" } else { "" })
                            }
                            Some(ItemImage::ShareService(index)) => format!("service {index}'s icon"),
                        };
                        format!("{id} = {shown}")
                    })
                    .collect();
                format!("{host}: {}", items.join(", "))
            })
            .collect()
    }

    /// The runs every menu carries: each Drive symbol on its own, and the seven tag circles.
    fn always(applied: &[u8]) -> Vec<String> {
        let mut runs = vec![
            "menu: drive_open = symbol arrow.up.forward.app".to_string(),
            "menu: drive_copy_link = symbol link".to_string(),
            "menu: drive_ask_gemini = symbol sparkles".to_string(),
        ];
        let circles: Vec<String> = SWATCHES
            .iter()
            .map(|swatch| {
                let checked = if applied.contains(&swatch.color) {
                    " checked"
                } else {
                    ""
                };
                format!("tag-color:{0} = circle {0}{checked}", swatch.color)
            })
            .collect();
        runs.push(format!("menu: {}", circles.join(", ")));
        runs
    }

    #[test]
    fn a_plain_row_carries_the_drive_symbols_and_the_tag_circles() {
        assert_eq!(described(image_runs(&FileContextInfo::default())), always(&[]));
    }

    #[test]
    fn an_applied_tag_color_gets_the_checked_circle() {
        let mut applied_tag_colors = [false; 8];
        applied_tag_colors[2] = true;
        applied_tag_colors[6] = true;
        let info = FileContextInfo {
            applied_tag_colors,
            ..FileContextInfo::default()
        };
        assert_eq!(described(image_runs(&info)), always(&[2, 6]));
    }

    /// Google Drive's own lines get Drive's logo, while Cmdr's three Drive items beside
    /// them keep their symbols.
    #[test]
    fn every_line_of_a_known_providers_offer_carries_its_logo() {
        let info = FileContextInfo {
            file_provider_offer: Some(offer("com.google.drivefs.fpext", 2)),
            ..FileContextInfo::default()
        };
        let mut expected = always(&[]);
        expected.push("menu: fp-action:0 = Google Drive logo, fp-action:1 = Google Drive logo".to_string());
        assert_eq!(described(image_runs(&info)), expected);
    }

    #[test]
    fn an_unknown_providers_lines_or_an_empty_offer_add_no_run() {
        for offer in [
            offer("com.example.fileprovider", 3),
            offer("com.getdropbox.dropbox.fileprovider", 0),
        ] {
            let info = FileContextInfo {
                file_provider_offer: Some(offer),
                ..FileContextInfo::default()
            };
            assert_eq!(described(image_runs(&info)), always(&[]));
        }
    }

    /// Every candidate stays in the run, iconless ones too, since the run is matched as
    /// one contiguous stretch of the submenu.
    #[test]
    fn open_with_is_one_run_in_its_submenu_with_a_gap_where_an_app_has_no_icon() {
        let mut info = FileContextInfo::default();
        info.open_with.candidates = vec![
            app("com.apple.Preview", true),
            app("com.example.NoIcon", false),
            app("com.apple.Safari", true),
        ];
        let mut expected = always(&[]);
        expected.push(
            "submenu open-with-submenu: open-with:com.apple.Preview = app icon, \
             open-with:com.example.NoIcon = nothing, open-with:com.apple.Safari = app icon"
                .to_string(),
        );
        assert_eq!(described(image_runs(&info)), expected);
    }

    #[test]
    fn share_is_one_run_in_its_submenu_pointing_at_each_offered_service() {
        let info = FileContextInfo {
            share_services: vec![service("AirDrop"), service("Mail")],
            ..FileContextInfo::default()
        };
        let mut expected = always(&[]);
        expected.push(
            "submenu share-submenu: share-service:0 = service 0's icon, share-service:1 = service 1's icon".to_string(),
        );
        assert_eq!(described(image_runs(&info)), expected);
    }

    fn titles(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn a_run_is_found_where_it_starts() {
        let menu = ["Open with", "Photos", "Preview", "", "Other…"];
        assert_eq!(find_title_run(&menu, &titles(&["Photos", "Preview"])), Some(1));
        assert_eq!(find_title_run(&menu, &titles(&["Other…"])), Some(4));
    }

    /// The header line carries the bare filename, so a folder named `Mail` sits above the
    /// real `Mail` item. A run of two can't start on it.
    #[test]
    fn a_lookalike_title_outside_the_run_doesnt_take_it() {
        let menu = ["Mail", "", "AirDrop", "Mail", "Messages"];
        assert_eq!(find_title_run(&menu, &titles(&["Mail", "Messages"])), Some(3));
    }

    /// Two share extensions may carry one name; inside the run they pair up by position.
    #[test]
    fn duplicate_titles_inside_a_run_match_by_position() {
        let menu = ["Notes", "Notes", "Reminders"];
        assert_eq!(
            find_title_run(&menu, &titles(&["Notes", "Notes", "Reminders"])),
            Some(0)
        );
    }

    #[test]
    fn a_partial_or_absent_run_matches_nothing() {
        let menu = ["AirDrop", "", "Mail"];
        assert_eq!(find_title_run(&menu, &titles(&["AirDrop", "Mail"])), None);
        assert_eq!(find_title_run(&menu, &titles(&["Messages"])), None);
        assert_eq!(find_title_run(&menu[..1], &titles(&["AirDrop", "Mail"])), None);
    }

    /// `display_accelerators.rs` puts the glyph after a TAB in the title AppKit reports.
    #[test]
    fn a_title_matches_without_its_displayed_shortcut() {
        let menu = ["Invert selection\t⇧8", "Select files…\t+"];
        assert_eq!(
            find_title_run(&menu, &titles(&["Invert selection", "Select files…"])),
            Some(0)
        );
    }

    /// A separator's title is empty, so an empty title in a run would match one.
    #[test]
    fn an_empty_run_or_an_empty_title_matches_nothing() {
        let menu = ["AirDrop", "", "Mail"];
        assert_eq!(find_title_run(&menu, &[]), None);
        assert_eq!(find_title_run(&menu, &titles(&[""])), None);
        assert_eq!(find_title_run(&menu, &titles(&["AirDrop", ""])), None);
    }

    /// The `command_map.rs` constant whose value is `value`.
    fn constant_named(value: &str) -> Option<String> {
        include_str!("command_map.rs").lines().find_map(|line| {
            let rest = line.trim().strip_prefix("pub const ")?;
            let (name, rest) = rest.split_once(": &str = \"")?;
            (rest.strip_suffix("\";")? == value).then(|| name.to_string())
        })
    }
}
