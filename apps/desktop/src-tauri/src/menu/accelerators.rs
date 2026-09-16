//! Keyboard accelerators: converting the frontend's shortcut strings into Tauri
//! accelerator strings, and swapping the accelerator on a live menu item.
//!
//! The conversion is the seam between two vocabularies. The frontend speaks macOS
//! glyphs and canonical key words (`⌘⇧P`, `Backspace`); Tauri wants `Cmd+Shift+P`.
//! Anything unrecognized passes through as-is, so a glyph that leaks in here
//! produces a garbage accelerator rather than an error, which is what the
//! canonical-key-names test guards.
//!
//! Two ways out, because a menu BAR accelerator is registered with the app and a
//! POPUP's is only drawn: [`frontend_shortcut_to_menu_text`] converts,
//! [`frontend_shortcut_to_accelerator`] converts and then refuses anything the bar
//! can't safely own.

use tauri::{AppHandle, Runtime, menu::MenuItem};

use crate::ignore_poison::IgnorePoison;

use super::menu_spec::{Platform, display_accelerator_label};
use super::{MenuItemEntry, MenuState};

/// Convert frontend shortcut format (⌘2) to the text a menu item shows (Cmd+2).
/// Returns None if the shortcut is empty.
///
/// No floor of any kind: a popup menu's key equivalents are never registered with the
/// app, so a bare `Space` or `F5` there is display text and can't swallow anything.
/// ❌ The menu bar must not use this — [`frontend_shortcut_to_accelerator`] is its door.
pub fn frontend_shortcut_to_menu_text(shortcut: &str) -> Option<String> {
    convert(shortcut).map(|(accelerator, _)| accelerator)
}

/// Convert frontend shortcut format (⌘2) to a Tauri accelerator the menu BAR may
/// register (Cmd+2). Returns None if the shortcut is empty, carries no ⌘ / ⌃ / ⌥, or names a
/// key muda has no `Code` for.
///
/// ❗ The modifier floor is what keeps a bare key out of the menu bar. AppKit fires a
/// registered accelerator app-wide, ahead of the webview, so a menu item bound to `*`
/// or `+` would eat that character in every text field in the app. Shift alone doesn't
/// clear the floor either: `⇧8` IS `*`.
///
/// ❗ The second refusal is [`is_codeable_key`], and it exists because the failure is SILENT:
/// Tauri discards a parse error and builds the item with no accelerator at all, so a string muda
/// can't read is indistinguishable from no shortcut until someone opens the menu and looks.
///
/// A combo refused for either reason isn't lost — it becomes a display-only accelerator
/// (`menu_spec::ItemSpec::display_accelerator`), drawn beside the item while the file pane's
/// keydown handler does the work.
pub fn frontend_shortcut_to_accelerator(shortcut: &str) -> Option<String> {
    match convert(shortcut)? {
        (accelerator, true) => Some(accelerator),
        (_, false) => None,
    }
}

/// The punctuation muda names a physical key for, verbatim from `parse_code`
/// (`muda-0.19.3/src/accelerator.rs:151`). ASCII letters and digits are the rest of it.
const CODEABLE_PUNCTUATION: &str = "`\\[],=-./';";

/// Whether muda can turn this key into a `Code`, which decides whether an accelerator naming
/// it can be REGISTERED at all.
///
/// ❗ The SHIFTED characters are the gap: `+`, `*`, `(`, `_` and friends have no `Code` of their
/// own, only the unshifted key they sit on. So `⌘+` can't be a real accelerator however it's
/// spelled — Tauri would discard the parse error and build the item with no key, silently. It
/// routes to the display path instead, exactly like a bare key, which is honest: the glyph shows
/// and the frontend's keydown dispatch runs the command.
///
/// ❌ Don't "fix" this by emitting `Cmd+Shift+Equal` for `⌘+`: that's a guess about the user's
/// layout, and the physical key that types `+` moves between them.
fn is_codeable_key(key: &str) -> bool {
    let mut chars = key.chars();
    let (Some(only), None) = (chars.next(), chars.next()) else {
        // A word form that reached here fell through every named-key arm above, so muda has no
        // name for it either.
        return false;
    };
    only.is_ascii_alphanumeric() || CODEABLE_PUNCTUATION.contains(only)
}

