// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `input-bindings-review.md` C9: the context menu.
//!
//! What is worth asserting here is not what the rows *do* — each of them
//! is a binding with its own tests elsewhere — but the four properties
//! the design rests on: a row replays its binding, the row's own key
//! works from inside the menu, an empty menu never opens, and dismissing
//! one costs nothing.

use super::super::*;
use super::support::*;

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn right_click(app: &mut App, column: u16, row: u16) {
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    });
}

fn labels(app: &App) -> Vec<&'static str> {
    app.menu
        .as_ref()
        .expect("menu must be open")
        .items
        .iter()
        .map(|i| i.label)
        .collect()
}

fn row_of(app: &App, label: &str) -> usize {
    labels(app)
        .iter()
        .position(|l| *l == label)
        .unwrap_or_else(|| panic!("no {label:?} row in {:?}", labels(app)))
}

/// A pane laid out where the fixture's rows fall on rows 0, 1, 2.
fn opened() -> App {
    let (mut app, _, _) = type_as_fixture();
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 20);
    app
}

/// Right-click moves the caret to what was clicked *before* building the
/// rows, since every row acts on the caret — a menu offering the old
/// node's actions at the new node's position is the one bug this whole
/// ordering exists to prevent.
#[test]
fn right_click_moves_the_caret_then_opens_the_menu() {
    let mut app = opened();
    app.manage_focus = true;

    right_click(&mut app, 5, 1);

    assert!(
        app.menu.is_some(),
        "a right-click in the main pane opens it"
    );
    assert!(
        !app.manage_focus,
        "and takes focus back, exactly as a left-click does"
    );
    assert_eq!(
        app.cursor_line(),
        1,
        "the caret is on the clicked line, not where it was"
    );
}

/// The rows offered are only the ones whose binding would do something
/// here: a leaf has nothing to fold, so it is not told it can.
#[test]
fn only_applicable_rows_are_offered() {
    let (mut app, inner_idx, id_idx) = type_as_fixture();
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 20);

    app.cursor = inner_idx;
    let items = app.main_menu_items();
    assert!(items.iter().any(|i| i.label == "Fold / unfold"));

    app.cursor = id_idx;
    let items = app.main_menu_items();
    assert!(
        !items.iter().any(|i| i.label == "Fold / unfold"),
        "a leaf has no fold to offer"
    );
    assert!(
        items.iter().any(|i| i.label == "Wire bytes for this line"),
        "but it still has its own bytes"
    );
}

/// Activating a row closes the menu and replays the binding it stands
/// for — the row does not reimplement the action, it *is* the action's
/// key.
#[test]
fn activating_a_row_replays_its_binding() {
    let mut app = opened();
    right_click(&mut app, 5, 0);
    let node = app.cursor;
    let row = row_of(&app, "Fold / unfold");

    app.activate_menu_item(row);

    assert!(app.menu.is_none(), "activating closes the menu");
    assert!(app.folded.contains(node), "and folds, as `z` itself would");
}

/// The second time a reader reaches for the menu, the keystroke that
/// will eventually replace it already works from inside it.
#[test]
fn a_rows_own_key_activates_it_while_the_menu_is_open() {
    let mut app = opened();
    right_click(&mut app, 5, 0);
    let node = app.cursor;

    app.handle_key(key('z'));

    assert!(app.menu.is_none());
    assert!(app.folded.contains(node));
}

/// A click past the end of the document names no node, which is a
/// surface rather than a miss: it gets the view's own settings, and none
/// of the node rows.
#[test]
fn a_click_past_the_document_offers_the_views_settings() {
    let mut app = opened();

    right_click(&mut app, 5, 15);

    let labels = labels(&app);
    assert!(labels.contains(&"Hide #@ annotations"), "{labels:?}");
    assert!(labels.contains(&"Hide heat cues"), "{labels:?}");
    assert!(
        !labels.iter().any(|l| l.starts_with("Wire bytes")),
        "no node here, so nothing that acts on one: {labels:?}"
    );

    // And the labels name the state the row would move *to*.
    app.annotations = false;
    app.heat_cues_hidden = true;
    let labels: Vec<&str> = app.pane_menu_items().iter().map(|i| i.label).collect();
    assert!(labels.contains(&"Show #@ annotations"), "{labels:?}");
    assert!(labels.contains(&"Show heat cues"), "{labels:?}");
}

/// Dismissing costs nothing, either way of dismissing it — which is what
/// makes an accidental right-click free.
#[test]
fn esc_and_a_click_away_both_close_without_acting() {
    let mut app = opened();
    right_click(&mut app, 5, 0);
    let node = app.cursor;
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.menu.is_none());
    assert!(!app.folded.contains(node));

    right_click(&mut app, 5, 0);
    // The menu has never been rendered, so its `area` is still the
    // default empty `Rect` and every point is outside it.
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 30,
        row: 18,
        modifiers: KeyModifiers::NONE,
    });
    assert!(app.menu.is_none());
    assert!(!app.folded.contains(node));
}

/// A surface with no applicable action simply does not get a menu — a
/// box with nothing in it would be worse than no box, because it invites
/// a second try. Refusing here is what keeps every caller from checking.
#[test]
fn a_menu_with_no_rows_never_opens() {
    let mut app = opened();

    app.open_menu(Vec::new(), (5, 5));
    assert!(app.menu.is_none());

    // The manage pane's rows all act on a highlighted entry, so a
    // right-click on a part of it that names no entry is the same case.
    app.manage_open = true;
    app.side_area = Rect::new(60, 0, 40, 20);
    app.manage_list_height = 20;
    right_click(&mut app, 62, 19);
    assert!(app.menu.is_none());
}

/// `m` opens the same menu at the caret, for the terminals that keep the
/// right button for themselves (§6.2).
#[test]
fn m_opens_the_menu_at_the_caret() {
    let mut app = opened();
    app.cursor_column = 3;
    app.handle_key(key('m'));

    let menu = app.menu.as_ref().expect("`m` opens the menu");
    assert_eq!(
        menu.anchor,
        (
            (render::FOLD_FIELD_WIDTH + 3 + render::HEAT_FIELD_WIDTH) as u16,
            app.main_area.y + app.cursor_display_row() as u16
        ),
        "anchored on the caret's own cell"
    );
    assert!(labels(&app).contains(&"Fold / unfold"));
}
