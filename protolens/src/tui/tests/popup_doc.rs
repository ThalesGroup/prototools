// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0285: the box a document token earns.

use super::super::heat_cue::{HEAT_ANCHOR_DEFAULT, HEAT_CUE_PREVIEW};
use super::super::heat_worker::{HeatWorkerHandle, RangeHeatEntry};
use super::super::popup::{HoverTarget, EXPLAIN_DWELL, HOVER_DWELL};
use super::super::popup_doc::{annotation_type_spans, doc_lines, DocElement, SuffixShape};
use super::super::tiered::Tier;
use super::super::*;
use super::support::*;
use ratatui::layout::Rect;

/// An app drawing `rows` verbatim, one root-level leaf each, sized so
/// that line `i` is the main pane's own terminal row `i`.
fn doc_app(rows: &[&str]) -> App {
    let mut app = sibling_leaves_app(rows);
    app.splash = false;
    app.main_area = Rect::new(0, 0, 120, 24);
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

/// The box a hover over `needle` on row `line` opens, as its text.
fn box_at(app: &mut App, line: usize, needle: &str) -> Vec<String> {
    let column = column_of(app, line, needle);
    let hit = app
        .doc_element_at_point(column, line as u16)
        .unwrap_or_else(|| panic!("`{needle}` is a target"));
    doc_lines(&hit, HEAT_ANCHOR_DEFAULT)
        .into_iter()
        .map(|l| l.text)
        .collect()
}

/// Whether `needle` on row `line` names nothing the box can speak for.
fn refused(app: &mut App, line: usize, needle: &str) -> bool {
    let column = column_of(app, line, needle);
    app.doc_element_at_point(column, line as u16).is_none()
}

/// Spec 0285 test plan 3 / S6: a modifier is explained under the token
/// as drawn, value and all, and above its tier.
#[test]
fn a_modifier_is_explained_with_its_value() {
    let mut app = doc_app(&["x: 1  #@ varint; val_ohb: 3"]);

    let lines = box_at(&mut app, 0, "val_ohb");
    assert_eq!(
        lines,
        [
            "non-canonical: legal, but no writer should emit it",
            "this varint is padded, not minimal",
        ],
        "token is in the border title; the 3 is visible there"
    );

    // Pointing at the value half of the modifier is pointing at the
    // modifier: one token, one target.
    let value = column_of(&app, 0, "val_ohb") + "val_ohb: ".len() as u16;
    let hit = app.doc_element_at_point(value, 0).expect("still the token");
    assert_eq!(
        doc_lines(&hit, HEAT_ANCHOR_DEFAULT)[0].text,
        "non-canonical: legal, but no writer should emit it"
    );

    // An invalid keyword carries the other tier, and `pack_size` — a
    // landmark rather than a fault — carries neither.
    let mut app = doc_app(&["y: 2  #@ bytes; INVALID_STRING", "z: 3  #@ pack_size: 5"]);
    assert_eq!(
        box_at(&mut app, 0, "INVALID_STRING")[0],
        "invalid: the blob is malformed, or this is not the schema"
    );
    assert_eq!(
        box_at(&mut app, 1, "pack_size"),
        ["how many elements this one packed wire record holds"]
    );
}

/// Spec 0322 S5 / test-plan item 6: the leaf's mark explains itself, and
/// does not borrow the fold marker's words — it folds nothing, so every
/// line of that box would be false of it. A clean leaf's fold column is
/// not a target at all.
#[test]
fn hovering_a_leaf_diamond_names_the_tier() {
    let mut app = doc_app(&["x: 1  #@ varint; val_ohb: 3", "y: 2  #@ varint"]);

    let hit = app
        .doc_element_at_point(app.main_area.x + render::HEAT_FIELD_WIDTH as u16, 0)
        .expect("the mark is a target");
    assert_eq!(
        doc_lines(&hit, HEAT_ANCHOR_DEFAULT)
            .into_iter()
            .map(|l| l.text)
            .collect::<Vec<_>>(),
        [
            "non-canonical: legal, but no writer should emit it".to_string(),
            "this row's own annotation says which".to_string(),
        ]
    );

    assert!(
        app.doc_element_at_point(app.main_area.x + render::HEAT_FIELD_WIDTH as u16, 1)
            .is_none(),
        "a clean leaf's fold column says nothing"
    );
}

/// Spec 0364 S3 / test-plan item 3: hovering the blue diamond on an
/// unknown leaf names the situation — the field is not in the parent's
/// schema — and reminds the reader that the annotation says the wire type,
/// even when `a` is hiding it.
#[test]
fn hovering_an_unknown_leaf_diamond_names_the_tier() {
    use crate::node_status::Status;
    let (mut app, _inner, _known, unknown) = unknown_field_fixture();
    app.splash = false;
    app.main_area = Rect::new(0, 0, 120, 24);

    // The unknown leaf's absolute line index is its screen row when
    // main_area.y == 0 and scroll.index == 0 (the fixture's defaults).
    let unknown_line = app.absolute_start(unknown) as u16;
    let pos = app
        .line_pos(unknown as usize)
        .expect("unknown is a valid line");
    let line_text = app.line_text(pos);
    let mark_col =
        app.main_area.x + render::HEAT_FIELD_WIDTH as u16 + render::marker_column(&line_text);

    let hit = app
        .doc_element_at_point(mark_col, unknown_line)
        .expect("the blue diamond is a hover target");

    assert_eq!(
        hit.element(),
        DocElement::AnomalyMark {
            status: Status::Unknown
        },
        "hit must be AnomalyMark with Unknown status"
    );
    assert_eq!(
        doc_lines(&hit, HEAT_ANCHOR_DEFAULT)
            .into_iter()
            .map(|l| l.text)
            .collect::<Vec<_>>(),
        [
            "this field is not declared in the parent's schema".to_string(),
            "it is invisible to protoc".to_string(),
            "the annotation at the end of the line says what wire type it carries".to_string(),
        ]
    );
}

/// Spec 0285 test plan 4 / S4: every part of a field declaration is its
/// own element, and the type name is none of them (S5/N1).
#[test]
fn the_annotation_declaration_is_explained() {
    let mut app = doc_app(&["p: [1, 2]  #@ repeated int32 [packed=true] = 85"]);

    assert_eq!(
        box_at(&mut app, 0, "repeated"),
        [
            "this field may occur more than once",
            "optional is the default and is never printed",
        ]
    );
    assert_eq!(
        box_at(&mut app, 0, "[packed=true]"),
        ["these elements share one length-prefixed record"],
    );
    assert_eq!(
        box_at(&mut app, 0, "= 85"),
        [
            "the field's number in its .proto message",
            "the number is on the wire; the name is not",
        ],
        "the sign and the digits are one target, so one answer"
    );
    assert_eq!(
        box_at(&mut app, 0, "#@")[0],
        "opens a prototext annotation: the rest of this line"
    );

    // `required` is the other label, and says which dialect it is from.
    let mut app = doc_app(&["q: 1  #@ required int32 = 1"]);
    assert_eq!(
        box_at(&mut app, 0, "required")[0],
        "proto2: this field must be present"
    );
}

/// Spec 0285 test plan 5 / S4: only the first token of an annotation can
/// be a wire type, which is what keeps `bytes` there from being read as
/// the scalar type name it also is.
#[test]
fn a_wire_type_token_is_not_read_as_a_scalar_type() {
    let mut app = doc_app(&["1000: \"ab\"  #@ bytes", "raw: \"ab\"  #@ bytes = 32"]);

    assert_eq!(
        box_at(&mut app, 0, "bytes"),
        ["wire type 2 — a length prefix, then that many bytes"],
        "no schema declared this field, so `bytes` is the wire type"
    );
    assert!(
        refused(&mut app, 1, "bytes"),
        "in a declaration `bytes` is the type name, which spec 0280 owns"
    );
}

/// Spec 0285 test plan 6 / S4: the field key says which of the three
/// kinds it is, which is readable from the key itself.
#[test]
fn the_field_key_says_which_kind_it_is() {
    let mut app = doc_app(&[
        "name: \"a\"  #@ string = 1",
        "999: 42  #@ varint",
        "[acme.blades]: 42  #@ int32 = 1000",
    ]);

    assert_eq!(
        box_at(&mut app, 0, "name"),
        ["the field's name, from the schema"]
    );
    assert_eq!(
        box_at(&mut app, 1, "999"),
        ["a field number: no schema declared this field"]
    );
    assert_eq!(
        box_at(&mut app, 2, "[acme.blades]"),
        ["an extension field, named by its full path"]
    );
}

/// Spec 0285 test plan 7 / N2: the value's box says what the value is,
/// and decodes nothing — except that an enum name is not what is on the
/// wire, which is a fact about the spelling rather than a decoding.
#[test]
fn the_value_says_why_it_is_spelled_that_way() {
    let mut app = doc_app(&[
        "x: 1  #@ int32 = 1",
        "c: GREEN  #@ Color(2) = 3",
        "u: 0x4005bf0a  #@ fixed32",
    ]);

    assert_eq!(
        box_at(&mut app, 0, "1  #@"),
        ["the field's value, as protoc --decode prints it"],
        "the value is the token; the annotation behind it is not part \
         of it, and nothing is decoded"
    );
    assert_eq!(
        box_at(&mut app, 1, "GREEN"),
        [
            "the field's value, as protoc --decode prints it",
            "the schema's name for the 2 on the wire",
        ]
    );
    assert_eq!(
        box_at(&mut app, 2, "0x4005bf0a")[1],
        "raw bits: no schema said how to read them"
    );
}

/// Spec 0285 test plan 8 / S5 / N1: the type name keeps 0280's box, so
/// a point has exactly one target and one dwell.
#[test]
fn the_type_name_still_opens_the_score_box() {
    let mut app = doc_app(&["x: 1  #@ repeated int32 [packed=true] = 85"]);
    let column = column_of(&app, 0, "int32");

    assert!(
        app.doc_element_at_point(column, 0).is_none(),
        "0285's hit test refuses the span 0280 owns"
    );
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.hover.as_ref().map(|h| h.target.clone()),
        Some(HoverTarget::Type(0)),
        "and the hover falls to 0280 rather than to a document box"
    );
}

