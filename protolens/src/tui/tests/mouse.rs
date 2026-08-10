// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::super::*;
use super::support::*;

/// Regression test (2026-07-15 feedback): `EnableMouseCapture` turns
/// on any-motion tracking, so real terminals send a bare `Moved`
/// event on essentially every pixel the cursor crosses, with no
/// click at all — a pure `Moved` event must not dismiss the splash
/// screen (nor clear a status message), unlike every other mouse
/// event kind, which legitimately counts as user input (spec 0113
/// D28).
#[test]
fn bare_mouse_move_does_not_dismiss_the_splash_screen() {
    let mut app = message_node_app();
    app.main_area = Rect::new(0, 0, 40, 20);
    assert!(app.splash);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert!(app.splash, "a bare mouse move must not dismiss the splash");

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert!(
        !app.splash,
        "an actual click must still dismiss the splash, same as before"
    );
}

/// Feedback (2026-07-15): mouse wheel and Shift-wheel scroll the `F1`
/// help overlay when the pointer hovers over it, instead of leaking
/// through to the main pane drawn underneath.
#[test]
fn mouse_wheel_scrolls_the_help_overlay_when_hovered() {
    let mut app = message_node_app();
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 20);
    app.help_open = true;
    app.help_area = Rect::new(5, 5, 30, 10);

    let cursor_before = app.cursor;
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 10,
        row: 6,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.help_scroll, 1);
    assert_eq!(
        app.cursor, cursor_before,
        "must not also scroll the main pane underneath"
    );

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 10,
        row: 6,
        modifiers: KeyModifiers::SHIFT,
    });
    assert_eq!(app.help_scroll, 2, "Shift-wheel scrolls it too");

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 10,
        row: 6,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.help_scroll, 1);

    // Hovering outside the overlay (but still over the main pane)
    // must not touch `help_scroll` — it falls through to the pane
    // underneath instead.
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 1,
        row: 1,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.help_scroll, 1, "unhovered help overlay must not react");
}

/// A mouse click landing in the main pane shifts keyboard focus back
/// to it without closing the still-open side pane (2026-07-14
/// feedback, item 3). Spec 0185 S5 exempts the override *selection*
/// pane from this, so the management pane is what carries the rule now.
#[test]
fn mouse_click_in_main_pane_refocuses_without_closing_the_manage_pane() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.cursor = inner_idx;
    app.manage_open = true;
    app.manage_focus = true;

    app.main_area = Rect::new(0, 0, 40, 20);
    app.side_area = Rect::new(60, 0, 40, 20);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });

    assert!(
        !app.manage_focus,
        "click in main pane must shift focus back to it"
    );
    assert!(app.manage_open, "the management pane must stay open");
}

/// Wheel scroll routes to whichever pane the mouse is hovering, not
/// whichever pane currently has keyboard focus (2026-07-14 feedback,
/// item 4). 2026-07-19 feedback item 2: the wheel now pans the
/// hovered pane's own scroll/pan offset instead of moving the cursor/
/// highlight, so this checks `scroll.index`/`override_scroll` — a
/// 1-row pane height on both sides ensures each has room to pan by
/// `WHEEL_PAN_STEP`.
#[test]
fn mouse_wheel_routes_by_hover_position_not_keyboard_focus() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.cursor = inner_idx;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(app.override_focus, "keyboard focus starts in the side pane");

    app.main_area = Rect::new(0, 0, 40, 1);
    app.side_area = Rect::new(60, 0, 40, 1);
    app.override_candidates = vec![("a.B".to_string(), None), ("a.C".to_string(), None)];
    app.override_list_height = 1;
    app.override_highlight = 0;
    app.override_scroll.index = 0;
    app.scroll.index = 0;

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 5,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.scroll.index, WHEEL_PAN_STEP,
        "hovering the main pane must pan it, even though the side \
         pane still has keyboard focus"
    );
    assert_eq!(
        app.override_scroll.index, 0,
        "the unhovered side pane must not react"
    );

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 65,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.scroll.index, WHEEL_PAN_STEP,
        "hovering the side pane must not pan the main pane"
    );
    assert_eq!(
        app.override_scroll.index, WHEEL_PAN_STEP,
        "hovering the side pane must pan it"
    );
}