/// The converted accelerator, plus whether the menu bar may register it: it needs a command
/// modifier (⌘, ⌃, or ⌥) AND a key muda can name.
fn convert(shortcut: &str) -> Option<(String, bool)> {
    if shortcut.is_empty() {
        return None;
    }

    let mut result = String::new();
    let mut registerable = false;
    let mut chars = shortcut.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '⌘' => {
                registerable = true;
                if !result.is_empty() {
                    result.push('+');
                }
                result.push_str("Cmd");
            }
            '⌃' => {
                registerable = true;
                if !result.is_empty() {
                    result.push('+');
                }
                result.push_str("Ctrl");
            }
            '⌥' => {
                registerable = true;
                if !result.is_empty() {
                    result.push('+');
                }
                // ❗ `Alt`, ❌ never `Opt`: muda accepts `OPTION` and `ALT` and nothing else
                // (`muda-0.19.3/src/accelerator.rs:534`), and Tauri throws the parse error away
                // (`normal.rs:65` is `.parse().ok()`), so `Opt` built items with NO accelerator at
                // all, silently. `an_accelerator_muda_refuses_is_dropped_without_a_word` guards it.
                result.push_str("Alt");
            }
            '⇧' => {
                if !result.is_empty() {
                    result.push('+');
                }
                result.push_str("Shift");
            }
            '↑' => {
                if !result.is_empty() {
                    result.push('+');
                }
                result.push_str("Up");
            }
            '↓' => {
                if !result.is_empty() {
                    result.push('+');
                }
                result.push_str("Down");
            }
            '←' => {
                if !result.is_empty() {
                    result.push('+');
                }
                result.push_str("Left");
            }
            '→' => {
                if !result.is_empty() {
                    result.push('+');
                }
                result.push_str("Right");
            }
            _ => {
                // Regular character (letter, number, etc.)
                if !result.is_empty() {
                    result.push('+');
                }
                // Handle special key names
                let remaining: String = std::iter::once(c).chain(chars.by_ref()).collect();
                if remaining.eq_ignore_ascii_case("enter") {
                    result.push_str("Enter");
                } else if remaining.eq_ignore_ascii_case("space") {
                    result.push_str("Space");
                } else if remaining.eq_ignore_ascii_case("tab") {
                    result.push_str("Tab");
                } else if remaining.eq_ignore_ascii_case("escape") {
                    result.push_str("Escape");
                } else if remaining.eq_ignore_ascii_case("backspace") {
                    result.push_str("Backspace");
                } else if remaining.eq_ignore_ascii_case("delete") {
                    result.push_str("Delete");
                } else if remaining.eq_ignore_ascii_case("insert") {
                    result.push_str("Insert");
                } else if remaining.starts_with('F') || remaining.starts_with('f') {
                    // Function keys like F1, F4
                    result.push_str(&remaining.to_uppercase());
                } else if remaining.eq_ignore_ascii_case("pageup") {
                    result.push_str("PageUp");
                } else if remaining.eq_ignore_ascii_case("pagedown") {
                    result.push_str("PageDown");
                } else if remaining.eq_ignore_ascii_case("home") {
                    result.push_str("Home");
                } else if remaining.eq_ignore_ascii_case("end") {
                    result.push_str("End");
                } else {
                    // Single character or unknown - use as-is (uppercase for letters)
                    if !is_codeable_key(&remaining) {
                        registerable = false;
                    }
                    result.push_str(&remaining.to_uppercase());
                }
                break;
            }
        }
    }

    if result.is_empty() {
        None
    } else {
        Some((result, registerable))
    }
}