/// Spec 0326 test plan 1-2 / S1: the words that stand where a type name
/// would are anchors for the same box, recognized by their text — `group`
/// lexes as a wire type and `message`, absent from the wire-type
/// vocabulary (N3), falls through to a modifier. A known group's row
/// names the node twice, so both words are in the zone.
#[test]
fn message_and_group_are_score_anchors() {
    let spans = |row: &str| -> Vec<String> {
        annotation_type_spans(row)
            .into_iter()
            .map(|s| row[s].to_string())
            .collect()
    };

    assert_eq!(spans("1 {  #@ message"), ["message"]);
    assert_eq!(spans("1 {  #@ group"), ["group"]);
    assert_eq!(
        spans("g {  #@ group; Foo = 3"),
        ["group", "Foo"],
        "one node, two words, one zone"
    );
    assert_eq!(spans("x: 1  #@ int32 = 1"), ["int32"], "0280's own target");
    assert!(
        spans("1000: \"ab\"  #@ bytes").is_empty(),
        "a real wire type names no type, so it anchors nothing"
    );
}

/// Spec 0326 test plan 3 / S2: one rule decides, so an anchor cannot
/// also open the document box. `group` knowingly loses its wire-type
/// explanation to the score box.
#[test]
fn a_score_anchor_is_not_a_document_box() {
    let mut app = doc_app(&["1 {  #@ message", "1 {  #@ group"]);

    assert!(refused(&mut app, 0, "message"));
    assert!(refused(&mut app, 1, "group"));

    // And the hover falls to 0280's target rather than to nothing,
    // which is the whole point: today `message` is neither.
    let column = column_of(&app, 0, "message");
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.hover.as_ref().map(|h| h.target.clone()),
        Some(HoverTarget::Type(0))
    );
}

