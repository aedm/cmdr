//! Icons on right-click menu items (macOS): SF Symbols on Cmdr's own items, and each
//! File Provider's logo on that provider's actions.
//!
//! The menu BAR gets its icons at build time (`macos_appkit.rs`'s `MENU_BAR_ICONS`),
//! because Tauri hands out the installed bar's `NSMenu`. A context menu's it does not
//! (muda's `ns_menu()` sits behind Tauri's sealed `ContextMenuBase`), so this reaches
//! the items through `NSMenuDidBeginTrackingNotification`, exactly as
//! `services_context.rs` does and for exactly the same reason. AppKit posts it before
//! the menu is laid out, which is why an image set here still gets its gutter.
//!
//! ## Why not `IconMenuItem`, which needs no AppKit at all
//!
//! Because it can't render a template image. muda turns the RGBA it's given into a PNG
//! and hands `NSImage` that, never calling `setTemplate:`, so the bitmap draws as
//! literal pixels. A monochrome glyph then stays whatever colour it was baked in: it
//! disappears in the mode it wasn't baked for, and it stays dark on the accent-coloured
//! fill of a highlighted row while the label beside it turns white. `NSMenuItem`'s own
//! `setImage:` with a real symbol image is the whole fix: AppKit tints it for light,
//! dark, and highlight, and it tracks the menu's font size.
//!
//! `IconMenuItem` stays right for the icons that ARE pixels (app icons in "Open with",
//! `NSSharingService` icons in `Share`, the tag colour circles), which is why those
//! don't come through here.
//!
//! ## Provider logos come through here too
//!
//! A logo is colored, so tinting isn't the reason; the format is. The logos are SVG
//! (`provider_logos.rs`), and `IconMenuItem` takes RGBA only, so they'd have to be
//! rasterized at a guessed scale first. An `NSImage` read from the SVG bytes stays a
//! vector, sharp on any display. It's deliberately NOT a template image: the brand's
//! colors are the point, so a highlighted row keeps them too.

use std::cell::{Cell, RefCell};

use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{NSImage, NSMenu, NSMenuItem};
use objc2_foundation::{NSData, NSNotification, NSSize};
use tauri::Runtime;
use tauri::menu::Menu;

use super::file_provider_items::file_provider_action_id;
use super::macos_appkit::{find_ns_item, menu_item_text, observe_menu_tracking, set_sf_symbol, tracking_menu};
use super::provider_logos::{ProviderLogo, logo_for_provider};
use super::{DRIVE_ASK_GEMINI_ID, DRIVE_COPY_LINK_ID, DRIVE_OPEN_ID};
use crate::file_system::file_provider_actions::ProviderOffer;

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
/// their provider's logo instead ([`icons_for`]), which only says whose action it is.
const FILE_CONTEXT_ICONS: &[(&str, &str)] = &[
    (DRIVE_OPEN_ID, "arrow.up.forward.app"),
    (DRIVE_COPY_LINK_ID, "link"),
    (DRIVE_ASK_GEMINI_ID, "sparkles"),
];

/// The side of a provider logo, in points: the box the SF Symbols beside it take.
///
/// At the menu's 13 pt font, `link` is 17 × 17, `sparkles` 15 × 17, and
/// `arrow.up.forward.app` 15 × 14 (verified on macOS 26.6, `NSImage.size` from a Swift
/// probe, 2026-09-12).
const LOGO_SIDE_POINTS: f64 = 16.0;

