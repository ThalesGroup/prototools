// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0242: the main-pane selection.

use super::super::*;
use super::support::*;

/// Spec 0242 test-plan items 1 and 2 (S1, S2, S4). The first
/// `Shift`-motion anchors where the caret already was; later ones only
/// move the caret, and both endpoint cells are inside the span.
#[test]
fn the_first_shift_motion_anchors_and_the_rest_only_move_the_caret() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2"]);
    app.splash = false;
    app.cursor = 0;
    app.reset_caret_column();
    assert_eq!(app.selection_span(), None, "nothing is selected at rest");

    app.select_right();
    let anchor = app.select_anchor.expect("the first motion anchors");
    assert_eq!(anchor.column, 0, "on the caret's own column");
    assert_eq!(
        app.selection_span(),
        Some((0, 0, 0, 2)),
        "both endpoint cells are in the span, so one press takes two"
    );

    app.select_right();
    assert_eq!(
        app.select_anchor,
        Some(anchor),
        "a second motion leaves the anchor alone"
    );
    assert_eq!(app.selection_span(), Some((0, 0, 0, 3)));

    let (count, text) = app.selected_text().expect("a span must yield text");
    assert_eq!(count, 1);
    assert_eq!(text, "alp");
}

/// Spec 0242 S2, the user's own correction (2026-08-05): once a
/// selection is engaged, the caret back on the anchor is *one*
/// character, not none — which is how the keyboard asks for a single
/// character at all.
#[test]
fn shift_right_then_shift_left_leaves_one_character_selected() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2"]);
    app.splash = false;
    app.cursor = 0;
    app.reset_caret_column();

    app.select_right();
    app.select_left();
    assert_eq!(app.cursor_column, 0, "the caret is back on the anchor");
    assert_eq!(app.selection_span(), Some((0, 0, 0, 1)));
    let (count, text) = app.selected_text().expect("a span must yield text");
    assert_eq!(count, 1);
    assert_eq!(text, "a");
}

/// Spec 0242 S5: the horizontal motions wrap onto the neighboring row at
/// either end, which is how a selection crosses a line boundary at all.
#[test]
fn a_horizontal_selection_wraps_onto_the_neighboring_row() {
    let mut app = sibling_leaves_app(&["ab", "cd"]);
    app.splash = false;
    app.cursor = 0;
    app.reset_caret_column();
    app.caret_to_line_end();

    app.select_right();
    assert_eq!(app.cursor, 1, "off the right end and onto the next row");
    assert_eq!(app.selection_span(), Some((0, 1, 1, 1)));
    let (_, text) = app.selected_text().expect("a span must yield text");
    assert_eq!(text, "b\nc");

    // And back the way it came, onto the row above.
    app.select_left();
    assert_eq!(
        app.selection_span(),
        Some((0, 1, 0, 2)),
        "back onto the anchor, which is one character and not none"
    );
    app.select_left();
    assert_eq!(app.cursor, 0);
    assert_eq!(app.selection_span(), Some((0, 0, 0, 2)));
}

/// Spec 0242 S3: any key that is not one of the four selection keys —
/// nor the `Ctrl-C` that copies what they selected — drops the
/// selection, so a plain motion does not drag it along behind the caret.
#[test]
fn a_plain_motion_drops_the_selection_and_ctrl_c_does_not() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2"]);
    app.splash = false;
    app.cursor = 0;
    app.reset_caret_column();

    app.handle_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::NONE));
    assert!(app.selection_span().is_some(), "`L` selects");

    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(
        app.selection_span().is_some(),
        "`Ctrl-C` copies without clearing"
    );

    app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
    assert_eq!(app.select_anchor, None, "a plain motion clears it");
}

/// Spec 0242 S6, the user's own correction (2026-08-04): a selection
/// motion changes nothing but the caret. It neither folds a node on the
/// way past nor opens one — a fold would hide rows already selected, an
/// unfold would reveal rows that were not.
#[test]
fn a_selection_motion_neither_folds_nor_unfolds() {
    let (mut app, items) = repeated_message_fixture();
    app.splash = false;
    app.set_cursor(items[0]);
    app.toggle_fold(items[0]);
    app.set_cursor(items[0]);
    app.caret_to_line_end();

    let folded_before = app.folded.clone();
    for _ in 0..6 {
        app.select_right();
    }
    assert_eq!(
        app.folded, folded_before,
        "stepping off the end of a folded node's header opens nothing"
    );

    app.caret_to_line_start();
    for _ in 0..6 {
        app.select_left();
    }
    assert_eq!(
        app.folded, folded_before,
        "and a leftward motion at Home folds nothing"
    );
}

/// Spec 0242 S12, the user's own correction (2026-08-04): the copy walks
/// the document, not the screen, so a selection dragged across a
/// collapsed message copies the message rather than the `{ ... }`
/// placeholder that stands in for it.
#[test]
fn the_copy_includes_what_a_fold_is_hiding() {
    let (mut app, items) = repeated_message_fixture();
    app.splash = false;

    // Select the whole document, unfolded, and keep what it says.
    let last = app.document_lines().len() - 1;
    let select_all = |app: &mut App| {
        app.select_anchor = Some(CursorPos {
            node: app.first_node,
            line_in_node: 0,
            column: 0,
        });
        app.select_engaged = true;
        let end = app.line_pos(last).expect("the last row must exist");
        app.cursor = end.node;
        app.cursor_line_in_node = end.line_in_node;
        app.caret_to_line_end();
        app.selected_text().expect("a span must yield text").1
    };
    let unfolded = select_all(&mut app);
    assert!(!unfolded.is_empty());

    app.toggle_fold(items[0]);
    assert!(app.folded.contains(&items[0]), "the fixture must fold");
    assert_eq!(
        select_all(&mut app),
        unfolded,
        "a fold changes the screen, never the clipboard"
    );
    assert!(
        !unfolded.contains(" ... }"),
        "and no collapse summary leaks into it"
    );
}
