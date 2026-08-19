// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The context menu: a short list drawn at a point, whose every row
//! stands for a key binding that already exists.
//!
//! The menu never *implements* an action. A row carries the `KeyEvent`
//! it is a label for, and activating the row closes the menu and replays
//! that event through `handle_key`. Three things follow from that, and
//! they are the whole reason for the design:
//!
//! - nothing is reachable only by menu, so a terminal that keeps the
//!   right button for itself (see `docs/protolens/input-bindings-review.md`
//!   §6.2) costs the reader discoverability, never capability;
//! - a row cannot drift from the binding it advertises, because it *is*
//!   the binding;
//! - the row's own key works while the menu is open, so using the menu
//!   twice teaches the keystroke that replaces it.

use super::*;

/// One row: what it says, and the binding it stands for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct MenuItem {
    pub(super) label: &'static str,
    /// Replayed through `handle_key` when the row is activated, and
    /// shown on the right of the row as its own hint.
    pub(super) key: KeyEvent,
}

impl MenuItem {
    /// A row for a plain unmodified character binding.
    pub(super) const fn plain(label: &'static str, c: char) -> Self {
        Self {
            label,
            key: KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        }
    }
}

/// An open context menu.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct Menu {
    pub(super) items: Vec<MenuItem>,
    /// Where the menu was asked for — the click, or the caret's cell.
    /// The box is drawn from here and flipped at the screen edges, so
    /// this is a request rather than the final position.
    pub(super) anchor: (u16, u16),
    pub(super) selected: usize,
    /// The box's outer `Rect` as of the last render, for hit-testing.
    /// Meaningless until `render_menu` has run once.
    pub(super) area: Rect,
}

impl Menu {
    pub(super) fn new(items: Vec<MenuItem>, anchor: (u16, u16)) -> Self {
        Self {
            items,
            anchor,
            selected: 0,
            area: Rect::default(),
        }
    }

    /// Moves the highlight by `delta`, saturating at both ends rather
    /// than wrapping — a menu is short enough that wrapping reads as a
    /// glitch, and every list in this app saturates.
    pub(super) fn move_selected(&mut self, delta: isize) {
        let last = self.items.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    /// The row at terminal row `row`, or `None` if the point is outside
    /// the menu's rows. Accounts for the one-cell border.
    pub(super) fn item_at(&self, column: u16, row: u16) -> Option<usize> {
        if !App::rect_contains(self.area, column, row) {
            return None;
        }
        let idx = row.checked_sub(self.area.y + 1)? as usize;
        (idx < self.items.len()).then_some(idx)
    }

    /// Widest `label` + `hint` pair, which is what the box is sized on.
    pub(super) fn content_width(&self) -> u16 {
        self.items
            .iter()
            .map(|item| item.label.chars().count() + key_label(&item.key).chars().count() + GAP)
            .max()
            .unwrap_or(0) as u16
    }
}

/// Columns between a row's label and its key hint, at the menu's
/// narrowest.
pub(super) const GAP: usize = 3;

impl App {
    /// The rows for a right-click on the main pane, with the caret
    /// already moved to the clicked node.
    ///
    /// A row is offered only when the binding behind it would do
    /// something here: a menu that lists an action and then answers it
    /// with an error message is worse than one that never offered it,
    /// because the reader cannot tell the two failures apart.
    pub(super) fn main_menu_items(&mut self) -> Vec<MenuItem> {
        let mut items = Vec::new();
        if self.can_override(self.cursor) {
            items.push(MenuItem::plain("Override this node", 't'));
        }
        if self.has_children(self.cursor) {
            items.push(MenuItem::plain("Fold / unfold", 'z'));
            items.push(MenuItem::plain("Fold / unfold, whole subtree", 'Z'));
        }
        items.push(MenuItem::plain("Wire bytes for this line", 'w'));
        items.push(MenuItem::plain("Wire bytes for this subtree", 'W'));
        // Spec 0280 S19: offered only where there is a number to
        // explain. Asked of the node's own header line, because that is
        // the line a cue is drawn on however far down a packed run the
        // caret happens to sit.
        if !matches!(
            self.heat_cue_at(LinePos::header(self.cursor)),
            heat_cue::HeatDisplay::None
        ) {
            items.push(MenuItem::plain("What this score is made of", 's'));
        }
        // `v` is Unix-only (it hands off to a Neovim), and it needs a
        // name under the caret to jump to — the same test `v` itself
        // makes before it reports "no declaration to jump to here".
        #[cfg(unix)]
        if self.fqdn_under_focus().is_some() {
            items.push(MenuItem::plain("Go to definition", 'v'));
        }
        items.push(MenuItem::plain("Manage overrides", 'o'));
        items
    }