/// Spec 0127 §G2: Shift+wheel pans whichever pane the mouse is
/// hovering; plain wheel (no Shift) keeps scrolling vertically as
/// before. 2026-07-19 feedback: Shift+wheel now pans by
/// `WHEEL_PAN_STEP` (not `PAN_STEP`, which stays reserved for Ctrl-
/// Left/Ctrl-Right); plain wheel now pans the viewport (item 2), no
/// longer moves the cursor.
#[test]
fn shift_wheel_pans_the_hovered_main_pane_plain_wheel_still_scrolls() {
    let (mut app, _items) = repeated_message_fixture();
    app.main_area = Rect::new(0, 0, 5, 1);
    app.side_area = Rect::new(60, 0, 40, 20);

    let cursor_before = app.cursor;
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 2,
        row: 0,
        modifiers: KeyModifiers::SHIFT,
    });
    assert_eq!(
        app.cursor, cursor_before,
        "Shift+wheel must pan, not move the cursor"
    );
    assert_eq!(
        app.pan_offset, WHEEL_PAN_STEP,
        "Shift+ScrollDown must pan right by the wheel step"
    );

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 2,
        row: 0,
        modifiers: KeyModifiers::SHIFT,
    });
    assert_eq!(app.pan_offset, 0, "Shift+ScrollUp must pan left");

    let scroll_before = app.scroll.index;
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 2,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.cursor, cursor_before,
        "plain wheel (no Shift) no longer moves the cursor (2026-07-19 \
         feedback item 2)"
    );
    assert_ne!(
        app.scroll.index, scroll_before,
        "plain wheel (no Shift) must still pan the viewport vertically"
    );
    assert_eq!(app.pan_offset, 0, "plain wheel must not pan horizontally");
}

/// Spec 0127 §G2: native `ScrollLeft`/`ScrollRight` pan the hovered
/// pane without needing Shift. 2026-07-19 feedback: at `WHEEL_PAN_STEP`,
/// not `PAN_STEP` (reserved for Ctrl-Left/Ctrl-Right).
#[test]
fn native_scroll_left_right_pans_without_shift() {
    let (mut app, _items) = repeated_message_fixture();
    app.main_area = Rect::new(0, 0, 5, 20);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollRight,
        column: 2,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.pan_offset, WHEEL_PAN_STEP, "ScrollRight must pan right");

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollLeft,
        column: 2,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.pan_offset, 0, "ScrollLeft must pan left");
}

/// Spec 0127 §G1: the override pane and the manage pane each carry
/// their own `pan_offset`, independent of the main pane's and of each
/// other's. 2026-07-19 feedback: wheel-driven pan now steps by
/// `WHEEL_PAN_STEP` and is clamped on the right (item 4), so explicit
/// `*_list_height`/candidate setup is needed to leave room to pan.
#[test]
fn override_and_manage_panes_pan_independently_of_the_main_pane() {
    let (mut app, items) = repeated_message_fixture();
    app.main_area = Rect::new(0, 0, 40, 20);
    app.side_area = Rect::new(60, 0, 5, 20);

    app.manage_open = true;
    app.manage_list_height = 5;
    app.overrides.activate(
        OverrideOrigin::Path {
            path: app.positional_path(items[0]),
        },
        None,
    );
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollRight,
        column: 62,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.manage_pan_offset, WHEEL_PAN_STEP);
    assert_eq!(
        app.pan_offset, 0,
        "the main pane's pan_offset must be untouched"
    );

    app.manage_open = false;
    app.override_target = Some(items[0]);
    app.override_list_height = 5;
    app.override_candidates = vec![("cand.SomeVeryLongTypeNameHere".to_string(), None)];
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollRight,
        column: 62,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.override_pan_offset, WHEEL_PAN_STEP);
    assert_eq!(
        app.manage_pan_offset, WHEEL_PAN_STEP,
        "unrelated to the override pane's own offset"
    );
}

