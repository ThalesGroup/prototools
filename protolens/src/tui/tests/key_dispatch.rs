// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::super::*;
use super::support::*;

/// Spec 0236 S20: `q` is bound to nothing at all — not in the main
/// pane, not on an empty tree, not in the help overlay — and `:quit`
/// (or any unambiguous prefix of it) is the only way out. The
/// `q`-then-`q` confirmation this replaced existed only because a
/// single keystroke was one slip from an accidental exit.
#[test]
fn q_no_longer_quits_and_quit_is_reachable_only_as_a_command() {
    let mut app = message_node_app();
    app.splash = false;

    for _ in 0..2 {
        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!app.should_quit);
    }

    app.help_open = true;
    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(!app.should_quit);
    assert!(app.help_open, "q must not close the help overlay either");
    app.help_open = false;

    let mut empty = empty_app();
    empty.splash = false;
    empty.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(!empty.should_quit);

    app.run_command("q");
    assert!(app.should_quit);
}

/// Spec 0236 S23: `:help` opens the same overlay `F1` does — the one
/// binding a newcomer cannot guess is the one that lists the others.
#[test]
fn help_command_opens_the_help_overlay() {
    let mut app = message_node_app();
    app.splash = false;

    app.run_command("help");
    assert!(app.help_open);
}

/// Spec 0113 D31: `Ctrl-Z` sets `should_suspend` (the actual
/// `SIGTSTP`/terminal dance lives in `run_loop`/`suspend`, outside
/// `App`'s own unit-testable surface) — checked centrally, so it
/// fires uniformly regardless of what has focus.
#[test]
#[cfg(unix)]
fn ctrl_z_sets_should_suspend() {
    let mut app = empty_app();
    app.splash = false;

    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
    assert!(app.should_suspend);
    assert!(!app.should_quit);
}

/// The splash screen is transparent to keyboard input (spec 0113 D22
/// amendment): the very first keypress both dismisses it and is
/// processed as a real command, rather than being swallowed.
#[test]
fn splash_dismissing_keypress_is_also_processed_as_a_command() {
    let mut app = message_node_app();
    assert!(app.splash);
    app.term_width = 120;

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(!app.splash);
    assert_eq!(app.override_target, Some(0));
}

/// `F1` opens the help overlay; `Esc` or `F1` closes it — `?` is
/// no longer bound to help, since it now belongs to in-pane search,
/// and `q` no longer is either (spec 0236 S20).
#[test]
fn f1_opens_and_closes_the_help_overlay() {
    let mut app = message_node_app();
    app.splash = false;

    app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(app.help_open);

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.help_open);

    app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(app.help_open);
    app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(!app.help_open);
}

/// Spec 0126 G1: `F1` opens the help overlay regardless of what
/// currently has focus — `manage_focus`, `override_focus`, and an
/// open `command_buffer` all used to swallow it before reaching the
/// main match arm's own `F1` handling.
#[test]
fn f1_opens_help_regardless_of_focus() {
    let mut app = message_node_app();
    app.splash = false;

    app.manage_focus = true;
    app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(app.help_open);
    app.help_open = false;

    app.override_focus = true;
    app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(app.help_open);
    app.help_open = false;
    app.override_focus = false;

    app.command_buffer = Some(String::new());
    app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(app.help_open);
}

/// Spec 0185 S5/Q2: `Tab` does not leave the override *selection* pane
/// — focus is locked to it, which is what keeps the preview overlay's
/// row anchor valid. The management pane keeps `Tab`: it splices for
/// real, so its main-pane content is committed content and nothing
/// there depends on an immutable anchor.
#[test]
fn tab_is_locked_out_of_the_override_pane_but_not_the_manage_pane() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(app.override_focus);

    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert!(app.override_focus, "Tab must not leave the selection pane");
    assert_eq!(app.override_target, Some(0), "...and the pane stays open");

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.manage_open);
    assert!(app.manage_focus);
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert!(!app.manage_focus, "the manage pane does not get the lock");
}

/// Item 3 (spec 0139 follow-up): `Enter` on a main-pane node with an
/// applicable override (active, here — the fixture's seeded root
/// entry) opens the management pane, same as pressing `o` directly.
#[test]
fn enter_opens_the_management_pane_when_an_override_applies() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.manage_open);
    assert!(app.manage_focus);
    assert_eq!(app.override_target, None);
}

/// Item 3 (spec 0139 follow-up): `Enter` on a main-pane node with
/// neither an active nor an applicable-inactive override opens the
/// selection pane instead, same as pressing `t` directly.
#[test]
fn enter_opens_the_override_pane_when_no_override_applies() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    while !app.overrides.entries().is_empty() {
        app.overrides.remove(0);
    }

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.override_focus);
    assert_eq!(app.override_target, Some(0));
    assert!(!app.manage_open);
}

/// Item 14 (2026-07-17 feedback): `Ctrl-Left`/`Ctrl-Right` pan the
/// override pane and the manage pane, mirroring the main pane's own
/// Ctrl-Left/Ctrl-Right (spec 0113 D24) and the mouse's Shift-wheel/
/// native horizontal-scroll pan already available for these panes
/// (`mouse.rs`'s
/// `override_and_manage_panes_pan_independently_of_the_main_pane`).
#[test]
fn ctrl_left_right_pan_the_override_and_manage_panes() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(app.override_focus);
    // 2026-07-19 feedback item 4: pan is now clamped on the right by
    // the widest visible row — set up a candidate list/pane width with
    // plenty of room to pan by a full `PAN_STEP` twice over, so the
    // clamp itself doesn't interfere with this test.
    app.override_candidates = vec![("cand.SomeVeryLongTypeNameHere".to_string(), None)];
    app.override_list_height = 5;
    app.side_area = Rect::new(0, 0, 5, 10);

    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
    assert_eq!(app.override_pan_offset, PAN_STEP);
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
    assert_eq!(app.override_pan_offset, 0);

    app.close_override();
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.manage_focus);
    app.manage_list_height = 5;
    app.side_area = Rect::new(0, 0, 5, 10);

    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
    assert_eq!(app.manage_pan_offset, PAN_STEP);
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
    assert_eq!(app.manage_pan_offset, 0);
}