/// Spec 0285 test plan 9 / S7: an explanation is asked for rather than
/// travelled over, so it waits longer than a datum does.
#[test]
fn an_explanation_waits_longer_than_a_score() {
    let mut app = doc_app(&["x: 1  #@ varint; val_ohb: 3"]);
    let moved = |column| MouseEvent {
        kind: MouseEventKind::Moved,
        column,
        row: 0,
        modifiers: KeyModifiers::NONE,
    };

    app.handle_mouse(moved(column_of(&app, 0, "val_ohb")));
    let deadline = app.hover_deadline.expect("a modifier arms the dwell");
    assert!(
        deadline > Instant::now() + HOVER_DWELL,
        "an explanation waits longer than the {HOVER_DWELL:?} a score does"
    );
    assert!(deadline <= Instant::now() + EXPLAIN_DWELL);

    // The same app, the same pointer, a type name: back to 0280's.
    let mut app = doc_app(&["x: 1  #@ int32 = 1"]);
    app.handle_mouse(moved(column_of(&app, 0, "int32")));
    let deadline = app.hover_deadline.expect("a type name arms it too");
    assert!(deadline <= Instant::now() + HOVER_DWELL);
}

/// Spec 0285 test plan 10 / S8: the box is 0280's, so everything that
/// takes one down takes this one down.
#[test]
fn a_document_box_is_dismissed_like_any_other() {
    let mut app = doc_app(&["x: 1  #@ varint; val_ohb: 3"]);
    let column = column_of(&app, 0, "val_ohb");
    // The deadline is planted and `track_hover_dwell` called by hand,
    // the shape spec 0280's own tests use: waiting one out would be
    // testing the clock.
    let open = |app: &mut App| {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        app.hover_deadline = Some(Instant::now() - EXPLAIN_DWELL);
        app.track_hover_dwell();
    };

    open(&mut app);
    let popup = app.popup.clone().expect("the dwell has been earned");
    assert_eq!(popup.anchor, (column, 0));
    assert!(matches!(popup.body, PopupBody::Doc(_)));

    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert!(app.popup.is_none(), "any keystroke takes it down");

    open(&mut app);
    assert!(app.popup.is_some());
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert!(app.popup.is_none(), "and so does a click");
}