/// Update the accelerator for any menu item tracked in the items HashMap.
/// Removes the old item, creates a new one with the same ID/label/enabled state
/// but a new accelerator, and reinserts at the same position.
///
/// `display_accelerator` is the combo to SHOW when `new_accelerator` is `None` because the
/// modifier floor refused it. It rebuilds the label from `MenuItemEntry::label` rather than
/// from the live item, so a rebind can't stack a second `(⇧8)` onto a Linux label.
///
/// ❗ On macOS the fresh `NSMenuItem` carries no attributed title, so the caller must re-run
/// `macos_appkit::set_display_accelerators` afterwards or the glyph disappears until the next
/// menu-bar swap. Same contract as the SF Symbols.
pub fn update_menu_item_accelerator<R: Runtime>(
    app: &AppHandle<R>,
    menu_state: &MenuState<R>,
    menu_item_id: &str,
    new_accelerator: Option<&str>,
    display_accelerator: Option<&str>,
) -> tauri::Result<()> {
    let mut items_guard = menu_state.items.lock_ignore_poison();
    let entry = items_guard
        .get(menu_item_id)
        .ok_or_else(|| tauri::Error::InvalidWindowHandle)?;

    let label = entry.label.clone();
    let built_label = match display_accelerator {
        Some(shortcut) => display_accelerator_label(&label, shortcut, Platform::current()),
        None => label.clone(),
    };
    let enabled = entry.item.is_enabled()?;
    let submenu = entry.submenu.clone();
    let position = entry.position;

    // Remove old item, create replacement with new accelerator, reinsert
    submenu.remove(&entry.item)?;
    let new_item = MenuItem::with_id(app, menu_item_id, &built_label, enabled, new_accelerator)?;
    submenu.insert(&new_item, position)?;

    // Update the HashMap entry
    items_guard.insert(
        menu_item_id.to_string(),
        MenuItemEntry {
            item: new_item,
            submenu,
            position,
            label,
        },
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frontend_shortcut_to_accelerator_simple() {
        // Basic modifier + key combinations
        assert_eq!(frontend_shortcut_to_accelerator("⌘1"), Some("Cmd+1".to_string()));
        assert_eq!(frontend_shortcut_to_accelerator("⌘2"), Some("Cmd+2".to_string()));
        assert_eq!(frontend_shortcut_to_accelerator("⌘⇧P"), Some("Cmd+Shift+P".to_string()));
        assert_eq!(frontend_shortcut_to_accelerator("⌥⌘O"), Some("Alt+Cmd+O".to_string()));
        assert_eq!(frontend_shortcut_to_accelerator("⌃⌘C"), Some("Ctrl+Cmd+C".to_string()));
    }

    #[test]
    fn test_frontend_shortcut_to_accelerator_arrows() {
        assert_eq!(frontend_shortcut_to_accelerator("⌘↑"), Some("Cmd+Up".to_string()));
        assert_eq!(frontend_shortcut_to_accelerator("⌘↓"), Some("Cmd+Down".to_string()));
        assert_eq!(frontend_shortcut_to_accelerator("⌘["), Some("Cmd+[".to_string()));
        assert_eq!(frontend_shortcut_to_accelerator("⌘]"), Some("Cmd+]".to_string()));
    }

    /// The key words a popup menu spells out. Every one of them is bare, so the menu
    /// bar refuses them and only the popup path sees them.
    #[test]
    fn test_frontend_shortcut_to_menu_text_special_keys() {
        assert_eq!(frontend_shortcut_to_menu_text("Tab"), Some("Tab".to_string()));
        assert_eq!(frontend_shortcut_to_menu_text("Enter"), Some("Enter".to_string()));
        assert_eq!(frontend_shortcut_to_menu_text("Space"), Some("Space".to_string()));
        assert_eq!(frontend_shortcut_to_menu_text("F4"), Some("F4".to_string()));
        assert_eq!(
            frontend_shortcut_to_menu_text("Backspace"),
            Some("Backspace".to_string())
        );
        assert_eq!(frontend_shortcut_to_menu_text("Delete"), Some("Delete".to_string()));
        assert_eq!(frontend_shortcut_to_menu_text("Insert"), Some("Insert".to_string()));
        assert_eq!(frontend_shortcut_to_menu_text("PageUp"), Some("PageUp".to_string()));
        assert_eq!(frontend_shortcut_to_menu_text("Escape"), Some("Escape".to_string()));
    }

    /// ❗ The modifier floor. AppKit fires a menu-bar accelerator app-wide, ahead of the
    /// webview, so a bare `*` / `+` / `-` registered here would be swallowed in every
    /// text field in the app. Shift doesn't clear the floor: `⇧8` IS `*` on a US layout.
    #[test]
    fn a_combo_with_no_command_modifier_is_no_menu_bar_accelerator() {
        for bare in ["*", "+", "-", "⇧8", "⇧-", "Space", "Tab", "F4", "Backspace", "↑"] {
            assert_eq!(
                frontend_shortcut_to_accelerator(bare),
                None,
                "`{bare}` carries no ⌘ / ⌃ / ⌥, so the menu bar must not register it"
            );
            assert!(
                frontend_shortcut_to_menu_text(bare).is_some(),
                "`{bare}` is still fine as menu TEXT, which is what a popup shows"
            );
        }
    }

    /// Any one of the three clears it, and the two paths then agree.
    #[test]
    fn one_command_modifier_is_enough_to_register() {
        for combo in ["⌘K", "⌃Enter", "⌥⇧=", "⇧⌘A"] {
            assert_eq!(
                frontend_shortcut_to_accelerator(combo),
                frontend_shortcut_to_menu_text(combo),
                "`{combo}` carries a command modifier, so both paths spell it the same way"
            );
            assert!(frontend_shortcut_to_accelerator(combo).is_some(), "`{combo}`");
        }
    }

    /// The SECOND refusal: a command modifier isn't enough if the key itself has no muda `Code`.
    ///
    /// Every shifted character is in this hole — muda names the physical key, and `+` doesn't have
    /// one. `view.zoom.in` ships `⌘+`, so this is a combo the app really produces. Registering it
    /// is impossible however it's spelled, and the silent failure (Tauri discards the parse error)
    /// is why refusing beats emitting something that looks fine and does nothing.
    #[test]
    fn a_key_muda_cannot_name_is_no_menu_bar_accelerator_either() {
        for uncodeable in ["⌘+", "⌘*", "⌥(", "⌃_"] {
            assert_eq!(
                frontend_shortcut_to_accelerator(uncodeable),
                None,
                "`{uncodeable}` names a key muda has no `Code` for, so registering it would \
                 silently produce a menu item with no accelerator"
            );
        }
        // The unshifted keys those characters sit on are fine, which is what makes the hole
        // specific rather than a blanket refusal of punctuation.
        for codeable in ["⌘=", "⌘-", "⌘[", "⌘,", "⌘.", "⌘/", "⌘;", "⌘'", "⌘`"] {
            assert!(
                frontend_shortcut_to_accelerator(codeable).is_some(),
                "`{codeable}` sits on a key muda names, so it registers"
            );
        }
    }

    /// The frontend only ever hands us its canonical word forms; the macOS glyphs
    /// (⌫ ⎋ ↩) are a display concern that must never reach an accelerator, since
    /// they'd fall through to the "unknown, use as-is" branch and produce garbage
    /// like `Cmd+⌫`.
    #[test]
    fn test_frontend_shortcut_to_accelerator_canonical_key_names() {
        assert_eq!(
            frontend_shortcut_to_accelerator("⌘Backspace"),
            Some("Cmd+Backspace".to_string())
        );
        assert_eq!(
            frontend_shortcut_to_accelerator("⌘⌥Escape"),
            Some("Cmd+Alt+Escape".to_string())
        );
    }

    #[test]
    fn test_frontend_shortcut_to_accelerator_empty() {
        assert_eq!(frontend_shortcut_to_accelerator(""), None);
        assert_eq!(frontend_shortcut_to_menu_text(""), None);
    }

    /// ❗ The test the other ones in this file couldn't be: it PARSES the output instead of
    /// comparing it to a string we made up.
    ///
    /// Everything downstream throws a parse failure away. `tauri::menu::MenuItem::new` is
    /// `accelerator.and_then(|s| s.as_ref().parse().ok())` (`tauri-2.11.5/src/menu/normal.rs:65`),
    /// so a token muda doesn't know builds an item with no key at all, with no error, no panic and
    /// no log line. `Opt` was exactly that: every ⌥ accelerator in the app — Copy path, Show in
    /// Finder, Open terminal here, and any ⌥ combo a user rebound to — silently had none, while a
    /// green `assert_eq!(…, "Cmd+Opt+C")` right here said it was fine.
    ///
    /// muda is a dev-dependency so this parses through the same code Tauri will.
    #[test]
    fn every_accelerator_this_converter_emits_is_one_muda_can_parse() {
        // Every modifier and every key shape the converter has a branch for, plus the shifted
        // characters that have no `Code` and the combos the registry actually ships.
        let combos = [
            "⌘K",
            "⌥⌘O",
            "⌃⌘C",
            "⌘⇧P",
            "⌃⌥⇧⌘A",
            "⌘↑",
            "⌘↓",
            "⌘←",
            "⌘→",
            "⌘[",
            "⌘]",
            "⌘,",
            "⌘=",
            "⌘-",
            "⌘+",
            "⌘*",
            "⌥⇧=",
            "⌥+",
            "⇧8",
            "+",
            "-",
            "*",
            "⌘Backspace",
            "⌘⌥Escape",
            "⌥Enter",
            "⌥Space",
            "⌥Tab",
            "⌥Delete",
            "⌥Insert",
            "⌥PageUp",
            "⌥PageDown",
            "⌥Home",
            "⌥End",
            "⌥F4",
            "",
        ];
        for combo in combos {
            // ❗ The contract, and the whole reason this test exists: whatever comes back has to
            // PARSE. `None` is a fine answer — it routes to the display path — but a `Some` that
            // muda refuses is the silent failure, because Tauri discards the parse error and
            // builds the item with no accelerator at all.
            let Some(accelerator) = frontend_shortcut_to_accelerator(combo) else {
                continue;
            };
            assert!(
                accelerator.parse::<muda::accelerator::Accelerator>().is_ok(),
                "`{combo}` converts to `{accelerator}`, which muda refuses — so Tauri would build \
                 the item with NO accelerator and say nothing"
            );
        }
    }

    /// The other half of the same guard: the menu bar's own hardcoded accelerator strings go
    /// straight to Tauri without passing through the converter, so they need parsing too.
    #[test]
    fn every_accelerator_the_menu_bar_hardcodes_is_one_muda_can_parse() {
        use crate::menu::menu_bar::MENU_BAR;
        use crate::menu::menu_spec::{EntryKind, Platform, SubmenuSpec};

        fn check(spec: &'static SubmenuSpec, platform: Platform) {
            for entry in spec.entries_on(platform) {
                let (id, accelerator) = match entry {
                    EntryKind::Item(item) => (item.id, item.accelerator.on(platform)),
                    EntryKind::Check(item) => (item.id, item.accelerator.on(platform)),
                    EntryKind::Submenu(nested) => {
                        check(nested, platform);
                        continue;
                    }
                    _ => continue,
                };
                let Some(accelerator) = accelerator else {
                    continue;
                };
                assert!(
                    accelerator.parse::<muda::accelerator::Accelerator>().is_ok(),
                    "`{id}` on {platform:?} is built with `{accelerator}`, which muda refuses, so \
                     the item comes up with no key and nothing says a word"
                );
            }
        }

        for platform in [Platform::MacOs, Platform::Linux] {
            for bar_menu in MENU_BAR.iter().filter(|bar_menu| bar_menu.is_on(platform)) {
                check(&bar_menu.submenu, platform);
            }
        }
    }
}
