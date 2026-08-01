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
/// highlight, so this checks `scroll_offset`/`override_scroll` — a
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
    app.override_scroll = 0;
    app.scroll_offset = 0;

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 5,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.scroll_offset, WHEEL_PAN_STEP,
        "hovering the main pane must pan it, even though the side \
         pane still has keyboard focus"
    );
    assert_eq!(
        app.override_scroll, 0,
        "the unhovered side pane must not react"
    );

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 65,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.scroll_offset, WHEEL_PAN_STEP,
        "hovering the side pane must not pan the main pane"
    );
    assert_eq!(
        app.override_scroll, WHEEL_PAN_STEP,
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

    let scroll_before = app.scroll_offset;
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
        app.scroll_offset, scroll_before,
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

/// Spec 0129 §G1/§G3: click-drag across N main-pane rows, release —
/// selects the whole `[start, end]` range in document order, and
/// `render` highlights every row in that range with `REVERSED`.
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
    assert_eq!(app.select_anchor, Some(1));
    assert_eq!(app.select_end, Some(1));

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 0,
        row: 3,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.select_anchor, Some(1));
    assert_eq!(app.select_end, Some(3));

    let (count, text) = app.selected_text().expect("selection must be active");
    assert_eq!(count, 3);
    assert_eq!(text, "beta: 2\ngamma: 3\ndelta: 4");

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 0,
        row: 3,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.select_anchor,
        Some(1),
        "selection persists after release"
    );
    assert_eq!(app.select_end, Some(3));
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
        column: 0,
        row: 2,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 0,
        row: 2,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.select_anchor, Some(0));
    assert_eq!(app.select_end, Some(2));

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
    assert_eq!(app.select_end, None);
}

/// Feedback (2026-07-15): double-clicking a main-pane line (two
/// `Down`+`Up` clicks on the same line, in quick succession)
/// explicitly selects that line for copy. Crossterm reports `Down`
/// identically for single and double clicks, so this exercises the
/// app's own timestamp/position-based disambiguation
/// (`App::last_click`/`pending_double_click`). Since item 3 (spec
/// 0139 follow-up), the same double-click also acts as the `t`/`o`
/// smart proxy — this fixture's cursor node is the seeded root
/// override's own target (active by construction), so the second
/// click also opens the management pane; the copy check below uses
/// `copy_selection_to_clipboard` directly rather than dispatching
/// `Ctrl-C` through `handle_key`, since that now routes to the
/// (focused) management pane instead of the main pane.
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

    assert_eq!(app.select_anchor, Some(0), "double-click selects the line");
    assert_eq!(app.select_end, Some(0));
    assert!(
        app.message.is_empty(),
        "unexpected message: {}",
        app.message
    );
    assert!(
        app.manage_open,
        "double-click on the overridden root node also opens the management pane"
    );

    let (count, text) = app.selected_text().expect("selection must be active");
    assert_eq!(count, 1);
    assert_eq!(text, "alpha: 1");

    app.copy_selection_to_clipboard();
    assert!(
        app.message == "1 line(s) copied to clipboard"
            || app.message == "1 line(s) copied to clipboard (OSC 52 fallback)",
        "unexpected message: {}",
        app.message
    );
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
        column: 0,
        row: 2,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.select_anchor, Some(2));
    assert_eq!(app.select_end, Some(0));

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
    assert_eq!(app.select_anchor, Some(0));
    assert_eq!(app.select_end, Some(0));
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
        column: 0,
        row: 2,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 0,
        row: 2,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.select_anchor, Some(0));
    assert_eq!(app.select_end, Some(2));

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 1,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.select_anchor,
        Some(1),
        "a fresh click replaces the old selection"
    );
    assert_eq!(app.select_end, Some(1));

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.select_anchor, None, "Esc clears the selection");
    assert_eq!(app.select_end, None);
}

/// Regression test: clicking a foldable node's `▸`/`▾` marker must
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

/// Regression test (2026-07-18 feedback): clicking a foldable node's
/// fold marker must shift keyboard focus to the main pane (as any
/// main-pane click does) but must NOT move the cursor/highlight —
/// the marker is a pure fold control, distinct from clicking the row
/// itself (which does move the cursor, unchanged).
#[test]
fn clicking_the_fold_marker_focuses_the_main_pane_without_moving_the_cursor() {
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
        app.cursor, original_cursor,
        "the cursor/highlight must not move on a marker click"
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
}
