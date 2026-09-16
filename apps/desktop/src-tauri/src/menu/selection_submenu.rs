//! The file context menu's `Selection >` submenu: everything the Select menu bar holds, plus
//! Toggle selection, one right-click away from the row it acts on.
//!
//! Its own file rather than another block in `file_context_menu.rs` because the ORDER of these rows
//! is otherwise unprovable. A real
//! `muda::Menu` panics off the main thread, so no unit test can build one and read it back; keeping
//! the rows as data ([`SELECTION_ROWS`]) means the test below pins what the user sees.

use tauri::{
    AppHandle, Runtime,
    menu::{PredefinedMenuItem, Submenu},
};

use crate::intl::menu_t;

use super::menu_items::{SameKindTarget, same_kind_menu_label};
use super::menu_structure::{ContextMenuShortcuts, context_item};
use super::{
    DESELECT_ALL_ID, DESELECT_FILES_ID, INVERT_SELECTION_ID, SELECT_ALL_ID, SELECT_FILES_ID, SELECT_SAME_KIND_ID,
    TOGGLE_SELECTION_ID,
};

/// One row of the Selection submenu.
pub(super) enum SelectionRow {
    /// A command item: the menu id it's built with, and the catalog key its label reads.
    Item(&'static str, &'static str),
    /// "Select all of the same kind", whose label says what THIS right-click would select.
    /// Composed at popup time, ❗ never read from the menu bar's debounced value.
    SameKind,
    Separator,
}

/// The submenu's rows, in display order.
///
/// Toggle selection leads because Space is discoverable nowhere else, and the two dialog items sit
/// below a separator because they're the only ones that ask a question before acting.
pub(super) const SELECTION_ROWS: &[SelectionRow] = &[
    SelectionRow::Item(TOGGLE_SELECTION_ID, "menu.context.toggleSelection"),
    SelectionRow::Item(SELECT_ALL_ID, "menu.select.all"),
    SelectionRow::Item(DESELECT_ALL_ID, "menu.select.deselectAll"),
    SelectionRow::SameKind,
    SelectionRow::Item(INVERT_SELECTION_ID, "menu.select.invert"),
    SelectionRow::Separator,
    SelectionRow::Item(SELECT_FILES_ID, "menu.select.files"),
    SelectionRow::Item(DESELECT_FILES_ID, "menu.select.deselectFiles"),
];

/// Builds `Selection >` for one right-click.
///
/// `same_kind` is what the right-clicked row would select, computed by the frontend as it opens the
/// menu (`pane-pointer.ts`). Every accelerator label comes from `shortcuts`, so a rebind shows up
/// here too; a popup's keys never fire, so a bare `Space` is safe as plain display text.
pub(super) fn build_selection_submenu<R: Runtime>(
    app: &AppHandle<R>,
    shortcuts: &ContextMenuShortcuts,
    same_kind: Option<&SameKindTarget>,
) -> tauri::Result<Submenu<R>> {
    let submenu = Submenu::new(app, menu_t("menu.context.selection"), true)?;
    for row in SELECTION_ROWS {
        match row {
            SelectionRow::Item(id, key) => {
                submenu.append(&context_item(app, shortcuts, id, menu_t(key), true)?)?;
            }
            SelectionRow::SameKind => {
                let label = same_kind_menu_label(same_kind);
                submenu.append(&context_item(app, shortcuts, SELECT_SAME_KIND_ID, label, true)?)?;
            }
            SelectionRow::Separator => submenu.append(&PredefinedMenuItem::separator(app)?)?,
        }
    }
    Ok(submenu)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::menu_id_to_command;

    fn item_ids() -> Vec<&'static str> {
        SELECTION_ROWS
            .iter()
            .filter_map(|row| match row {
                SelectionRow::Item(id, _) => Some(*id),
                SelectionRow::SameKind => Some(SELECT_SAME_KIND_ID),
                SelectionRow::Separator => None,
            })
            .collect()
    }

    /// The order David settled on. A submenu is cheap to reorder by accident and expensive to
    /// notice, so it's pinned rather than described.
    #[test]
    fn the_submenu_reads_top_to_bottom_in_the_agreed_order() {
        assert_eq!(
            item_ids(),
            vec![
                TOGGLE_SELECTION_ID,
                SELECT_ALL_ID,
                DESELECT_ALL_ID,
                SELECT_SAME_KIND_ID,
                INVERT_SELECTION_ID,
                SELECT_FILES_ID,
                DESELECT_FILES_ID,
            ]
        );
        // The separator sits between Invert selection and Select files…, which is what splits the
        // act-now rows from the two that open a dialog.
        assert!(matches!(SELECTION_ROWS[5], SelectionRow::Separator));
    }

    /// A row that resolves to no command is a row that draws, looks clickable, and does nothing:
    /// `handle_menu_event` routes every one of these through `menu_id_to_command`.
    #[test]
    fn every_row_maps_to_a_command() {
        for id in item_ids() {
            assert!(
                menu_id_to_command(id).is_some(),
                "the `{id}` row has no command behind it, so clicking it would do nothing"
            );
        }
    }
}