    /// The rows for a right-click on the main pane that names no node —
    /// the blank space past the end of the document, which is the pane
    /// itself rather than anything in it.
    ///
    /// Everything here is a *view* setting, and none of it is in
    /// `main_menu_items`. That is the desktop idiom rather than a
    /// shortage of rows: right-clicking a file offers what you can do to
    /// the file, right-clicking the desktop offers what you can do to
    /// the desktop, and a menu that showed the same twelve rows wherever
    /// it opened would teach nothing about where it opened.
    ///
    /// "Manage overrides" is the one row deliberately in both, because
    /// it is a *destination* rather than an edit — it acts on neither
    /// the node nor the view, and somewhere you can always get to is
    /// what a destination is for.
    ///
    /// The two view controls name the state they would move to, not the
    /// one they are in. A row saying "Annotations" leaves the reader to
    /// guess which way it goes; a row saying "Hide annotations" does
    /// not, and it costs a pair of literals. `i` is a three-state
    /// rotation (spec 0331) rather than a toggle, so it has three
    /// destinations to name; the row still names exactly the next one.
    pub(super) fn pane_menu_items(&self) -> Vec<MenuItem> {
        vec![
            MenuItem::plain(
                if self.annotations {
                    "Hide #@ annotations"
                } else {
                    "Show #@ annotations"
                },
                'a',
            ),
            MenuItem::plain(
                match self.heat_cues {
                    heat_cue::HeatCueMode::Off => "Show heat cues",
                    heat_cue::HeatCueMode::Findings => "Show every score",
                    heat_cue::HeatCueMode::All => "Hide heat cues",
                },
                'i',
            ),
            MenuItem::plain("Manage overrides", 'o'),
        ]
    }

    /// The rows for a right-click on the manage pane, with the highlight
    /// already moved to the clicked entry.
    ///
    /// Empty when the pane holds no entries: every row here acts on the
    /// highlighted one, so there is nothing to offer and the menu does
    /// not open at all.
    pub(super) fn manage_menu_items(&self) -> Vec<MenuItem> {
        if self.overrides.entries().is_empty() {
            return Vec::new();
        }
        vec![
            MenuItem::plain("Activate / deactivate", 'a'),
            MenuItem::plain("Activate, with children", 'A'),
            MenuItem::plain("Rotate origin kind", 'z'),
            MenuItem::plain("Duplicate", 'D'),
            MenuItem::plain("Delete", 'd'),
            MenuItem::plain("Override this entry", 'o'),
        ]
    }

    /// Where a keyboard-opened menu points: the caret's own cell in the
    /// main pane, or the left edge of the highlighted row in the manage
    /// pane.
    ///
    /// Derived rather than recorded during the last render. The two
    /// arithmetics below are the exact inverses of the ones the two
    /// panes draw with — `PaneScroll::top` and `RowHeights::offset` for
    /// the main pane, `manage_row_at`'s own for the manage pane — so
    /// there is no second copy of the layout to keep in step, and the
    /// answer is right on the first keystroke after a resize, before any
    /// render has recorded anything.
    ///
    /// Clamped into the pane both ways: the caret is normally on screen
    /// (the render auto-scrolls it into view), but a deliberate pan
    /// (`Alt`-arrows, the wheel) is allowed to leave it behind, and a
    /// menu hanging off the pane it belongs to would be worse than one
    /// pinned to the edge nearest the thing it acts on.
    pub(super) fn menu_anchor(&self) -> (u16, u16) {
        if self.manage_focus {
            let area = self.side_area;
            let row = self.manage_highlighted_row() as isize
                - self.manage_scroll.index as isize
                - self.manage_scroll.skip;
            let last = self.manage_list_height.saturating_sub(1) as isize;
            return (area.x, area.y + row.clamp(0, last) as u16);
        }
        let area = self.main_area;
        let heights = self.row_heights();
        let cursor_row = if self.tree.is_empty() {
            0
        } else {
            self.cursor_composed_row()
        };
        let row = heights.offset(cursor_row) as isize - self.scroll.top(&heights);
        let last = area.height.saturating_sub(1) as isize;
        // The caret's drawn cell, as `caret_draw_index` computes it: the
        // fold field, then the column, less the pan, plus the heat
        // gutter. Its past-the-text branch reduces to the same
        // expression, so there is only the one here.
        let column =
            (render::FOLD_FIELD_WIDTH + self.cursor_column).saturating_sub(self.pan_offset);
        let column = (area.x + render::HEAT_FIELD_WIDTH as u16).saturating_add(column as u16);
        (
            column.min(area.right().saturating_sub(1)),
            area.y + row.clamp(0, last) as u16,
        )
    }

