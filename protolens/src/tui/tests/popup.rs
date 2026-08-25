// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0280: the box a heat cue's number can be asked for.

use super::super::heat_cue::HeatCueMode;
use super::super::heat_worker::RangeHeatEntry;
use super::super::popup::{Breakdown, HoverTarget, HOVER_DWELL};
use super::super::tiered::Tier;
use super::super::*;
use super::heat_cue::seed_range_heat_entry;
use super::support::*;
use crate::override_pane::{inferred_breakdown, inferred_score};

/// An app whose node 0 shows a cue, sized so its header row is the main
/// pane's own first row at `(0, 0)`.
///
/// The seeding is `i_rotates_the_cue_away_and_back`'s: a
/// `RangeHeatEntry` planted straight into `heat_caches`, which is what
/// lets a cue exist without a scoring graph behind it. A session opens
/// with no cues drawn at all (spec 0331 S1), so the mode is asked for
/// here too.
fn cue_app() -> App {
    let mut app = message_node_app();
    app.splash = false;
    app.heat_cues = HeatCueMode::Findings;
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
    // The pane's leading columns are the heat glyph's reserved gutter.
    render::HEAT_FIELD_WIDTH as u16 + content[..at].chars().count() as u16
}

/// The node a `Type` hover names — `None` when nothing is hovered, and
/// `None` for a wire or document target, neither of which names a type.
fn hovered_node(app: &App) -> Option<usize> {
    match app.hover.as_ref()?.target {
        HoverTarget::Type(node) => Some(node),
        HoverTarget::Wire(_) | HoverTarget::Doc(_) => None,
    }
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
/// and nothing else on the row names a *type*.
///
/// Both halves matter. A hover that armed nothing would give the reader
/// no way in; one that armed everywhere would put a box over the text
/// they are reading, which is exactly the open-ended surface N3 refuses.
///
/// Since spec 0285 the neighboring tokens do arm a dwell of their own,
/// so the assertions below are that they are not *this* target: one
/// point, one box (0285 S5).
#[test]
fn hover_over_a_type_name_arms_the_dwell() {
    let mut app = type_row_app();

    app.handle_mouse(moved(column_of(&app, 0, "int32"), 0));
    assert!(
        app.hover_deadline.is_some(),
        "the type a row declares must arm the dwell"
    );
    assert_eq!(hovered_node(&app), Some(0));
    assert!(app.popup.is_none(), "the dwell has not expired yet");

    // The field key is the document, which is what a click is for.
    app.handle_mouse(moved(column_of(&app, 0, "x: 1"), 0));
    assert_eq!(
        hovered_node(&app),
        None,
        "an ordinary document column names no type"
    );

    // The field number the type is declared with is not the type.
    app.handle_mouse(moved(column_of(&app, 0, "= 1") + 2, 0));
    assert_eq!(hovered_node(&app), None);

    // Neither a label in front of the name nor an enum's value behind
    // it belongs to it.
    app.handle_mouse(moved(column_of(&app, 1, "repeated"), 1));
    assert_eq!(hovered_node(&app), None, "a label is not the type");
    app.handle_mouse(moved(column_of(&app, 1, "Color"), 1));
    assert_eq!(hovered_node(&app), Some(1));
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
    assert!(app.popup.is_none());

    app.hover_deadline = Some(Instant::now() - HOVER_DWELL);
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let popup = app.popup.clone().expect("the dwell has been earned");
    assert_eq!(popup.anchor, (target, 0));
    assert!(
        app.hover_deadline.is_none(),
        "a fired deadline must not re-fire on every following frame"
    );

    app.handle_mouse(moved(column_of(&app, 0, "x: 1"), 0));
    assert!(
        app.popup.is_none(),
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

    let b = inferred_breakdown(&payload, "Msg0", graph.graph(), false)
        .expect("Msg0 is a root type of this graph");
    assert!(!b.vetoed);
    assert_eq!(b.matches, 1, "field 1 is declared");
    assert_eq!(b.unknowns, 1, "field 2 is not");
    assert_eq!(b.out_of_range, 0);
    assert_eq!(b.non_canonical, 0);
    assert_eq!(b.mismatches, 0);

    assert_eq!(
        Some(b.score()),
        inferred_score(&payload, "Msg0", graph.graph(), false),
        "the decomposition must sum to the number the cue prints"
    );

    // `None` keeps `inferred_score`'s own meaning: not a root type.
    assert!(inferred_breakdown(&payload, "no.such.Type", graph.graph(), false).is_none());
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

    let b = inferred_breakdown(&payload, "Msg0", graph.graph(), false).expect("still a root type");
    assert!(b.vetoed, "a wire-type contradiction vetoes");
    assert_eq!(
        inferred_score(&payload, "Msg0", graph.graph(), false),
        None,
        "and `inferred_score` reports it by refusing to answer"
    );

    let popup = Popup {
        body: PopupBody::Score {
            type_key: "Msg0".to_string(),
            breakdown: Breakdown::Scored(b),
            candidate: None,
        },
        anchor: (0, 0),
        doc_title: None,
    };
    let lines = App::popup_lines(&popup, 22);
    assert_eq!(lines.len(), 2, "the type key and the verdict, nothing else");
    assert!(lines[1].text.contains("vetoed"), "{lines:?}");
    assert!(
        !lines.iter().any(|l| l.text.contains('×')),
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
    assert!(app.popup.is_none());
}

/// Spec 0280 S15: zero categories are omitted, so a clean node's box is
/// one line saying so rather than five zeros under a score.
#[test]
fn only_the_non_zero_terms_are_printed() {
    let graph = test_scoring_graph();
    let payload = [(1u8 << 3), 1];

    let b = inferred_breakdown(&payload, "Msg0", graph.graph(), false).unwrap();
    let popup = Popup {
        body: PopupBody::Score {
            type_key: "Msg0".to_string(),
            breakdown: Breakdown::Scored(b),
            candidate: None,
        },
        anchor: (0, 0),
        doc_title: None,
    };
    let lines = App::popup_lines(&popup, 22);
    assert_eq!(
        lines.len(),
        3,
        "the type key, the score, and the one non-zero term: {lines:?}"
    );
    assert!(lines[1].text.contains("score"), "{lines:?}");
    assert!(lines[2].text.contains("fields matched"), "{lines:?}");
}

/// Spec 0310 S3: a cut range's counters stay meaningful — unlike a
/// vetoed one's — so the box prints them, and says why the reading is
/// provisional in a line of its own.
#[test]
fn a_cut_range_says_so_and_still_shows_its_terms() {
    let graph = test_scoring_graph();
    // Field 1 (declared) in full, then a LEN field whose declared length
    // runs past what is there.
    let payload = [(1u8 << 3), 1, (2u8 << 3) | WT_LEN as u8, 9, b'x'];

    let b = inferred_breakdown(&payload, "Msg0", graph.graph(), true).expect("a root type");
    assert!(!b.vetoed, "a cut is not a contradiction");
    assert!(b.truncated > 0);
    assert_eq!(b.matches, 1, "the field before the cut still counts");

    let popup = Popup {
        body: PopupBody::Score {
            type_key: "Msg0".to_string(),
            breakdown: Breakdown::Scored(b),
            candidate: None,
        },
        anchor: (0, 0),
        doc_title: None,
    };
    let lines = App::popup_lines(&popup, 22);
    assert!(lines[2].text.contains("fields matched"), "{lines:?}");
    let cut = lines
        .iter()
        .find(|l| l.text.contains("the bytes ran out"))
        .unwrap_or_else(|| panic!("the box must say the range was cut: {lines:?}"));
    assert!(cut.text.contains("-5"), "with its weight: {cut:?}");
    assert!(
        cut.text.contains('×'),
        "and with a count, since truncated is now u64: {cut:?}"
    );
    let score = &lines[1].text;
    assert!(
        score.contains("score") && score.contains(&b.score().to_string()),
        "the terms below must still sum to the score: {lines:?}"
    );
}

// ---------------------------------------------------------------------
// Spec 0326: the box an untyped node opens
// ---------------------------------------------------------------------

/// An app whose node 0 is a message no schema named — the shape a
/// `1 {  #@ message` row has — with `Msg0` optionally seeded as the top
/// of `best_count` tied candidates.
///
/// `NO_FQDN` on a message node is exactly what makes
/// `current_type_key` say nothing (`heat_cue.rs`'s first arm), and so
/// what sends `score_body` down the candidate path.
fn untyped_app(seed: Option<usize>) -> App {
    let mut app = message_node_app_with_graph();
    app.splash = false;
    app.main_area = Rect::new(0, 0, 80, 22);
    Arc::get_mut(&mut app.tree).expect("the fixture holds it alone")[0]
        .span
        .type_fqdn = NO_FQDN;
    assert_eq!(app.current_type_key(0), None, "the fixture's premise");

    if let Some(best_count) = seed {
        let start = app.heat_scored_range(0).start;
        let mut caches = app.heat_caches.lock().unwrap();
        caches.by_range.upsert(
            start,
            RangeHeatEntry {
                best_score: Some(0),
                best_count,
                // One entry is all `record_sweep` guarantees, and all
                // S4 reads.
                top_n: vec![("Msg0".to_string(), 0)],
            },
            Tier::Visible,
        );
    }
    app
}

/// A raw `message` node (no named FQDN) scores as "message" directly —
/// the candidate info is already on the RHS heat cue and not repeated
/// in the box.
#[test]
fn an_untyped_node_shows_its_best_candidate() {
    let mut app = untyped_app(Some(3));
    app.open_score_popup(0, (0, 0));
    let body = app.popup.clone().expect("the box opens").body;

    let PopupBody::Score {
        type_key,
        candidate,
        ..
    } = body
    else {
        panic!("a score box: {body:?}");
    };
    assert_eq!(type_key, "message", "scored as the raw message type");
    assert_eq!(candidate, None, "no candidate info for message nodes");
}

/// Same with no cache entry: still scores as "message", not pending.
#[test]
fn an_unscored_range_is_pending_not_unranked() {
    let mut app = untyped_app(None);
    app.open_score_popup(0, (0, 0));
    let body = app.popup.clone().expect("the box still opens").body;
    assert!(
        matches!(
            body,
            PopupBody::Score {
                type_key: ref k,
                candidate: None,
                ..
            } if k == "message"
        ),
        "raw message node: scored as \"message\", no candidate: {body:?}"
    );
}

/// Spec 0326 test plan 5-7 / S5: the tie line is printed under a
/// candidate and only when there are ties, and a typed node's box is
/// the one it always was.
#[test]
fn only_a_candidate_box_counts_the_ties() {
    let graph = test_scoring_graph();
    let payload = [(1u8 << 3), 1];
    let b = inferred_breakdown(&payload, "Msg0", graph.graph(), false).unwrap();

    let ties_line = |candidate| {
        let popup = Popup {
            body: PopupBody::Score {
                type_key: "Msg0".to_string(),
                breakdown: Breakdown::Scored(b),
                candidate,
            },
            anchor: (0, 0),
            doc_title: None,
        };
        App::popup_lines(&popup, 22)
            .into_iter()
            .find(|l| l.text.contains("also score"))
            .map(|l| l.text.trim().to_string())
    };

    assert_eq!(
        ties_line(Some(3)),
        Some(format!("3 others also score {}", b.score())),
        "the number printed is the breakdown's own sum, so the two \
         numbers in the box cannot come apart"
    );
    assert_eq!(ties_line(Some(0)), None, "a lone best has no others");
    assert_eq!(ties_line(None), None, "and a typed node has no candidate");
}