/// Spec 0285 N4: a modifier with nothing to say is not a target, which
/// is what keeps the `#@ prototext: protoc` header out — a box whose
/// only line is the word already on screen is worse than no box.
#[test]
fn a_keyword_with_no_clause_is_not_a_target() {
    let mut app = doc_app(&["x: 1  #@ prototext: protoc", "y: 2  #@ varint; ??gibberish"]);

    assert!(refused(&mut app, 0, "prototext"));
    assert!(refused(&mut app, 1, "??gibberish"));
    // The `#@` in front of the header still is one: it says what the
    // whole line is doing there, and it is where the format is named.
    assert_eq!(
        box_at(&mut app, 0, "#@"),
        [
            "opens a prototext annotation: the rest of this line",
            "is how the bytes were encoded, not part of the message",
            "prototext is textproto, plus these annotations",
        ],
        "the marker introduces the annotation rather than being it"
    );
}

// ---------------------------------------------------------------------
// Spec 0287: the chrome beside a row
// ---------------------------------------------------------------------

/// A `message_node_app` whose root has a heat cue seeded on it, drawn
/// so the header is the pane's own row 0.
///
/// The two arguments are the two caches a cue is read out of, and
/// their absence is what spec 0154 G6's progressive shapes *are*: no
/// `stats` is `[?]`, stats without a `current` is `[?/best]`.
fn cue_app(stats: Option<(i64, usize)>, current: Option<Option<i64>>) -> App {
    let mut app = message_node_app();
    app.splash = false;
    app.main_area = Rect::new(0, 0, 120, 24);
    // A session opens with no cues drawn at all (spec 0331 S1).
    app.heat_cues = heat_cue::HeatCueMode::Findings;
    // Without a worker, `heat_cue_resolve` reads an unsettled cache as
    // "nothing will ever resolve this" and draws no cue at all, so the
    // two progressive shapes would be unreachable.
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    let start = extract::message_payload_range(&app.blob, &app.tree[0].span.raw_range).start;
    {
        let mut caches = app.heat_caches.lock().unwrap();
        if let Some((best_score, best_count)) = stats {
            caches.by_range.upsert(
                start,
                RangeHeatEntry {
                    best_score: Some(best_score),
                    best_count,
                    top_n: vec![("protolens_internal.dummy".to_string(), 0); HEAT_CUE_PREVIEW],
                },
                Tier::Visible,
            );
        }
        if let Some(score) = current {
            caches.current_score.upsert(
                (start, "google.protobuf.DescriptorProto".to_string()),
                score,
                Tier::Visible,
            );
        }
    }
    app
}