/// Spec 0244 S9 / test-plan item 9: an over-panned side pane draws blank
/// rows above its first row, and clicking one of them names nothing —
/// without the guard the negative skip would fold back into an index and
/// select a row several places further down the list.
#[test]
fn a_click_on_a_blank_row_selects_nothing() {
    let (mut app, items) = repeated_message_fixture();
    app.side_area = Rect::new(60, 0, 20, 5);

    // Override pane: three blank rows, then candidate 0.
    app.override_target = Some(items[0]);
    app.override_list_height = 5;
    app.override_candidates = (0..10).map(|i| (format!("cand.Type{i}"), None)).collect();
    app.override_highlight = 7;
    app.override_scroll.set_top(-3, &FLAT_ROWS);
    for row in 0..3 {
        app.handle_override_click(62, row);
        assert_eq!(
            app.override_highlight, 7,
            "blank row {row} must leave the highlight alone"
        );
    }
    app.handle_override_click(62, 3);
    assert_eq!(
        app.override_highlight, 0,
        "the first real row is candidate 0"
    );

    // Manage pane, same shape.
    app.override_target = None;
    app.manage_open = true;
    app.manage_list_height = 5;
    for field in 1..=4 {
        app.overrides.activate(
            OverrideOrigin::PathField {
                path: "/".to_string(),
                field,
            },
            None,
        );
    }
    let last = app.overrides.entries().len() - 1;
    app.manage_highlight = last;
    app.manage_scroll.set_top(-3, &FLAT_ROWS);
    for row in 0..3 {
        app.handle_manage_click(70, row, false);
        assert_eq!(
            app.manage_highlight, last,
            "blank row {row} must leave the highlight alone"
        );
    }
}

/// Spec 0129 §G1/§G3, as amended by spec 0242 S10: click-drag across N
/// main-pane rows, release. The drag moves the *caret*, so the selection
/// runs from the character the click landed on to the one under the
/// pointer; dragging past the end of the last row's text clamps there,
/// which is how a whole-line drag is spelled.
#[test]
fn drag_select_spans_multiple_main_pane_rows() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3", "delta: 4"]);
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 20);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 1,
        modifiers: KeyModifiers::NONE,
    });
    assert!(app.select_anchor.is_some(), "the click seeds the anchor");
    assert_eq!(
        app.selection_span(),
        None,
        "but a click engages no selection, so nothing is selected yet"
    );

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 39,
        row: 3,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.selection_span(), Some((1, 0, 3, 8)));

    let (count, text) = app.selected_text().expect("selection must be active");
    assert_eq!(count, 3);
    assert_eq!(text, "beta: 2\ngamma: 3\ndelta: 4");

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 39,
        row: 3,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.selection_span(),
        Some((1, 0, 3, 8)),
        "selection persists after release"
    );
    // Spec 0131 §G1/test plan item 3: mouse release no longer copies
    // by itself.
    assert!(
        app.message.is_empty(),
        "unexpected message: {}",
        app.message
    );

    // Spec 0131 §G1/test plan item 1: `Ctrl-C` copies the persisted
    // drag-selection. No working clipboard provider exists in this
    // (headless) test environment — the OSC 52 fallback path is
    // exactly what's exercised here, not a panic.
    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(
        app.message == "3 line(s) copied to clipboard"
            || app.message == "3 line(s) copied to clipboard (OSC 52 fallback)",
        "unexpected message: {}",
        app.message
    );
}

/// A plain click (`Down`+`Up`, no drag, not the second half of a
/// double-click) must deselect any active selection rather than leave a
/// length-1 selection behind. Selecting a single line by mouse is the
/// double-click's job instead — see
/// `double_click_selects_the_clicked_line_for_copy`.
#[test]
fn plain_click_with_no_drag_deselects() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3"]);
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 20);

    // Seed an existing drag-selection first, so this also proves a
    // later plain click clears a *pre-existing* selection, not just
    // "never selects in the first place".
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 39,
        row: 2,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 39,
        row: 2,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.selection_span(), Some((0, 0, 2, 8)));

    // A plain click on a different line (no drag) must clear it.
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 1,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 0,
        row: 1,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.select_anchor, None, "plain click deselects");
    assert_eq!(app.selection_span(), None);
}

