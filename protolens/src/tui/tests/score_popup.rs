// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0280: the box a heat cue's number can be asked for.

use super::super::score_popup::{Breakdown, HOVER_DWELL};
use super::super::*;
use super::heat_cue::seed_range_heat_entry;
use super::support::*;
use crate::override_pane::{inferred_breakdown, inferred_score};

/// An app whose node 0 shows a cue, sized so its header row is the main
/// pane's own first row at `(0, 0)`.
///
/// The seeding is `i_toggles_heat_cues_hidden`'s: a `RangeHeatEntry`
/// planted straight into `heat_caches`, which is what lets a cue exist
/// without a scoring graph behind it.
fn cue_app() -> App {
    let mut app = message_node_app();
    app.splash = false;
    let range = extract::message_payload_range(&app.blob, &app.tree[0].span.raw_range);
    seed_range_heat_entry(
        &mut app,
        range.start,
        Some(50),
        1,
        "google.protobuf.DescriptorProto",
        Some(10),
    );
    app.main_area = Rect::new(0, 0, 80, 22);
    app
}

/// An app whose rows carry `#@` annotations declaring a type, sized so
/// that row 0 is the main pane's own first row at `(0, 0)`.
///
/// Two rows because the target is the `type` production and not the
/// declaration: row 1 carries both of the things that sit next to a
/// type name in one — a label in front of it and an enum's value behind
/// it — and neither is part of the target.
fn type_row_app() -> App {
    let mut app = sibling_leaves_app(&["x: 1  #@ int32 = 1", "y: 2  #@ repeated Color(5) = 2"]);
    app.splash = false;
    app.main_area = Rect::new(0, 0, 80, 22);
    app
}

/// The pane column of the first character of `needle` as row `line` is
/// drawn — fold margin, indentation and all.
fn column_of(app: &App, line: usize, needle: &str) -> u16 {
    let content = app.row_content(app.committed_row(line).expect("a drawn row"));
    let at = content.find(needle).expect("the row draws it");
    // Column 0 of the pane is the heat glyph's reserved gutter.
    1 + content[..at].chars().count() as u16
}