    /// Opens the menu for whichever surface has keyboard focus, at its
    /// caret rather than at the pointer.
    ///
    /// Routed exactly as the right-click is, and refusing for the same
    /// reason in the same case: while the override selection pane is
    /// open, focus is locked to it and the main pane offers nothing, so
    /// the attempt is named rather than silently dropped.
    pub(super) fn open_menu_at_caret(&mut self) {
        if self.override_target.is_some() && !self.manage_focus {
            self.message = OVERRIDE_FOCUS_LOCK_MESSAGE.to_string();
            return;
        }
        let anchor = self.menu_anchor();
        let items = if self.manage_focus {
            self.manage_menu_items()
        } else {
            self.main_menu_items()
        };
        self.open_menu(items, anchor);
    }

    /// Opens `items` at `anchor`, unless there is nothing to offer.
    ///
    /// Refusing an empty menu is what keeps every caller from having to
    /// check: a surface with no applicable action simply does not get a
    /// menu, and the click that asked for one is spent.
    pub(super) fn open_menu(&mut self, items: Vec<MenuItem>, anchor: (u16, u16)) {
        if items.is_empty() {
            return;
        }
        self.menu = Some(Menu::new(items, anchor));
    }

    /// Moves the open menu's highlight, if there is one.
    pub(super) fn move_menu_selection(&mut self, delta: isize) {
        if let Some(menu) = &mut self.menu {
            menu.move_selected(delta);
        }
    }

    /// Closes the menu and replays the binding on row `idx`.
    ///
    /// Closing *first* is what makes the replay safe: `handle_key`
    /// answers an open menu ahead of everything else, so dispatching
    /// while one is still open would come straight back here.
    pub(super) fn activate_menu_item(&mut self, idx: usize) {
        let Some(menu) = self.menu.take() else { return };
        let Some(item) = menu.items.get(idx) else {
            return;
        };
        self.handle_key(item.key);
    }

    /// The keyboard, while a menu is open. It is the innermost modal in
    /// the app, so this runs ahead of every other tier — including the
    /// help overlay, which a menu may legitimately be drawn over.
    pub(super) fn handle_menu_key(&mut self, key: KeyEvent) {
        let Some(menu) = &mut self.menu else { return };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                let idx = menu.selected;
                self.activate_menu_item(idx);
            }
            KeyCode::Char('n') if ctrl => menu.move_selected(1),
            KeyCode::Char('p') if ctrl => menu.move_selected(-1),
            KeyCode::Char('j') | KeyCode::Down => menu.move_selected(1),
            KeyCode::Char('k') | KeyCode::Up => menu.move_selected(-1),
            KeyCode::Home => menu.selected = 0,
            KeyCode::End => menu.selected = menu.items.len().saturating_sub(1),
            // A row's own binding activates it. This is the menu paying
            // for itself: the second time a reader reaches for it, the
            // keystroke they will eventually use on its own already
            // works from inside it.
            _ => {
                if let Some(idx) = menu.items.iter().position(|i| matches_hotkey(&key, &i.key)) {
                    self.activate_menu_item(idx);
                }
            }
        }
    }
}

/// Whether a pressed key is the one a row advertises.
///
/// Compares the code and the `Ctrl`/`Alt` state, but not `Shift`: a
/// terminal reports a capital as `Char('Z')` and may or may not set
/// `SHIFT` alongside it, and the case already carries the distinction.
fn matches_hotkey(pressed: &KeyEvent, bound: &KeyEvent) -> bool {
    const REAL: KeyModifiers = KeyModifiers::CONTROL.union(KeyModifiers::ALT);
    pressed.code == bound.code
        && pressed.modifiers.intersection(REAL) == bound.modifiers.intersection(REAL)
}

/// How a binding is spelled on a menu row, and in `HELP_TEXT`.
///
/// Deliberately the *help's* spelling (`Ctrl-x`, not `C-x` or `^X`):
/// the menu's job is to point at the help's vocabulary, so it has to
/// use it.
pub(super) fn key_label(key: &KeyEvent) -> String {
    let mut out = String::new();
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        out.push_str("Ctrl-");
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        out.push_str("Alt-");
    }
    // A capital letter already carries its own Shift; saying so twice
    // ("Shift-D") is noise, and the help never writes it that way.
    if key.modifiers.contains(KeyModifiers::SHIFT)
        && !matches!(key.code, KeyCode::Char(c) if c.is_ascii_uppercase())
    {
        out.push_str("Shift-");
    }
    match key.code {
        KeyCode::Char(' ') => out.push_str("Space"),
        KeyCode::Char(c) => out.push(c),
        KeyCode::Enter => out.push_str("Enter"),
        KeyCode::Delete => out.push_str("Delete"),
        KeyCode::Esc => out.push_str("Esc"),
        other => out.push_str(&format!("{other:?}")),
    }
    out
}