/// Spec 0242 S10, the user's own correction (2026-08-05): the mouse's
/// way of selecting exactly one character is to drag off it and back —
/// the counterpart of `Shift-Right` `Shift-Left`. A drag engages the
/// selection and stays engaged, so the caret returning to the anchor is
/// one character rather than none, and the release keeps it.
#[test]
fn a_drag_out_and_back_selects_the_single_character_it_started_on() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2"]);
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 20);
    let text_x = 1 + render::FOLD_FIELD_WIDTH as u16;

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: text_x + 2,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: text_x + 5,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.selection_span(), Some((0, 2, 0, 6)));

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: text_x + 2,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: text_x + 2,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.selection_span(),
        Some((0, 2, 0, 3)),
        "back on the character it started on, and the release keeps it"
    );

    let (count, text) = app.selected_text().expect("selection must be active");
    assert_eq!(count, 1);
    assert_eq!(text, "p");

    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(
        app.message == "1 character(s) copied to clipboard"
            || app.message == "1 character(s) copied to clipboard (OSC 52 fallback)",
        "unexpected message: {}",
        app.message
    );
}

/// Spec 0242 S10, extended by the user (2026-08-05): `Shift`-click is
/// the mouse's spelling of a run of `Shift`-motions. With nothing
/// engaged it anchors at the caret and selects up to the click; with a
/// selection already engaged the anchor holds still, so the same
/// gesture extends it or contracts it depending on where it lands.
#[test]
fn shift_click_extends_the_selection_from_the_caret_then_from_the_anchor() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2"]);
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 20);
    let text_x = 1 + render::FOLD_FIELD_WIDTH as u16;
    let shift_click = |app: &mut App, column: u16, modifiers: KeyModifiers| {
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.handle_mouse(MouseEvent {
                kind,
                column,
                row: 0,
                modifiers,
            });
        }
    };

    // Put the caret on column 2 with a plain click, which selects
    // nothing.
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: text_x + 2,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: text_x + 2,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.selection_span(), None);

    // Unengaged: anchor at the caret, select through the click.
    shift_click(&mut app, text_x + 5, KeyModifiers::SHIFT);
    assert_eq!(app.selection_span(), Some((0, 2, 0, 6)));

    // Engaged: the anchor stays on column 2 and the selection contracts.
    // `Ctrl` is the alias most terminals actually forward.
    shift_click(&mut app, text_x + 3, KeyModifiers::CONTROL);
    assert_eq!(app.selection_span(), Some((0, 2, 0, 4)));

    // ...and extends the other way, past the anchor.
    shift_click(&mut app, text_x, KeyModifiers::SHIFT);
    assert_eq!(app.selection_span(), Some((0, 0, 0, 3)));

    let (lines, text) = app.selected_text().expect("selection must be active");
    assert_eq!(lines, 1);
    assert_eq!(text, "alp");
}

/// Feedback (2026-07-15): double-clicking a main-pane line (two
/// `Down`+`Up` clicks on the same line, in quick succession)
/// explicitly selects that line for copy. Crossterm reports `Down`
/// identically for single and double clicks, so this exercises the
/// app's own timestamp/position-based disambiguation
/// (`App::last_click`/`pending_double_click`).
///
/// Spec 0242 S10, as amended by the user (2026-08-05): selecting the
/// line is *all* it does. It used to double as the `t`/`o` smart proxy
/// (spec 0139), and this fixture's cursor node is the seeded root
/// override's own target, so the assertion that the manage pane stays
/// shut is a real one.
#[test]
fn double_click_selects_the_clicked_line_for_copy() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2"]);
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 20);
    app.term_width = 120;

    for _ in 0..2 {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
    }

    assert_eq!(
        app.selection_span(),
        Some((0, 0, 0, 8)),
        "double-click selects the whole line it landed on"
    );
    assert!(
        app.message.is_empty(),
        "unexpected message: {}",
        app.message
    );
    assert!(
        !app.manage_open,
        "and does not also open a side pane, even on an overridden node"
    );
    assert!(app.override_target.is_none());

    let (count, text) = app.selected_text().expect("selection must be active");
    assert_eq!(count, 1);
    assert_eq!(text, "alpha: 1");

    // Spec 0242 S13: a selection inside one line is reported in
    // characters, since "1 line(s)" would be a lie about a partial one
    // and this message is the only sign the copy happened.
    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(
        app.message == "8 character(s) copied to clipboard"
            || app.message == "8 character(s) copied to clipboard (OSC 52 fallback)",
        "unexpected message: {}",
        app.message
    );
}