/// What an armed item shows.
#[derive(Clone, Copy)]
enum ItemIcon {
    /// An SF Symbol, which AppKit tints for light, dark, and highlight.
    Symbol(&'static str),
    /// A provider's colored logo.
    Logo(&'static ProviderLogo),
}

/// Icons armed for the next context menu to open.
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
    /// `(title, icon)` for the menu currently going up. Main-thread-only by
    /// construction: every reader runs from the menu thread.
    static ARMED: RefCell<Vec<(String, ItemIcon)>> = const { RefCell::new(Vec::new()) };
    /// Whether the tracking observer is registered. Registering it lazily keeps the
    /// cost with the first right-click instead of every launch.
    static OBSERVING: Cell<bool> = const { Cell::new(false) };
}

/// Arms the file context menu's icons for `menu`, which must be about to `popup()`:
/// [`FILE_CONTEXT_ICONS`], plus the provider's logo on each line of `provider_offer`.
///
/// ❗ Hold the returned guard until `popup()` returns, the way `ServicesLoan` is held:
/// `popup()` runs AppKit's tracking loop, so the menu only starts tracking inside it.
/// Dropping the guard early would disarm before the icons ever landed.
///
/// Answers `None` when there is nothing to do: off the main thread, or the menu built
/// none of the items with icons (rows that are neither Drive items nor a known provider's,
/// which is most of them).
pub fn lend_context_menu_icons<R: Runtime>(menu: &Menu<R>, provider_offer: Option<&ProviderOffer>) -> Option<IconLoan> {
    let Some(mtm) = MainThreadMarker::new() else {
        log::warn!(target: "menu", "Not on the main thread; the context menu's items show no icons");
        return None;
    };
    let armed: Vec<(String, ItemIcon)> = icons_for(provider_offer)
        .into_iter()
        .filter_map(|(id, icon)| Some((menu_item_text(&menu.get(id.as_str())?)?, icon)))
        .collect();
    if armed.is_empty() {
        return None;
    }
    ensure_observing(mtm);
    ARMED.with(|slot| *slot.borrow_mut() = armed);
    Some(IconLoan(mtm))
}

/// `(menu item ID, icon)` for every item the file context menu can carry an icon on.
///
/// Which of them the menu actually built is the caller's question: the Drive items exist
/// only for a Drive row, and an offer's lines only when File Provider vouched for them.
/// An offer from a provider with no logo adds nothing.
fn icons_for(provider_offer: Option<&ProviderOffer>) -> Vec<(String, ItemIcon)> {
    let symbols = FILE_CONTEXT_ICONS
        .iter()
        .map(|&(id, symbol)| (id.to_string(), ItemIcon::Symbol(symbol)));
    let logos = provider_offer
        .and_then(|offer| Some((offer.actions.len(), logo_for_provider(&offer.provider_id)?)))
        .into_iter()
        .flat_map(|(count, logo)| (0..count).map(move |index| (file_provider_action_id(index), ItemIcon::Logo(logo))));
    symbols.chain(logos).collect()
}

/// Registers the tracking observer, once per process.
fn ensure_observing(mtm: MainThreadMarker) {
    if OBSERVING.replace(true) {
        return;
    }
    observe_menu_tracking(mtm, "put icons on the right-click menu", |mtm, note| {
        on_menu_did_begin_tracking(mtm, note);
    });
}

/// Puts the armed icons on the tracking menu's items.
///
/// Fires for every menu the app tracks (the menu bar included), so it does nothing
/// unless icons are armed and the tracking menu carries the titles they name. A title
/// that isn't there costs an icon and nothing else, which is the same bargain the menu
/// bar's pass makes.
fn on_menu_did_begin_tracking(mtm: MainThreadMarker, notification: &NSNotification) {
    let armed = ARMED.with(|slot| slot.borrow().clone());
    if armed.is_empty() {
        return;
    }
    let Some(menu) = tracking_menu(mtm, notification) else {
        return;
    };
    apply(&menu, &armed);
}

/// Sets each armed icon on the item carrying its title, if this menu has one.
fn apply(menu: &NSMenu, armed: &[(String, ItemIcon)]) {
    for (title, icon) in armed {
        let Some(item) = find_ns_item(menu, title) else {
            continue;
        };
        match icon {
            ItemIcon::Symbol(symbol) => set_sf_symbol(&item, symbol),
            ItemIcon::Logo(logo) => set_logo(&item, logo),
        }
    }
}

/// Puts a provider's logo on a menu item, sized like the SF Symbols beside it.
///
/// No version gate, unlike `set_sf_symbol`: `initWithData:` is as old as `NSImage`, so an
/// OS whose image loader can't read SVG answers nil rather than raising, and nil costs the
/// logo and nothing else. Every logo loads as `_NSSVGImageRep` on macOS 26.6 (Swift probe,
/// 2026-09-12); older releases are unverified.
fn set_logo(item: &NSMenuItem, logo: &ProviderLogo) {
    let data = NSData::with_bytes(logo.svg);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        log::debug!(target: "menu", "NSImage can't read {}'s SVG logo here, so its actions show none", logo.provider);
        return;
    };
    image.setSize(NSSize::new(LOGO_SIDE_POINTS, LOGO_SIDE_POINTS));
    item.setImage(Some(&image));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_system::file_provider_actions::OfferedAction;
    use std::collections::HashSet;