/// 2026-07-19 feedback items 1/2: `Ctrl-Up`/`Ctrl-Down` pan the override
/// pane's candidate list vertically without moving the highlight,
/// bounded only by the content itself — no longer tied to keeping the
/// highlighted row in view.
///
/// Spec 0244 test-plan item 7 (G2): the side panes run past both ends by
/// the same rule as the main pane, leaving one row of content on screen.
#[test]
fn override_pane_pans_past_both_ends() {
    let mut app = message_node_app();
    app.splash = false;
    app.override_focus = true;
    app.override_target = Some(0);
    app.override_candidates = (0..30).map(|i| (format!("cand.Type{i}"), None)).collect();
    app.override_list_height = 5;
    app.override_highlight = 19;
    app.override_scroll.index = 15;
    let max_top = 30 - 1;
    let min_top = 1 - 5;

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL));
    assert_eq!(
        app.override_scroll.top(&FLAT_ROWS),
        (15 + PAN_STEP as isize).min(max_top),
        "Ctrl-Down must pan toward the content's own bottom edge"
    );
    assert_eq!(
        app.override_highlight, 19,
        "panning must not move the highlight"
    );

    // Spec 0286: the last candidate is a wall now, so reaching the
    // over-pan past it takes leaning rather than one more key.
    pan_to_the_bound(&mut app, Pannable::Override, true);
    assert_eq!(
        app.override_scroll.top(&FLAT_ROWS),
        max_top,
        "the last candidate alone on the pane's first row, and no further"
    );

    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL));
    assert_eq!(
        app.override_scroll.top(&FLAT_ROWS),
        max_top - PAN_STEP as isize
    );
    assert_eq!(app.override_highlight, 19);

    app.override_scroll = PaneScroll::default();
    pan_to_the_bound(&mut app, Pannable::Override, false);
    assert_eq!(
        app.override_scroll.top(&FLAT_ROWS),
        min_top,
        "the first candidate alone on the pane's last row, and no further"
    );
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL));
    assert_eq!(
        app.override_scroll.top(&FLAT_ROWS),
        min_top,
        "already at the top bound"
    );
}

/// 2026-07-19 feedback items 1/2: `Ctrl-Up`/`Ctrl-Down` pan the manage
/// pane's list vertically without moving the highlight, bounded only by
/// the content itself — no longer tied to keeping the highlighted row in
/// view. Uses 30 distinct-origin entries (each gets its own `Header`
/// row, spec 0117 §3 amendment).
///
/// Spec 0244 test-plan item 7 (G2), the manage-pane half.
#[test]
fn manage_pane_pans_past_both_ends() {
    let mut app = message_node_app();
    app.splash = false;
    app.manage_open = true;
    app.manage_focus = true;
    for field in 1..=30 {
        app.overrides.activate(
            OverrideOrigin::PathField {
                path: "/".to_string(),
                field,
            },
            None,
        );
    }
    let target_field = 15;
    let target_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| {
            e.origin
                == OverrideOrigin::PathField {
                    path: "/".to_string(),
                    field: target_field,
                }
        })
        .unwrap();
    app.manage_highlight = target_idx;
    app.manage_list_height = 5;
    app.manage_scroll.index = 10;
    let total_rows = app.manage_display_rows().len();
    let max_top = total_rows as isize - 1;
    let min_top = 1 - 5;

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL));
    assert_eq!(
        app.manage_scroll.top(&FLAT_ROWS),
        (10 + PAN_STEP as isize).min(max_top),
        "Ctrl-Down must pan toward the content's own bottom edge"
    );
    assert_eq!(
        app.manage_highlight, target_idx,
        "panning must not move the highlight"
    );

    app.manage_scroll.set_top(max_top, &FLAT_ROWS);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL));
    assert_eq!(
        app.manage_scroll.top(&FLAT_ROWS),
        max_top,
        "the last row alone on the pane's first row, and no further"
    );

    app.manage_scroll = PaneScroll::default();
    // Spec 0286: the first row is a wall, and the over-pan is past it.
    pan_to_the_bound(&mut app, Pannable::Manage, false);
    assert_eq!(
        app.manage_scroll.top(&FLAT_ROWS),
        min_top,
        "the first row alone on the pane's last row, and no further"
    );
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL));
    assert_eq!(
        app.manage_scroll.top(&FLAT_ROWS),
        min_top,
        "already at the top bound"
    );
    assert_eq!(app.manage_highlight, target_idx);
}