/// The box the pane's column 0 of row 0 opens.
fn glyph_box(app: &mut App) -> Vec<String> {
    let hit = app
        .doc_element_at_point(app.main_area.x, app.main_area.y)
        .expect("the glyph is a target");
    doc_lines(&hit, HEAT_ANCHOR_DEFAULT)
        .into_iter()
        .map(|l| l.text)
        .collect()
}

/// The drawn heat suffix of row 0, hit through `heat_cue_at_point`'s
/// own geometry — one column inside it, so the test cannot pass by
/// pointing at the row's text.
fn suffix_element(app: &mut App) -> DocElement {
    let content = app.row_content(app.committed_row(0).expect("a drawn row"));
    let column = app.main_area.x + render::HEAT_FIELD_WIDTH as u16 + content.chars().count() as u16;
    assert_eq!(
        app.heat_cue_at_point(column, app.main_area.y),
        Some(0),
        "the column must be inside the drawn suffix"
    );
    app.doc_element_at_point(column, app.main_area.y)
        .expect("the suffix is a target")
        .element()
}

/// The box that same suffix opens.
fn suffix_box(app: &mut App) -> Vec<String> {
    let content = app.row_content(app.committed_row(0).expect("a drawn row"));
    let column = app.main_area.x + render::HEAT_FIELD_WIDTH as u16 + content.chars().count() as u16;
    assert_eq!(
        app.heat_cue_at_point(column, app.main_area.y),
        Some(0),
        "the column must be inside the drawn suffix"
    );
    let hit = app
        .doc_element_at_point(column, app.main_area.y)
        .expect("the suffix is a target");
    doc_lines(&hit, HEAT_ANCHOR_DEFAULT)
        .into_iter()
        .map(|l| l.text)
        .collect()
}

/// Spec 0287 test plan 1 / S4: the glyph's own color decides the words,
/// so the box and the mark cannot disagree (G2). Spec 0337 S6: the box
/// also states the current anchor so the brightness is interpretable.
#[test]
fn the_heat_glyph_explains_its_color() {
    // The default anchor is ln(144) ≈ 4.97, so exp(anchor).round() = 144.
    let mut app = cue_app(Some((50, 1)), Some(Some(10)));
    assert_eq!(
        glyph_box(&mut app),
        [
            "the current typing (10) does not max the score (50)",
            "see the heat cue on the right",
            "brightest at score 144 and above",
        ]
    );

    // A tie is the same glyph in the same column, and a different box.
    let mut app = cue_app(Some((50, 3)), Some(Some(50)));
    assert_eq!(
        glyph_box(&mut app),
        [
            "current typing maxes the score (50) but there are ties",
            "see the heat cue on the right",
            "brightest at score 144 and above",
        ]
    );
}