/// The user's report (2026-08-05): "a quick sequence of 4 clicks on the
/// fold-toggle results as if there had only been 3." Whatever the
/// terminal does with a fast run, the app's own contract is that every
/// click that reaches it toggles — no timing, no counting, no state
/// carried from the previous click.
#[test]
fn every_click_on_the_fold_marker_toggles_however_many_arrive() {
    let mut app = message_node_app();
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 20);
    let node = 0;
    let line = app.line_text(app.line_pos(app.absolute_start(node)).unwrap());
    let marker = render::marker_column(&line);

    let folded = app.folded.contains(&node);
    for i in 1..=6 {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: app.main_area.x + 1 + marker,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: app.main_area.x + 1 + marker,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            app.folded.contains(&node),
            folded != (i % 2 == 1),
            "click {i} must have toggled"
        );
    }
}

/// Spec 0129 §G1/test plan item 3: dragging upward (end row above
/// start row) still copies the correct range in top-to-bottom
/// document order, not reversed.
#[test]
fn drag_select_upward_still_copies_top_to_bottom() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3"]);
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 20);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 39,
        row: 2,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.selection_span(),
        Some((0, 0, 2, 8)),
        "the span is ordered, whichever way the pointer went"
    );

    let (count, text) = app.selected_text().expect("selection must be active");
    assert_eq!(count, 3, "top-to-bottom order, not reversed");
    assert_eq!(text, "alpha: 1\nbeta: 2\ngamma: 3");

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert!(
        app.message.is_empty(),
        "unexpected message: {}",
        app.message
    );

    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(
        app.message == "3 line(s) copied to clipboard"
            || app.message == "3 line(s) copied to clipboard (OSC 52 fallback)",
        "unexpected message: {}",
        app.message
    );
}

/// Spec 0131 §G1/test plan item 2: `Ctrl-C` with no active selection
/// copies exactly the cursor's current line.
#[test]
fn ctrl_c_with_no_selection_copies_cursor_line() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2"]);
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 20);

    assert_eq!(app.select_anchor, None, "no selection active yet");
    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(
        app.select_anchor, None,
        "and copying a line does not start one"
    );
    assert!(
        app.message == "1 line(s) copied to clipboard"
            || app.message == "1 line(s) copied to clipboard (OSC 52 fallback)",
        "unexpected message: {}",
        app.message
    );
}

/// Spec 0131 §G2/test plan item 4: a clipboard-unavailable
/// environment (no provider reachable, as in this headless test
/// harness) produces the OSC 52 fallback message instead of
/// panicking.
#[test]
fn clipboard_unavailable_shows_fallback_message_without_panicking() {
    let mut app = sibling_leaves_app(&["alpha: 1"]);
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 20);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.selection_span(),
        None,
        "spec 0242 S10: a click on a character selects nothing"
    );
    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    // This sandbox has no reachable clipboard provider, so the
    // failure branch is exactly what's exercised here.
    assert!(
        app.message == "1 line(s) copied to clipboard"
            || app.message == "1 line(s) copied to clipboard (OSC 52 fallback)",
        "unexpected message: {}",
        app.message
    );
}

/// Spec 0129 §G3: a fresh click starts a new selection, replacing
/// the old one; `Esc` clears an existing selection's highlight too.
#[test]
fn fresh_click_replaces_selection_esc_clears_it() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3"]);
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 20);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 39,
        row: 2,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 39,
        row: 2,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.selection_span(), Some((0, 0, 2, 8)));

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 1,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.selection_span(),
        None,
        "a fresh click replaces the old selection with an empty one"
    );

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.select_anchor, None, "Esc clears the selection");
}