/// Spec 0245 S2. A pan already sitting on its bound moves nothing and
/// says so, so `run_loop` owes it no frame — a wheel held against the
/// top of the document is the commonest way to fill the event queue
/// with input that changes the screen not at all.
///
/// The flag must also be *cleared* by a pan that does move, since
/// `run_loop` only resets it between dispatches: a stalled pan followed
/// by a live one has to redraw.
#[test]
fn a_pan_that_hit_its_bound_asks_for_no_frame() {
    let texts: Vec<String> = (0..40).map(|i| format!("f{i}: 0")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut app = sibling_leaves_app(&refs);
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 20);

    app.set_scroll_top(1 - 20);
    app.wheel_pan_up();
    assert!(app.event_changed_nothing, "already at the top bound");
    app.wheel_pan_down();
    assert!(!app.event_changed_nothing, "and this one did move");

    // The horizontal pan is bounded the same way, and no document here
    // is wide enough to pan at all.
    app.pan_left();
    assert!(app.event_changed_nothing, "already at column 0");

    // The side panes answer for themselves — they have their own
    // `PaneScroll` and their own wall, though they share the arithmetic
    // over both (`side_pan_vertical`).
    app.override_focus = true;
    app.override_target = Some(0);
    app.override_candidates = (0..30).map(|i| (format!("cand.Type{i}"), None)).collect();
    app.override_list_height = 5;
    app.override_scroll.set_top(1 - 5, &FLAT_ROWS);
    app.override_pan_vertical(WHEEL_PAN_STEP, true);
    assert!(app.event_changed_nothing);
    app.override_pan_vertical(WHEEL_PAN_STEP, false);
    assert!(!app.event_changed_nothing);
}

/// Spec 0244 test-plan item 8 (N2): an over-pan survives moving the
/// highlight. `clamp_scroll_to_visible` is a minimal nudge, not a
/// re-anchoring — it only brings the highlight back on screen, and the
/// blank rows that are not in its way stay where the pan put them.
#[test]
fn moving_the_highlight_pulls_the_viewport_no_further_than_needed() {
    let area = Rect::new(0, 0, 40, 6);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();

    let mut app = message_node_app();
    app.splash = false;
    app.override_focus = true;
    app.override_target = Some(0);
    app.override_candidates = (0..30).map(|i| (format!("cand.Type{i}"), None)).collect();
    // Four blank rows above candidate 0, which then sits on the pane's
    // last row — the top bound for a 5-row list.
    app.override_scroll.set_top(-4, &FLAT_ROWS);
    app.override_highlight = 0;
    app.last_override_highlight = None;
    terminal
        .draw(|f| app.render_override_pane(f, area))
        .unwrap();
    assert_eq!(app.override_list_height, 5);
    assert_eq!(
        app.override_scroll.top(&FLAT_ROWS),
        -4,
        "the highlight is already on screen, so nothing is owed"
    );

    app.override_highlight = 2;
    terminal
        .draw(|f| app.render_override_pane(f, area))
        .unwrap();
    assert_eq!(
        app.override_scroll.top(&FLAT_ROWS),
        -2,
        "candidate 2 comes to the last row; the other two blanks stay"
    );

    let mut app = message_node_app();
    app.splash = false;
    app.manage_open = true;
    app.manage_focus = true;
    for field in 1..=30 {
        app.overrides.activate(
            OverrideOrigin::PathField {
                path: "/".to_string(),
                field,
            },
            None,
        );
    }
    app.manage_scroll.set_top(-4, &FLAT_ROWS);
    app.manage_highlight = 0;
    app.last_manage_highlight = None;
    terminal.draw(|f| app.render_manage_pane(f, area)).unwrap();
    assert_eq!(app.manage_list_height, 5);
    let first_row = app.manage_highlighted_row();
    assert_eq!(
        app.manage_scroll.top(&FLAT_ROWS),
        -4 + first_row as isize,
        "pulled down only by the header rows above the first entry"
    );
    assert!(
        app.manage_scroll.top(&FLAT_ROWS) < 0,
        "and the blank rows it did not need are still there"
    );
}

/// Spec 0144 G1/G2: `v` resolves the FQDN under focus from whichever
/// pane currently has it — the override candidate pane here — and
/// (with `DescriptorContext::empty_for_test()`'s empty pool) reports
/// G3's "unknown type" outcome.
#[test]
#[cfg(unix)]
fn v_in_override_pane_reports_unknown_type_for_unresolvable_candidate() {
    let mut app = message_node_app();
    app.splash = false;
    app.override_focus = true;
    app.override_target = Some(0);
    app.override_candidates = vec![("test.SomeType".to_string(), None)];
    app.override_highlight = 0;

    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(app.message, "unknown type: test.SomeType");
}

/// Spec 0144 G2: the `None` sentinel row (alphabetic-mode row 0) has
/// no declaration to jump to — `v` must not even attempt a lookup.
#[test]
#[cfg(unix)]
fn v_in_override_pane_is_a_no_op_for_the_none_sentinel() {
    let mut app = message_node_app();
    app.splash = false;
    app.override_focus = true;
    app.override_target = Some(0);
    app.override_candidates = vec![("none".to_string(), None)];
    app.override_highlight = 0;

    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(app.message, "no declaration to jump to here");
}