fn moved(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Moved,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// Spec 0280 test plan 1 / S10: the annotation's type name is a target
/// and nothing else on the row is.
///
/// Both halves matter. A hover that armed nothing would give the reader
/// no way in; one that armed everywhere would put a box over the text
/// they are reading, which is exactly the open-ended surface N3 refuses.
#[test]
fn hover_over_a_type_name_arms_the_dwell() {
    let mut app = type_row_app();

    app.handle_mouse(moved(column_of(&app, 0, "int32"), 0));
    assert!(
        app.hover_deadline.is_some(),
        "the type a row declares must arm the dwell"
    );
    assert_eq!(app.hover.map(|h| h.node), Some(0));
    assert!(app.score_popup.is_none(), "the dwell has not expired yet");

    // The value is the document, which is what a click is for.
    app.handle_mouse(moved(column_of(&app, 0, "x: 1"), 0));
    assert!(
        app.hover_deadline.is_none(),
        "an ordinary document column names nothing"
    );

    // The field number the type is declared with is not the type.
    app.handle_mouse(moved(column_of(&app, 0, "= 1") + 2, 0));
    assert!(app.hover_deadline.is_none());

    // Neither a label in front of the name nor an enum's value behind
    // it belongs to it.
    app.handle_mouse(moved(column_of(&app, 1, "repeated"), 1));
    assert!(app.hover.is_none(), "a label is not the type");
    app.handle_mouse(moved(column_of(&app, 1, "Color"), 1));
    assert_eq!(app.hover.map(|h| h.node), Some(1));
    app.handle_mouse(moved(column_of(&app, 1, "(5)"), 1));
    assert!(app.hover.is_none(), "an enum's value is not the type");

    // With the annotations hidden (`a`) there is no name on screen to
    // point at. The hit test needs no rule for that: `row_content` is
    // what it reads, and the annotation is no longer in it.
    let target = column_of(&app, 0, "int32");
    app.annotations = false;
    app.handle_mouse(moved(target, 0));
    assert!(app.hover.is_none());
}

/// Spec 0280 test plan 2 / S11-S12: the frame that notices an expired
/// dwell is the frame that opens the box, and leaving the target closes
/// it again.
///
/// The deadline is planted rather than waited out — the same shape
/// spec 0263's notes record for `message_deadline`, and for the same
/// reason: a test that slept `HOVER_DWELL` would be testing the clock.
#[test]
fn the_dwell_opens_the_popup_and_leaving_closes_it() {
    let mut app = type_row_app();
    let target = column_of(&app, 0, "int32");
    app.handle_mouse(moved(target, 0));
    assert!(app.score_popup.is_none());

    app.hover_deadline = Some(Instant::now() - HOVER_DWELL);
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let popup = app.score_popup.clone().expect("the dwell has been earned");
    assert_eq!(popup.anchor, (target, 0));
    assert!(
        app.hover_deadline.is_none(),
        "a fired deadline must not re-fire on every following frame"
    );

    app.handle_mouse(moved(column_of(&app, 0, "x: 1"), 0));
    assert!(
        app.score_popup.is_none(),
        "a move off the target takes the box with it (S16)"
    );
}

/// Spec 0280 test plan 3 / S9 / G5: a pointer merely crossing the pane
/// costs no frame, and the one move that does owe one is the one that
/// erases a box already on screen.
#[test]
fn a_bare_move_costs_no_frame() {
    let mut app = type_row_app();
    let document = column_of(&app, 0, "x: 1");
    let target = column_of(&app, 0, "int32");

    app.event_changed_nothing = false;
    app.handle_mouse(moved(document, 0));
    assert!(
        app.event_changed_nothing,
        "over the document: nothing drawn"
    );

    app.event_changed_nothing = false;
    app.handle_mouse(moved(target, 0));
    assert!(app.hover.is_some());
    assert!(
        app.event_changed_nothing,
        "arming the dwell is not a visible change; the frame it \
         eventually needs is bought by `hover_deadline`"
    );

    // With a box on screen, the move that removes it must be drawn.
    app.open_score_popup(0, (target, 0));
    app.event_changed_nothing = false;
    app.handle_mouse(moved(document, 0));
    assert!(
        !app.event_changed_nothing,
        "erasing a visible box owes a frame"
    );
}

/// Spec 0280 test plan 5 / S1: the counts reported are the scorer's own,
/// and their weighted sum is the number the cue shows.
///
/// The payload is one declared field and one undeclared one, against a
/// graph whose `Msg0` declares field 1 as `uint64` only — so the two
/// terms are known independently of what the scorer does with them.
#[test]
fn the_breakdown_is_the_scores_own_terms() {
    let graph = test_scoring_graph();
    // field 1, varint 1 (declared) then field 2, varint 1 (not).
    let payload = [(1u8 << 3), 1, (2u8 << 3), 1];

    let b = inferred_breakdown(&payload, "Msg0", graph.graph())
        .expect("Msg0 is a root type of this graph");
    assert!(!b.vetoed);
    assert_eq!(b.matches, 1, "field 1 is declared");
    assert_eq!(b.unknowns, 1, "field 2 is not");
    assert_eq!(b.out_of_range, 0);
    assert_eq!(b.non_canonical, 0);
    assert_eq!(b.mismatches, 0);

    assert_eq!(
        Some(b.score()),
        inferred_score(&payload, "Msg0", graph.graph()),
        "the decomposition must sum to the number the cue prints"
    );

    // `None` keeps `inferred_score`'s own meaning: not a root type.
    assert!(inferred_breakdown(&payload, "no.such.Type", graph.graph()).is_none());
}

/// Spec 0280 test plan 6 / S3: a vetoed entry reports only that.
///
/// Its counters hold whatever had accumulated when the veto fired
/// part-way through a field, which is a fact about where the walk
/// stopped rather than about the payload — so the box must not print
/// them, however many there are.
#[test]
fn a_vetoed_type_reports_only_that() {
    let graph = test_scoring_graph();
    // Field 1 is declared `uint64`; here it arrives as a LEN record,
    // which the walk cannot reconcile and vetoes on.
    let payload = [(1u8 << 3) | WT_LEN as u8, 1, b'x'];

    let b = inferred_breakdown(&payload, "Msg0", graph.graph()).expect("still a root type");
    assert!(b.vetoed, "a wire-type contradiction vetoes");
    assert_eq!(
        inferred_score(&payload, "Msg0", graph.graph()),
        None,
        "and `inferred_score` reports it by refusing to answer"
    );

    let popup = ScorePopup {
        type_key: "Msg0".to_string(),
        breakdown: Breakdown::Scored(b),
        anchor: (0, 0),
    };
    let lines = App::score_popup_lines(&popup);
    assert_eq!(lines.len(), 2, "the type key and the verdict, nothing else");
    assert!(lines[1].contains("vetoed"), "{lines:?}");
    assert!(
        !lines.iter().any(|l| l.contains('×')),
        "no counters in the box: {lines:?}"
    );
}

/// Spec 0280 S4: the two states that must not render as a box full of
/// zeros.
#[test]
fn a_missing_graph_and_an_unranked_type_are_different_answers() {
    let mut app = cue_app();
    assert!(app.ctx.graph.is_none());
    assert_eq!(app.score_breakdown(0), Breakdown::NoGraph);

    let mut app = message_node_app_with_graph();
    app.splash = false;
    // The fixture's node carries a type this graph has never heard of,
    // which is `Unranked` rather than a graph that is missing.
    assert_eq!(app.score_breakdown(0), Breakdown::Unranked);
}

/// Spec 0280 S5: the memo answers the second ask without re-scoring, and
/// is keyed tightly enough that it cannot answer for another node.
#[test]
fn the_memo_is_one_entry_keyed_on_the_range_and_the_type() {
    let mut app = cue_app();
    let first = app.score_breakdown(0);
    let key = app.breakdown_memo.clone().expect("one entry, now filled");
    assert_eq!(key.0 .0, app.heat_scored_range(0).start);
    assert_eq!(Some(key.0 .1), app.current_type_key(0));
    assert_eq!(key.1, first);

    // A key that no longer matches must not be answered from the memo.
    app.breakdown_memo = Some((
        (usize::MAX, "no.such.Type".to_string()),
        Breakdown::Unranked,
    ));
    assert_eq!(app.score_breakdown(0), Breakdown::NoGraph);
}

/// Spec 0280 test plan 7 / S18-S19: `s` opens the same box at the caret,
/// and the row is offered in the context menu of a node showing a cue.
#[test]
fn s_opens_the_same_box_at_the_caret() {
    let mut app = cue_app();
    app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    let popup = app.score_popup.clone().expect("`s` opens the box");
    assert_eq!(popup.anchor, app.menu_anchor());
    assert_eq!(popup.breakdown, app.score_breakdown(app.cursor));

    // Any key at all takes it down again (S16) — there is no dismiss
    // binding to learn.
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert!(app.score_popup.is_none());

    let rows = app.main_menu_items();
    assert!(
        rows.iter().any(|i| i.key.code == KeyCode::Char('s')),
        "a node showing a cue offers the row"
    );

    // With the cues hidden there is no number on screen to explain, so
    // the row goes with them.
    app.heat_cues_hidden = true;
    let rows = app.main_menu_items();
    assert!(!rows.iter().any(|i| i.key.code == KeyCode::Char('s')));
}

/// Spec 0280 test plan 8 / S17: the menu is the innermost modal and
/// keeps the pointer while it is open.
#[test]
fn nothing_hovers_while_a_menu_is_open() {
    let mut app = type_row_app();
    let target = column_of(&app, 0, "int32");
    app.open_menu_at_caret();
    assert!(app.menu.is_some());

    app.handle_mouse(moved(target, 0));
    assert!(app.hover.is_none(), "the menu owns the pointer");
    assert!(app.hover_deadline.is_none());

    // And a box cannot be opened underneath one either.
    app.open_score_popup(0, (target, 0));
    assert!(app.score_popup.is_none());
}

/// Spec 0280 S15: zero categories are omitted, so a clean node's box is
/// one line saying so rather than five zeros and a total.
#[test]
fn only_the_non_zero_terms_are_printed() {
    let graph = test_scoring_graph();
    let payload = [(1u8 << 3), 1];

    let b = inferred_breakdown(&payload, "Msg0", graph.graph()).unwrap();
    let popup = ScorePopup {
        type_key: "Msg0".to_string(),
        breakdown: Breakdown::Scored(b),
        anchor: (0, 0),
    };
    let lines = App::score_popup_lines(&popup);
    assert_eq!(
        lines.len(),
        3,
        "the type key, the one non-zero term, and the total: {lines:?}"
    );
    assert!(lines[1].contains("fields matched"), "{lines:?}");
    assert!(lines[2].contains("total"), "{lines:?}");
}