/// Spec 0287 test plan 2 / S4: each of `HeatDisplay`'s drawn shapes has
/// its own words, and the token line is the suffix as drawn.
#[test]
fn the_heat_suffix_explains_its_numbers() {
    // lines[0] is always "Best scorers:" — the token moved to the border
    // title. lines[1] is the first top-scoring FQDN from the cache
    // (seeded as "protolens_internal.dummy"), or a fallback sentence.
    let lines = |app: &mut App| {
        let lines = suffix_box(app);
        (lines[0].clone(), lines[1].clone())
    };

    let mut app = cue_app(Some((50, 1)), Some(Some(10)));
    assert_eq!(
        lines(&mut app),
        (
            "Best scorers:".to_string(),
            "protolens_internal.dummy".to_string()
        )
    );

    let mut app = cue_app(Some((50, 1)), Some(None));
    assert_eq!(
        lines(&mut app),
        (
            "Best scorers:".to_string(),
            "protolens_internal.dummy".to_string()
        )
    );

    let mut app = cue_app(Some((50, 3)), Some(Some(50)));
    assert_eq!(
        lines(&mut app),
        (
            "Best scorers:".to_string(),
            "protolens_internal.dummy".to_string()
        )
    );

    let mut app = cue_app(Some((50, 1)), None);
    assert_eq!(
        lines(&mut app),
        (
            "Best scorers:".to_string(),
            "protolens_internal.dummy".to_string()
        )
    );

    // No stats at all: the cache has no entry, so heat_top is empty.
    let mut app = cue_app(None, None);
    assert_eq!(
        lines(&mut app),
        (
            "Best scorers:".to_string(),
            "still scoring these bytes".to_string()
        )
    );
}

/// Spec 0287 G3: the one thing about the suffix a reader cannot
/// discover by looking at it, on every shape it takes.
#[test]
fn the_heat_suffix_names_the_double_click() {
    for (stats, current) in [
        (Some((50, 1)), Some(Some(10))),
        (Some((50, 1)), Some(None)),
        (Some((50, 3)), Some(Some(50))),
        (Some((50, 1)), None),
        (None, None),
    ] {
        let mut app = cue_app(stats, current);
        let lines = suffix_box(&mut app);
        assert_eq!(
            lines.last().map(String::as_str),
            Some("double-click to choose a type for this node"),
            "the suffix is a control, whatever it currently reads"
        );
    }

    // The glyph is not: `heat_cue_at_point` measures the suffix alone,
    // so column 0 has no double-click to name and the box points at the
    // numbers instead.
    let mut app = cue_app(Some((50, 1)), Some(Some(10)));
    assert!(!glyph_box(&mut app).iter().any(|l| l.contains("double")));
}

/// Spec 0331 test 7 / S6: spec 0287's rule is that every drawn mark
/// says what it is, so the two new cues have boxes — each its own, not
/// nothing and not the other's.
#[test]
fn the_new_cues_have_boxes() {
    // The unique optimum.
    let mut app = cue_app(Some((50, 1)), Some(Some(50)));
    app.heat_cues = heat_cue::HeatCueMode::All;
    assert_eq!(
        suffix_element(&mut app),
        DocElement::HeatSuffix(SuffixShape::Agree)
    );
    let lines = suffix_box(&mut app);
    // Token moved to border title; first content line is "Best scorers:".
    assert_eq!(lines[0], "Best scorers:");
    // The top-scoring FQDN is shown, seeded as "protolens_internal.dummy".
    assert_eq!(lines[1], "protolens_internal.dummy", "{lines:?}");
    assert_eq!(
        lines.last().map(String::as_str),
        Some("double-click to choose a type for this node")
    );

    // Every candidate rejected — which `cue_app`'s `stats` cannot spell,
    // since `None` there means no cache entry at all (`[?]`).
    let mut app = cue_app(None, None);
    app.heat_cues = heat_cue::HeatCueMode::All;
    let start = extract::message_payload_range(&app.blob, &app.tree[0].span.raw_range).start;
    app.heat_caches.lock().unwrap().by_range.upsert(
        start,
        RangeHeatEntry {
            best_score: None,
            best_count: 0,
            top_n: vec![("protolens_internal.dummy".to_string(), 0); HEAT_CUE_PREVIEW],
        },
        Tier::Visible,
    );
    assert_eq!(
        suffix_element(&mut app),
        DocElement::HeatSuffix(SuffixShape::Unmatched)
    );
    let lines = suffix_box(&mut app);
    // Token moved to border title; first content line is "Best scorers:".
    assert_eq!(lines[0], "Best scorers:");
    assert!(lines[1].contains("no type known here fits"), "{lines:?}");
}