/// Spec 0144 G2: the manage pane's highlighted entry's own type is
/// resolved, independent of the override pane/cursor.
#[test]
#[cfg(unix)]
fn v_in_manage_pane_reports_unknown_type_for_the_highlighted_entry() {
    let mut app = message_node_app();
    app.splash = false;
    app.manage_open = true;
    app.manage_focus = true;
    app.overrides.activate(
        OverrideOrigin::PathField {
            path: "/".to_string(),
            field: 1,
        },
        Some("test.ManageType".to_string()),
    );
    app.manage_highlight = app
        .overrides
        .entries()
        .iter()
        .position(|e| e.r#type.as_deref() == Some("test.ManageType"))
        .unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(app.message, "unknown type: test.ManageType");
}

/// Spec 0144 G2: with neither pane focused, `v` falls back to the
/// main-pane cursor node's own type.
#[test]
#[cfg(unix)]
fn v_in_main_pane_reports_unknown_type_for_the_cursor_node() {
    let mut app = message_node_app();
    app.splash = false;

    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(app.message, "unknown type: google.protobuf.DescriptorProto");
}

/// Spec 0144 G2: a scalar main-pane node carries no `type_fqdn` at
/// all — `v` is a no-op, not a lookup failure, when it also has no
/// active override and no parent schema to fall back to (2026-07-18
/// fix: `fqdn_under_focus` now also consults those, mirroring
/// `status_type_label`, so an enum-typed scalar can resolve — see
/// `v_in_main_pane_resolves_an_enum_scalars_natural_type` below).
#[test]
#[cfg(unix)]
fn v_in_main_pane_is_a_no_op_for_a_scalar_node() {
    let mut app = sibling_leaves_app(&["field: 1"]);
    app.splash = false;
    // This fixture's single node sits at path "/" (root-level, no
    // parent) — drop the seeded root-type override entry first, or
    // it would accidentally match that node and mask the no-op case
    // this test targets.
    while !app.overrides.entries().is_empty() {
        app.overrides.remove(0);
    }

    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(app.message, "no declaration to jump to here");
}

/// Regression test (2026-07-18 feedback): `v` on an enum-typed scalar
/// field with no active override must resolve the field's own
/// natural enum type and attempt a lookup — previously
/// `fqdn_under_focus` read only `span.type_fqdn` (always `None` for
/// scalars) for the main-pane branch, so this always reported "no
/// declaration to jump to here" even though `status_type_label`
/// already resolved and displayed the same FQDN on the status line.
#[test]
#[cfg(unix)]
fn v_in_main_pane_resolves_an_enum_scalars_natural_type() {
    let (mut app, durability_idx) = enum_field_fixture();
    app.cursor = durability_idx;
    assert!(app.proto_root.is_none());

    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(
        app.message,
        "no proto root configured; set one with :proto-root <dir> or -I/--proto-root"
    );
}

/// Spec 0144 G2 (`fqdn_under_focus` doc comment): the internal,
/// non-real `decode::MESSAGE_SET_ITEM_FQDN` placeholder is never
/// registered as a real message — `v` must treat it the same as "no
/// type at all", not surface a confusing "unknown type" message.
#[test]
#[cfg(unix)]
fn v_is_a_no_op_for_the_internal_message_set_item_fqdn() {
    let mut app = message_set_fixture();
    let item_idx = node_with_type(&app, decode::MESSAGE_SET_ITEM_FQDN)
        .expect("fixture must contain a MessageSet Item node");
    app.cursor = item_idx;

    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(app.message, "no declaration to jump to here");
}

/// Spec 0144 G3/G4: a real, resolving FQDN (`type_as_fixture`'s
/// `test.Inner`) clears G3's own check, but with no `proto_root`
/// configured `v` stops at G4 with a clear message.
#[test]
#[cfg(unix)]
fn v_reports_missing_proto_root_when_type_resolves_but_none_is_configured() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    assert!(app.proto_root.is_none());

    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(
        app.message,
        "no proto root configured; set one with :proto-root <dir> or -I/--proto-root"
    );
    assert!(app.pending_editor_open.is_none());
}

/// Spec 0144 G4: a configured `proto_root` under which the resolved
/// file doesn't actually exist reports "proto source not found"
/// rather than silently arming the editor handoff.
#[test]
#[cfg(unix)]
fn v_reports_proto_source_not_found_when_the_file_is_missing_under_proto_root() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    let proto_root = std::env::temp_dir().join("protolens-v-test-missing-root");
    app.proto_root = Some(proto_root.clone());

    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(
        app.message,
        format!(
            "proto source not found: test_type_as.proto (under proto-root {})",
            proto_root.display()
        )
    );
    assert!(app.pending_editor_open.is_none());
}

/// Spec 0144 G1-G4: the full happy path — a resolving FQDN, a
/// configured `proto_root`, and the resolved `.proto` file actually
/// present — arms `pending_editor_open` (the fixture carries no
/// `source_code_info`, so `locate_declaration` falls back to line 1,
/// column 1 — see `neovim::locate_declaration`'s own doc comment).
#[test]
#[cfg(unix)]
fn v_arms_pending_editor_open_when_the_proto_source_is_found() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let proto_root = std::env::temp_dir().join(format!("protolens-v-test-root-{n}"));
    std::fs::create_dir_all(&proto_root).unwrap();
    let proto_path = proto_root.join("test_type_as.proto");
    std::fs::write(&proto_path, "").unwrap();
    app.proto_root = Some(proto_root.clone());

    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    std::fs::remove_dir_all(&proto_root).unwrap();

    let req = app
        .pending_editor_open
        .expect("must arm pending_editor_open");
    assert_eq!(req.path, proto_path);
    assert_eq!(req.line, 1);
    assert_eq!(req.col, 1);
}