/// Regression test: clicking a foldable node's `⏵`/`⏷` marker must
/// still toggle its fold now that the heat-cue gutter (spec 0138 N1)
/// permanently occupies column 0 of `main_area`, shifting every line's
/// own text (and its marker) one column to the right.
#[test]
fn clicking_the_fold_marker_toggles_the_node_despite_the_heat_cue_gutter() {
    let (mut app, grp_idx) = group_type_fixture();
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 20);
    assert!(app.has_children(grp_idx));
    assert!(!app.folded.contains(&grp_idx));

    let line_idx = app.absolute_start(grp_idx);
    let indent_len = (app.document_lines()[line_idx].len()
        - app.document_lines()[line_idx].trim_start().len()) as u16;
    // Column 0 is the heat-cue gutter, so the marker itself sits at
    // column `indent_len + 1`.
    let marker_col = indent_len + 1;

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: marker_col,
        row: line_idx as u16,
        modifiers: KeyModifiers::NONE,
    });
    assert!(
        app.folded.contains(&grp_idx),
        "clicking the marker must fold the node"
    );

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: marker_col,
        row: line_idx as u16,
        modifiers: KeyModifiers::NONE,
    });
    assert!(
        !app.folded.contains(&grp_idx),
        "clicking the marker again must unfold the node"
    );
}

/// The fold margin is part of the row's content, so panning right
/// scrolls it off the left edge along with everything else — the hit
/// target has to follow the glyph the user can see, not the column it
/// occupied before the pan.
#[test]
fn a_marker_click_follows_the_glyph_when_the_view_is_panned() {
    let (mut app, grp_idx) = group_type_fixture();
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 20);

    let line_idx = app.absolute_start(grp_idx);
    let row = line_idx as u16;
    let marker = render::marker_column(&app.line_text(app.line_pos(line_idx).unwrap()));
    assert!(marker >= 2, "the fixture's group must be indented");
    app.pan_offset = marker as usize;

    // Panned by exactly the marker's own column, the glyph is drawn in
    // the pane's first text column, immediately right of the heat-cue
    // gutter.
    let drawn_col = app.main_area.x + 1;
    let former_col = drawn_col + marker;
    let click = |app: &mut App, column| {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
    };

    // The column the marker sat at before the pan now shows plain text,
    // and must be treated as such. Checked first, because the fold
    // below narrows the content and `folds_changed` clamps `pan_offset`
    // to match.
    click(&mut app, former_col);
    assert!(
        !app.folded.contains(&grp_idx),
        "a click past the panned fold field places the caret, not a fold"
    );

    click(&mut app, drawn_col);
    assert!(
        app.folded.contains(&grp_idx),
        "the panned marker must still toggle"
    );
}

/// Clicking a foldable node's fold marker shifts keyboard focus to the
/// main pane, as any main-pane click does, and puts the cursor on the
/// node it folded.
///
/// The cursor part reverses a 2026-07-18 decision that the marker was
/// a pure fold control and should leave the highlight alone. It was
/// wrong in the same way for the marker as for the keyboard: whatever
/// gesture reaches `toggle_fold`, the next keystroke should act on the
/// node whose shape just changed.
#[test]
fn clicking_the_fold_marker_focuses_the_main_pane_and_selects_the_node() {
    let (mut app, grp_idx) = group_type_fixture();
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 20);
    app.manage_open = true;
    app.manage_focus = true;
    let original_cursor = app.cursor;
    assert_ne!(original_cursor, grp_idx, "fixture sanity check");

    let line_idx = app.absolute_start(grp_idx);
    let indent_len = (app.document_lines()[line_idx].len()
        - app.document_lines()[line_idx].trim_start().len()) as u16;
    let marker_col = indent_len + 1;

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: marker_col,
        row: line_idx as u16,
        modifiers: KeyModifiers::NONE,
    });

    assert!(
        app.folded.contains(&grp_idx),
        "clicking the marker must still fold the node"
    );
    assert!(!app.manage_focus, "focus must shift to the main pane");
    assert_eq!(
        app.cursor, grp_idx,
        "the cursor follows the fold it just closed"
    );
}