/// Spec 0335 test 6 / S5: the square now means a third thing, and spec
/// 0287's rule applies to it too — its own prose, not the tie's and not
/// the mismatch's. The second line is the one that has to differ: a
/// reader who has learned "brighter means bigger" from the other two
/// would otherwise read a maximum score off a flat sentinel.
#[test]
fn the_unmatched_square_has_a_box() {
    let mut app = cue_app(None, None);
    app.heat_cues = heat_cue::HeatCueMode::All;
    let start = extract::message_payload_range(&app.blob, &app.tree[0].span.raw_range).start;
    app.heat_caches.lock().unwrap().by_range.upsert(
        start,
        RangeHeatEntry {
            best_score: None,
            best_count: 0,
            top_n: vec![("protolens_internal.dummy".to_string(), 0); HEAT_CUE_PREVIEW],
        },
        Tier::Visible,
    );
    assert_eq!(
        glyph_box(&mut app),
        [
            "no type known here fits these bytes",
            "this one is not graded - there is no score",
        ],
        "and no trailing line pointing at numbers that are not there"
    );
}

/// Spec 0287 test plan 8 / S3: the target is exactly what is on screen,
/// so rotating the cues away takes both cue targets with the marks.
/// `I` and not `i` (spec 0331 S2): from the findings view, backward is
/// the direction that reaches "nothing shown".
#[test]
fn a_hidden_cue_is_not_a_target() {
    let mut app = cue_app(Some((50, 1)), Some(Some(10)));
    let content = app.row_content(app.committed_row(0).expect("a drawn row"));
    let suffix_column =
        app.main_area.x + render::HEAT_FIELD_WIDTH as u16 + content.chars().count() as u16;
    assert!(app.doc_element_at_point(app.main_area.x, 0).is_some());
    assert!(app.doc_element_at_point(suffix_column, 0).is_some());

    app.handle_key(KeyEvent::new(KeyCode::Char('I'), KeyModifiers::NONE));

    assert!(app.doc_element_at_point(app.main_area.x, 0).is_none());
    assert!(app.doc_element_at_point(suffix_column, 0).is_none());
}

/// Spec 0287 test plan 4 / S4: the box flips with the glyph, and says
/// which way the click will go.
#[test]
fn the_fold_marker_explains_which_way_it_is() {
    let (mut app, inner, ..) = unknown_field_fixture();
    app.splash = false;
    app.main_area = Rect::new(0, 0, 120, 24);
    let line = app.absolute_start(inner);
    let pos = app.line_pos(line).unwrap();
    let column = app.main_area.x
        + render::HEAT_FIELD_WIDTH as u16
        + render::marker_column(&app.line_text(pos));

    let read = |app: &mut App| {
        let hit = app
            .doc_element_at_point(column, line as u16)
            .expect("the marker is a target");
        doc_lines(&hit, HEAT_ANCHOR_DEFAULT)
            .into_iter()
            .map(|l| l.text)
            .collect::<Vec<_>>()
    };

    assert!(!app.is_folded(inner));
    let open = read(&mut app);
    assert_eq!(open[0], "this node is unfolded");
    assert_eq!(open[1], "click to fold it");

    app.toggle_fold(inner);
    let closed = read(&mut app);
    assert_eq!(closed[0], "this node is folded");
    assert_eq!(closed[1], "click to unfold it");

    // Spec 0247 S10: this fixture's subtree holds an unknown field, so
    // the glyph wears a status color and the box says what it means.
    assert_eq!(
        closed.last().map(String::as_str),
        Some("the color is the worst thing found anywhere inside"),
        "the fixture's subtree is not clean, so the glyph is colored"
    );
}