/// Glitch reported 2026-07-18: a missing `nvim` binary must not crash
/// protolens — the failure is reported via `app.message` and the TUI
/// keeps running.
///
/// The absent binary is arranged by naming one, through
/// `neovim::EDITOR_PROGRAM`, rather than by hoping this machine has no
/// Neovim: the earlier version of this test probed `PATH` and returned
/// early when it found one, which meant it never ran anywhere Neovim
/// was installed — and spawning the real thing here would block on
/// `waitpid` forever.
#[test]
fn open_editor_reports_a_missing_nvim_instead_of_crashing() {
    neovim::EDITOR_PROGRAM.set("protolens-test-no-such-editor");
    let mut app = empty_app();
    // `open_editor` requires `io::Error: From<B::Error>` (it propagates
    // real I/O errors via `?`) — `TestBackend`'s `Error` is `Infallible`,
    // which doesn't convert, so a `CrosstermBackend` over an in-memory
    // buffer is used instead; it never touches a real terminal.
    let mut terminal = Terminal::new(CrosstermBackend::new(Vec::new())).unwrap();
    let req = neovim::EditorRequest {
        path: PathBuf::from("/tmp/protolens-test-does-not-exist.proto"),
        line: 1,
        col: 1,
    };
    // The return value itself isn't asserted: `enable_raw_mode_and_reenter`
    // (called on the way out, regardless of the branch below) talks to the
    // *real* process stdout/stdin, which isn't a tty under `cargo test`
    // and can legitimately fail here — a pre-existing, orthogonal
    // limitation of this sandboxed test run, not something Glitch 1's fix
    // is responsible for. What this test verifies is that the missing-
    // `nvim` spawn failure itself doesn't propagate as an `Err` (the
    // actual crash reported) but is instead converted to a message.
    let _ = neovim::open_editor(&mut terminal, &mut app, req);
    assert!(app.message.contains("cannot launch nvim"));
    assert!(matches!(app.editor_state, neovim::EditorState::NotRunning));
}

// ── Spec 0156 G3: the `x<b|p|d<b|p>>` export chord ─────────────────────────

/// `x` then `b`/`p` pre-fill `export --binary/--prototext <path>` and
/// open the command line.
#[test]
fn x_b_and_x_p_prefill_export_data_format_and_open_the_command_line() {
    for (key, flag) in [('b', "--binary"), ('p', "--prototext")] {
        let (mut app, _, _) = type_as_fixture();
        let expected_path = app.default_extract_path();
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(app.pending_x, ExportChord::Leader);
        app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
        assert_eq!(app.pending_x, ExportChord::None);
        assert_eq!(
            app.command_buffer.as_deref(),
            Some(format!("export {flag} {expected_path}").as_str())
        );
        assert_eq!(app.command_kind, CommandLineKind::Command);
    }
}

/// `x` then `d` then `b`/`p` pre-fill `export --descriptor-binary/
/// --descriptor-prototext <path>` and open the command line.
#[test]
fn x_d_b_and_x_d_p_prefill_export_descriptor_format_and_open_the_command_line() {
    for (key, flag) in [
        ('b', "--descriptor-binary"),
        ('p', "--descriptor-prototext"),
    ] {
        let (mut app, _, _) = type_as_fixture();
        let expected_path = app.default_export_descriptor_path();
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(app.pending_x, ExportChord::Descriptor);
        app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
        assert_eq!(app.pending_x, ExportChord::None);
        assert_eq!(
            app.command_buffer.as_deref(),
            Some(format!("export {flag} {expected_path}").as_str())
        );
        assert_eq!(app.command_kind, CommandLineKind::Command);
    }
}

/// `x` then any other key clears `pending_x` back to `None`, does not
/// open the command line, and still performs that key's normal action
/// (cursor moves for `j`).
#[test]
fn x_then_other_key_cancels_and_falls_through_unswallowed() {
    let (mut app, _, _) = type_as_fixture();
    let cursor_before = app.cursor;
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert_eq!(app.pending_x, ExportChord::Leader);
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(app.pending_x, ExportChord::None);
    assert!(app.command_buffer.is_none());
    assert_ne!(app.cursor, cursor_before, "the 'j' move must still fire");
}

/// `x` then `d` then any other key clears `pending_x` back to `None`,
/// does not open the command line, and still performs that key's
/// normal action.
#[test]
fn x_d_then_other_key_cancels_and_falls_through_unswallowed() {
    let (mut app, _, _) = type_as_fixture();
    let cursor_before = app.cursor;
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert_eq!(app.pending_x, ExportChord::Descriptor);
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(app.pending_x, ExportChord::None);
    assert!(app.command_buffer.is_none());
    assert_ne!(app.cursor, cursor_before, "the 'j' move must still fire");
}

/// `x` then `x` again: the chord cancels (no pre-fill), and
/// `pending_x` stays `None` afterward — it does not re-arm.
#[test]
fn x_then_x_cancels_without_prefill_and_does_not_rearm() {
    let (mut app, _, _) = type_as_fixture();
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert_eq!(app.pending_x, ExportChord::Leader);
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert_eq!(app.pending_x, ExportChord::None);
    assert!(app.command_buffer.is_none());
}

/// Plain `x` alone, or `x` then `d` alone (no follow-up key yet):
/// `command_buffer` stays `None`.
#[test]
fn bare_x_and_x_d_alone_leave_the_command_buffer_untouched() {
    let (mut app, _, _) = type_as_fixture();
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert!(app.command_buffer.is_none());

    let (mut app, _, _) = type_as_fixture();
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert!(app.command_buffer.is_none());
}

/// `default_export_descriptor_path` (spec 0156 G5): at the document
/// root, the segment is always `no range`.
#[test]
fn default_export_descriptor_path_uses_no_range_at_the_root() {
    let (app, _, _) = type_as_fixture();
    assert_eq!(app.cursor, app.first_node);
    assert!(app.default_export_descriptor_path().contains(".no range."));
}

/// An active rename override (spec 0119 §G4's `f`) on the cursor node
/// takes priority over both the `no range` root default and the
/// `short_type` suffix: the segment is the renamed field name alone,
/// e.g. `<stem>.exemplar.desc` rather than `<stem>.no range.Outer.desc`.
#[test]
fn default_export_descriptor_path_uses_an_active_rename_over_no_range() {
    let (mut app, _, _) = type_as_fixture();
    assert_eq!(app.cursor, app.first_node);
    app.run_command(&format!(
        "override {} --as test.Outer",
        app.positional_path(app.cursor)
    ));
    let entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| e.active && e.r#type.as_deref() == Some("test.Outer"))
        .expect("type-as on root must have created an active entry");
    app.overrides
        .rename(entry_idx, Some("exemplar".to_string()));
    let path = app.default_export_descriptor_path();
    assert!(
        path.ends_with(".exemplar.desc"),
        "expected the renamed field name to replace `no range.<Type>`, got: {path}"
    );
}