/// A fold marker is a control, so double-clicking it toggles twice —
/// landing back where it started — and must *not* also open the
/// override/manage panes the way a double-click on the node's body
/// does. Each click has already spent itself on the fold, so it is
/// never half of a pair.
#[test]
fn double_click_on_the_fold_marker_toggles_twice_and_opens_nothing() {
    let (mut app, grp_idx) = group_type_fixture();
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 20);
    app.term_width = 120;

    let line_idx = app.absolute_start(grp_idx);
    let indent_len = (app.document_lines()[line_idx].len()
        - app.document_lines()[line_idx].trim_start().len()) as u16;
    let marker_col = indent_len + 1;

    for _ in 0..2 {
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.handle_mouse(MouseEvent {
                kind,
                column: marker_col,
                row: line_idx as u16,
                modifiers: KeyModifiers::NONE,
            });
        }
    }

    assert!(
        !app.folded.contains(&grp_idx),
        "two clicks on the marker toggle twice, back to where they started"
    );
    assert!(
        app.override_target.is_none(),
        "the marker must not open the override selection pane"
    );
    assert!(
        !app.manage_open,
        "nor the manage pane — that is the body's double-click"
    );
}

/// Spec 0147 G1: `main_area` excludes the main pane's own local
/// statusline row, so a click on that row is not mistaken for a
/// click on content row 0 — the cursor must not move.
#[test]
fn click_on_the_main_panes_own_statusline_row_is_not_treated_as_content_row_0() {
    let (mut app, _inner_idx, _id_idx) = type_as_fixture();

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    let statusline_row = app.main_area.y + app.main_area.height;
    let cursor_before = app.cursor;
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: app.main_area.x,
        row: statusline_row,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: app.main_area.x,
        row: statusline_row,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.cursor, cursor_before,
        "click on the local statusline row must not move the cursor"
    );
}

/// Spec 0194 test-plan item 15 (S7). A click is a caret placement, so
/// it has to invert S1's column-to-screen mapping — and the two
/// exceptions have to survive it: the fold marker stays a pure fold
/// control that leaves the caret alone, and the gutter clamps onto the
/// first non-blank instead of being rejected.
#[test]
fn a_click_places_the_caret_except_on_the_fold_marker() {
    let (mut app, items) = repeated_message_fixture();
    app.splash = false;
    app.main_area = Rect::new(0, 0, 60, 20);

    let header = app.absolute_start(items[1]);
    let row = app
        .visible_row_of_line(header)
        .expect("the header row is visible") as u16;
    let indent =
        app.document_lines()[header].len() - app.document_lines()[header].trim_start().len();
    // The heat-cue column, then the fold field, then the row's text.
    let text_origin = 1 + render::FOLD_FIELD_WIDTH as u16;

    app.handle_click(text_origin + indent as u16 + 2, row);
    assert_eq!(app.cursor, items[1], "the click moved the cursor there");
    assert_eq!(app.cursor_column, indent + 2, "and set the column");
    assert_eq!(app.desired_column, indent + 2);

    app.handle_click(0, row);
    assert_eq!(
        app.cursor_column, indent,
        "a click in the gutter clamps to the first non-blank"
    );

    let marker = render::marker_column(&app.document_lines()[header]);
    app.handle_click(marker + 1, row);
    assert!(
        app.folded.contains(&items[1]),
        "a click on the marker still folds"
    );
    assert_eq!(
        app.cursor_column, indent,
        "and leaves the caret exactly where it was"
    );

    // The blank column `fold_margin` draws beside the marker is part of
    // the same control: a one-cell target is too small to click twice in
    // a row without drifting off it.
    app.handle_click(marker + 2, row);
    assert!(
        !app.folded.contains(&items[1]),
        "the whole two-column fold field toggles, not just the glyph"
    );
    assert_eq!(app.cursor_column, indent, "and still not the caret");
}