/// Spec 0287 test plan 5 / S3: the hover target and the click target
/// are the same rectangle, because they are the same locator. Every
/// column of the fold field is both, and no column outside it is
/// either.
#[test]
fn the_fold_marker_hover_and_click_share_a_rectangle() {
    let (mut app, inner, ..) = unknown_field_fixture();
    app.splash = false;
    app.main_area = Rect::new(0, 0, 120, 24);
    let line = app.absolute_start(inner);
    let pos = app.line_pos(line).unwrap();
    let marker = render::marker_column(&app.line_text(pos)) as usize;

    for column in 0..12u16 {
        let in_field = (marker..marker + render::FOLD_FIELD_WIDTH)
            .contains(&(column as usize).wrapping_sub(render::HEAT_FIELD_WIDTH));
        let is_marker = matches!(
            app.doc_element_at_point(column, line as u16),
            Some(hit) if matches!(hit.element(), DocElement::FoldMarker { .. })
        );
        assert_eq!(
            is_marker, in_field,
            "column {column}: the box and the toggle must agree"
        );
        // And what the click does there is the same answer, read off
        // the one locator both go through.
        assert_eq!(app.in_fold_field(column, pos), in_field);
    }
}

/// Spec 0287 test plan 6 / S3: pointing at the brace and pointing at
/// the ellipsis give one answer, and a folded node says so.
#[test]
fn a_folded_node_explains_its_summary() {
    let (mut app, inner, ..) = unknown_field_fixture();
    app.splash = false;
    app.main_area = Rect::new(0, 0, 120, 24);
    app.toggle_fold(inner);
    let line = app.absolute_start(inner);

    let content = app.row_content(app.committed_row(line).expect("a drawn row"));
    let brace = content.rfind('{').expect("a folded node draws its brace");
    let first = render::HEAT_FIELD_WIDTH as u16 + content[..brace].chars().count() as u16;

    let expected = [
        "this node is folded: its fields are not shown",
        "click the marker in the left margin to unfold it",
    ];
    for column in first..first + "{ ... }".len() as u16 {
        let hit = app
            .doc_element_at_point(column, line as u16)
            .unwrap_or_else(|| panic!("column {column} of the summary is a target"));
        let lines: Vec<String> = doc_lines(&hit, HEAT_ANCHOR_DEFAULT)
            .into_iter()
            .map(|l| l.text)
            .collect();
        assert_eq!(lines, expected, "column {column}");
    }
}

/// Spec 0287 test plan 7 / S4: spec 0260's arm — a grayed `{ ... }` is
/// not a fold the reader made, and the box says that instead.
#[test]
fn an_unbaked_summary_says_nobody_has_looked() {
    let (mut app, inner, ..) = unknown_field_fixture();
    app.splash = false;
    app.main_area = Rect::new(0, 0, 120, 24);
    app.toggle_fold(inner);
    app.auto_folded.insert(inner);
    let line = app.absolute_start(inner);

    let content = app.row_content(app.committed_row(line).expect("a drawn row"));
    // The fold margin holds a multibyte glyph, so the column is a
    // count of characters, not the brace's byte offset.
    let brace = content.rfind('{').expect("a brace");
    let column = render::HEAT_FIELD_WIDTH as u16 + content[..brace].chars().count() as u16;
    let hit = app
        .doc_element_at_point(column, line as u16)
        .expect("the summary is a target");
    let lines: Vec<String> = doc_lines(&hit, HEAT_ANCHOR_DEFAULT)
        .into_iter()
        .map(|l| l.text)
        .collect();
    assert_eq!(lines, ["nobody has looked inside this region yet"]);
}

/// Spec 0287 test plan 9 / S3: the chrome takes only columns no token
/// can reach, so a key that starts immediately after the fold field
/// still answers for its own first character.
#[test]
fn chrome_does_not_steal_a_token() {
    let mut app = doc_app(&["x: 1  #@ varint"]);
    assert_eq!(
        box_at(&mut app, 0, "x")[0],
        "the field's name, from the schema"
    );
}