/// A cursor whose parent's schema resolves its field name uses that
/// name as the segment.
#[test]
fn default_export_descriptor_path_uses_the_resolvable_field_name() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.set_cursor(inner_idx);
    assert!(app.default_export_descriptor_path().contains(".inner."));
}

/// A non-root cursor with no resolvable schema field name falls back
/// to the numeric `<start>-<end>` range (same as `default_extract_path`).
#[test]
fn default_export_descriptor_path_falls_back_to_the_numeric_range_when_unresolvable() {
    // Two sibling varint scalar leaves, `parent: None` (so `parent_field`
    // can't resolve a schema field name), each with a real, in-bounds
    // `raw_range` into a real blob (`display_range`'s fallback path
    // parses the wire tag at `raw_range.start`, so an empty/synthetic
    // blob would panic).
    // Spec 0216: the two records are two top-level slots of the arena,
    // hence two roots, and the overlay carries no links at all.
    let blob = vec![0x08u8, 0x05, 0x10u8, 0x07];
    let make_node = |field_number: u32, raw_range: std::ops::Range<u32>| TreeNode {
        span: NodeSpan {
            field_number,
            raw_range,
            text_range: field_number - 1..field_number,
            level: 0,
            type_fqdn: NO_FQDN,
            is_message: false,
            packed_record_start: NO_PACKED_RECORD,
            wire_type: WT_VARINT as u8,
        },
        lines_total: 1,
        lines_visible: 1,
        rendered_as: NOT_RENDERED,
    };
    let tree = vec![make_node(1, 0..2), make_node(2, 2..4)];
    let decoded = Decoded {
        total_lines: 2,
        // Spec 0257 S1: a hand-built document was never bounded.
        stops: Vec::new(),
        row_budget: None,
        node_text: vec![Some(Box::from("a")), Some(Box::from("b"))],
        tree,
        root_type: "google.protobuf.FileDescriptorProto".to_string(),
        arena: crate::decode::arena_of(&blob),
        blob: Arc::new(Blob::unwrapped(blob)),
        wrapper_offset: 0,
        root_candidates: Vec::new(),
        fqdns: FqdnTable::new(),
    };
    let mut app = app_named(decoded, DescriptorContext::empty_for_test(), "test.pb");
    app.set_cursor(1);
    assert_ne!(app.cursor, app.first_node);
    let path = app.default_export_descriptor_path();
    assert!(
        !path.contains("no range") && !path.contains(".inner."),
        "expected a numeric range segment, got: {path}"
    );
}

/// Spec 0194 test-plan item 17 (S6), amended by spec 0199 S9. The whole
/// reassignment, asserted as a table: the unshifted keys move the caret
/// in the text, the shifted ones widen a fold to the sibling level, and
/// folding also lives on `Space` and the `z` chords. Each reclaimed key
/// is also checked for *not* doing what it used to.
#[test]
fn the_reassigned_keys_dispatch_where_the_table_says() {
    let plain = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    let (mut app, items) = repeated_message_fixture();
    app.splash = false;
    app.set_cursor(items[1]);
    let base = app.cursor_column;

    // `l`/`h` belong to the caret now. They used to enter the first
    // child and fold the node.
    app.handle_key(plain('l'));
    assert_eq!(app.cursor, items[1], "`l` no longer enters the first child");
    assert_eq!(app.cursor_column, base + 1);
    app.handle_key(plain('h'));
    assert_eq!(app.cursor_column, base);
    assert!(app.folded.is_empty(), "`h` no longer folds");

    // `$` and `0` are the two ends of the same row.
    app.handle_key(plain('$'));
    assert_eq!(app.cursor_column, app.caret_bounds().1);
    app.handle_key(plain('0'));
    assert_eq!(app.cursor_column, base);
    app.handle_key(plain('$'));
    app.handle_key(plain('^'));
    assert_eq!(app.cursor_column, base, "`^` is `0`'s twin, not a variant");

    // The sibling-wide fold (spec 0199 S9) moved onto `Control` when
    // spec 0242 S8 gave the shifted pair to the selection. It used to
    // be parent/first-child motion, which `h`/`l` absorbed.
    let ctrl = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
    app.handle_key(ctrl('h'));
    assert!(
        items.iter().all(|i| app.folded.contains(i)),
        "`Ctrl-h` folds the whole sibling level"
    );
    assert_eq!(app.cursor, items[1], "and moves no cursor");
    app.handle_key(ctrl('l'));
    assert!(
        items.iter().all(|i| !app.folded.contains(i)),
        "`Ctrl-l` unfolds it again"
    );
    assert_eq!(app.cursor, items[1]);

    // And the shifted pair selects instead, folding nothing.
    app.handle_key(plain('H'));
    assert!(app.selection_span().is_some(), "`H` selects");
    assert!(app.folded.is_empty(), "and folds nothing");
    app.handle_key(plain('L'));
    assert!(app.folded.is_empty(), "nor does `L`");
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    // `z` acts on its own now — it was a chord prefix (`za`/`zc`/`zo`
    // and their sibling-wide capitals) and is a one-key toggle.
    app.handle_key(plain('z'));
    assert!(app.folded.contains(&items[1]), "`z` alone toggles");
    app.handle_key(plain('z'));
    assert!(!app.folded.contains(&items[1]), "and toggles back");
}