    /// Every ID here is one the file context menu actually builds.
    ///
    /// The icons land through AppKit, which resolves a Tauri ID to the title it
    /// currently carries and matches on that, so a stale ID costs an icon with no
    /// crash and no log line anyone reads. Building a real menu needs AppKit on the main
    /// thread, so the source is what we can check here.
    #[test]
    fn every_icon_names_an_item_the_context_menu_builds() {
        let source = include_str!("menu_structure.rs");
        let ids: HashSet<&str> = FILE_CONTEXT_ICONS.iter().map(|&(id, _)| id).collect();
        for id in ids {
            // The constant's NAME, since that's what the builder call spells.
            let name = constant_named(id).expect("every context-menu icon id is a `command_map.rs` constant");
            assert!(
                source.contains(&name),
                "`menu_structure.rs` builds no item with `{name}`, so its icon never lands"
            );
        }
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

    /// `(ID, what it shows)`, a logo named by its provider.
    fn described(icons: Vec<(String, ItemIcon)>) -> Vec<(String, String)> {
        icons
            .into_iter()
            .map(|(id, icon)| {
                let shown = match icon {
                    ItemIcon::Symbol(symbol) => format!("symbol {symbol}"),
                    ItemIcon::Logo(logo) => format!("{} logo", logo.provider),
                };
                (id, shown)
            })
            .collect()
    }

    fn cmdrs_own() -> Vec<(String, String)> {
        [
            (DRIVE_OPEN_ID, "symbol arrow.up.forward.app"),
            (DRIVE_COPY_LINK_ID, "symbol link"),
            (DRIVE_ASK_GEMINI_ID, "symbol sparkles"),
        ]
        .iter()
        .map(|&(id, shown)| (id.to_string(), shown.to_string()))
        .collect()
    }

    #[test]
    fn without_an_offer_only_cmdrs_own_items_carry_icons() {
        assert_eq!(described(icons_for(None)), cmdrs_own());
    }

    /// Google Drive's own lines get Drive's logo, while Cmdr's three Drive items beside
    /// them keep their symbols.
    #[test]
    fn every_line_of_a_known_providers_offer_carries_its_logo() {
        let mut expected = cmdrs_own();
        expected.extend([
            ("fp-action:0".to_string(), "Google Drive logo".to_string()),
            ("fp-action:1".to_string(), "Google Drive logo".to_string()),
        ]);
        assert_eq!(
            described(icons_for(Some(&offer("com.google.drivefs.fpext", 2)))),
            expected
        );
    }

    #[test]
    fn an_unknown_providers_lines_or_an_empty_offer_add_no_icons() {
        assert_eq!(
            described(icons_for(Some(&offer("com.example.fileprovider", 3)))),
            cmdrs_own()
        );
        assert_eq!(
            described(icons_for(Some(&offer("com.getdropbox.dropbox.fileprovider", 0)))),
            cmdrs_own()
        );
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