/// `Space`/`f` page down and `Shift-Space`/`b` page up, each landing
/// exactly where the literal `PageDown`/`PageUp` key does.
///
/// `b` is the one that has to work: a terminal that does not report
/// modifiers on printable keys delivers `Shift-Space` as a bare `Space`,
/// which pages the wrong way, and a compact keyboard may have no
/// `PageUp` key at all.
#[test]
fn space_and_f_page_down_shift_space_and_b_page_up() {
    let paged = |keys: &[KeyEvent]| {
        let mut app = wide_sibling_scalars_app(200);
        app.splash = false;
        app.main_area = Rect::new(0, 0, 40, 20);
        for &k in keys {
            app.handle_key(k);
        }
        app.cursor
    };

    let page_down = KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE);
    let page_up = KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE);
    let down = paged(&[page_down]);
    assert!(down > 0, "a page down must move the cursor at all");

    for k in [
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
    ] {
        assert_eq!(paged(&[k]), down, "{k:?} must page down");
    }

    // Two pages down, then one back up — a single page up from the top
    // would have nowhere to go and prove nothing.
    let up = paged(&[page_down, page_down, page_up]);
    assert!(
        up > 0 && up < down * 2,
        "a page up must move back, not home"
    );

    for k in [
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::SHIFT),
        KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
    ] {
        assert_eq!(paged(&[page_down, page_down, k]), up, "{k:?} must page up");
    }
}

/// Spec 0236 G5: `f`/`b` page in the override pane, the manage pane
/// and the help overlay too, matching the main pane above. Freeing `f`
/// in the manage pane — it used to open the rename buffer — is what
/// made the set uniform.
#[test]
fn f_and_b_page_in_every_pane() {
    let f = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE);
    let b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE);

    // Override pane: `t` opens it on a field with a candidate list.
    let (mut app, _inner_idx, id_idx) = type_as_fixture();
    app.splash = false;
    app.cursor = id_idx;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(app.override_focus);
    assert!(app.override_candidates.len() > 1);
    let start = app.override_highlight;
    app.handle_key(f);
    assert_ne!(app.override_highlight, start, "`f` pages the candidates");
    app.handle_key(b);
    assert_eq!(app.override_highlight, start, "and `b` pages back");

    // Manage pane.
    let (mut app, _items) = repeated_message_fixture();
    app.splash = false;
    app.manage_open = true;
    app.manage_focus = true;
    for field in [1u64, 2] {
        app.overrides.activate(
            OverrideOrigin::PathField {
                path: "/".to_string(),
                field,
            },
            None,
        );
    }
    app.manage_highlight = 0;
    app.handle_key(f);
    assert_ne!(app.manage_highlight, 0, "`f` pages the entries");
    app.handle_key(b);
    assert_eq!(app.manage_highlight, 0, "and `b` pages back");

    // Help overlay.
    let mut app = empty_app();
    app.splash = false;
    app.help_open = true;
    app.handle_key(f);
    assert!(app.help_scroll > 0, "`f` scrolls the help");
    app.handle_key(b);
    assert_eq!(app.help_scroll, 0, "and `b` scrolls back");
}

/// `Ctrl-N`/`Ctrl-P` are `Down`/`Up` in every pane that has a
/// `Down`/`Up` — Emacs' own next/previous-line, and the pair a reader
/// reaches for when the arrow keys are inconvenient.
#[test]
fn ctrl_n_and_ctrl_p_alias_down_and_up_in_every_pane() {
    let ctrl_n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
    let ctrl_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);

    // Main pane.
    let (mut app, items) = repeated_message_fixture();
    app.splash = false;
    app.set_cursor(items[0]);
    app.handle_key(ctrl_n);
    let after_down = app.cursor;
    assert_ne!(after_down, items[0], "Ctrl-N moves down the document");
    app.handle_key(ctrl_p);
    assert_eq!(app.cursor, items[0], "and Ctrl-P moves back up");

    // Override pane: `t` opens it on a field with a candidate list.
    let (mut app, _inner_idx, id_idx) = type_as_fixture();
    app.splash = false;
    app.cursor = id_idx;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(app.override_focus);
    assert!(app.override_candidates.len() > 1);
    let start = app.override_highlight;
    app.handle_key(ctrl_n);
    assert_eq!(app.override_highlight, start + 1);
    app.handle_key(ctrl_p);
    assert_eq!(app.override_highlight, start);

    // Manage pane.
    let (mut app, _items) = repeated_message_fixture();
    app.splash = false;
    app.manage_open = true;
    app.manage_focus = true;
    for field in [1u64, 2] {
        app.overrides.activate(
            OverrideOrigin::PathField {
                path: "/".to_string(),
                field,
            },
            None,
        );
    }
    app.manage_highlight = 0;
    app.handle_key(ctrl_n);
    assert_eq!(app.manage_highlight, 1);
    app.handle_key(ctrl_p);
    assert_eq!(app.manage_highlight, 0);

    // Help overlay.
    let mut app = empty_app();
    app.splash = false;
    app.help_open = true;
    app.handle_key(ctrl_n);
    assert_eq!(app.help_scroll, 1);
    app.handle_key(ctrl_p);
    assert_eq!(app.help_scroll, 0);
}

/// The main pane's `Control`/`Alt` vocabulary is exactly what
/// `handle_key`'s gate spells out, and nothing else.
///
/// Every plain-character arm in that match carries no modifier condition
/// of its own, so before the gate existed a `Char('q')` arm answered
/// `Ctrl-Q` too, and the whole lower-case alphabet was live under both
/// modifiers. This walks every letter and every plainly-bound
/// punctuation key under `Control` and under `Alt`, and asserts the
/// unbound ones leave the app exactly as they found it.
#[test]
fn only_the_bound_ctrl_and_alt_chords_do_anything_in_the_main_pane() {
    // Enough of the observable state that any plain-key arm leaking
    // through the gate would move one of these.
    fn state(app: &App) -> String {
        format!(
            "{} {} {} {} {} {:?} {} {} {} {} {} {} {:?}",
            app.cursor,
            app.cursor_column,
            app.cursor_line_in_node,
            app.folded.len(),
            app.annotations,
            app.wire,
            app.heat_cues_hidden,
            app.override_target.is_some(),
            app.manage_open,
            app.command_buffer.is_some(),
            app.should_quit,
            app.help_open,
            app.message,
        )
    }

    // `Ctrl-Z` is absent: it is the suspend key, handled at the very top
    // of `handle_key` and never reaching the gate.
    let bound = [
        (KeyModifiers::CONTROL, "fbnpaeoichjkl"),
        (KeyModifiers::ALT, "hlbf"),
    ];
    // The letters, plus every punctuation key the pane binds plainly.
    let keys: Vec<char> = ('a'..='z').chain(" :/?$%^0gGtwxWHLZ".chars()).collect();

    for (modifier, live) in bound {
        for c in &keys {
            if live.contains(*c) {
                continue;
            }
            let (mut app, items) = repeated_message_fixture();
            app.splash = false;
            app.set_cursor(items[1]);
            // Every keypress clears a stale notice before its own handler
            // runs (spec 0147 G5), and the fixture starts with a startup
            // warning in there — so clear it first, or the comparison is
            // against a message no key could have left standing.
            app.message.clear();
            let before = state(&app);
            app.handle_key(KeyEvent::new(KeyCode::Char(*c), modifier));
            assert_eq!(
                state(&app),
                before,
                "{modifier:?}-{c} is not bound and must do nothing"
            );
        }
    }
}

/// The same, for the manage pane — the surface where an ungated chord
/// costs the most: `Ctrl-D` would delete the highlighted entry outright
/// and `Ctrl-Q` close the pane.
#[test]
fn only_the_bound_ctrl_and_alt_chords_do_anything_in_the_manage_pane() {
    fn state(app: &App) -> String {
        format!(
            "{} {} {} {} {} {:?} {:?}",
            app.manage_highlight,
            app.manage_open,
            app.manage_focus,
            app.cursor,
            app.command_buffer.is_some(),
            app.overrides.entries(),
            app.message,
        )
    }

    let keys: Vec<char> = ('a'..='z').chain(" /?gGADZ".chars()).collect();
    for modifier in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
        for c in &keys {
            // The pane's whole Ctrl/Alt vocabulary.
            if modifier == KeyModifiers::CONTROL && "np".contains(*c) {
                continue;
            }
            let (mut app, _items) = repeated_message_fixture();
            app.splash = false;
            app.manage_open = true;
            app.manage_focus = true;
            for field in [1u64, 2] {
                app.overrides.activate(
                    OverrideOrigin::PathField {
                        path: "/".to_string(),
                        field,
                    },
                    None,
                );
            }
            app.manage_highlight = 1;
            app.message.clear();
            let before = state(&app);
            app.handle_key(KeyEvent::new(KeyCode::Char(*c), modifier));
            assert_eq!(
                state(&app),
                before,
                "{modifier:?}-{c} is not bound and must do nothing"
            );
        }
    }
}

/// Spec 0194 test-plan item 17, search half. `p` was spec 0195's
/// "previous match" and is gone; `n` and `N` are the two directions,
/// as vim has them.
#[test]
fn n_and_shift_n_repeat_the_last_search_in_both_directions() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "beta: 3"]);
    app.splash = false;
    app.last_search = Some((SearchDir::Forward, "beta".to_string()));

    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    assert_eq!(app.cursor, 1);
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    assert_eq!(app.cursor, 2);
    app.handle_key(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::NONE));
    assert_eq!(app.cursor, 1, "`N` runs the same pattern backwards");
}

/// Spec 0194 test-plan item 19 (S10). A jumplist entry is a caret
/// position, not a node: `Ctrl-o` has to put back the column and the
/// header/footer side as well, and clamp them if the row it returns to
/// has shrunk in the meantime.
#[test]
fn ctrl_o_restores_the_whole_caret_position() {
    let ctrl_o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
    let (mut app, items) = repeated_message_fixture();
    app.splash = false;

    app.set_cursor(items[0]);
    app.cursor_line_in_node = app.tree[items[0]].lines_total - 1;
    app.cursor_column = app.caret_bounds().1;
    let was = (app.cursor, app.cursor_on_footer(), app.cursor_column);

    app.record_jump();
    app.set_cursor(items[2]);
    app.handle_key(ctrl_o);
    assert_eq!(
        (app.cursor, app.cursor_on_footer(), app.cursor_column),
        was,
        "the node, the footer side and the column all come back"
    );

    // The same jump, to a row that has lost most of its text since.
    app.cursor_line_in_node = 0;
    app.cursor_column = app.caret_bounds().1;
    let wide = app.cursor_column;
    app.record_jump();
    app.set_cursor(items[2]);
    app.node_text_mut()[items[0]] = Some(Box::from("x"));

    app.handle_key(ctrl_o);
    assert_eq!(app.cursor, items[0]);
    assert!(
        wide > 0 && app.cursor_column < wide,
        "the column was clamped"
    );
    assert_eq!(
        app.cursor_column,
        app.caret_bounds().1,
        "onto the shrunken row's last reachable column"
    );
}
