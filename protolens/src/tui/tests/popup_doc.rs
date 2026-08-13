// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0285: the box a document token earns.

use super::super::popup::{HoverTarget, EXPLAIN_DWELL, HOVER_DWELL};
use super::super::popup_doc::doc_lines;
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
    // Column 0 of the pane is the heat glyph's reserved gutter.
    1 + content[..at].chars().count() as u16
}

/// The box a hover over `needle` on row `line` opens, as its text.
fn box_at(app: &mut App, line: usize, needle: &str) -> Vec<String> {
    let column = column_of(app, line, needle);
    let hit = app
        .doc_element_at_point(column, line as u16)
        .unwrap_or_else(|| panic!("`{needle}` is a target"));
    doc_lines(&hit).into_iter().map(|l| l.text).collect()
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
            "val_ohb: 3",
            "non-canonical: legal, but no writer should emit it",
            "this varint is padded, not minimal",
        ],
        "the token leads, so the 3 is shown without a format string"
    );

    // Pointing at the value half of the modifier is pointing at the
    // modifier: one token, one target.
    let value = column_of(&app, 0, "val_ohb") + "val_ohb: ".len() as u16;
    let hit = app.doc_element_at_point(value, 0).expect("still the token");
    assert_eq!(doc_lines(&hit)[0].text, "val_ohb: 3");

    // An invalid keyword carries the other tier, and `pack_size` — a
    // landmark rather than a fault — carries neither.
    let mut app = doc_app(&["y: 2  #@ bytes; INVALID_STRING", "z: 3  #@ pack_size: 5"]);
    assert_eq!(
        box_at(&mut app, 0, "INVALID_STRING")[1],
        "invalid: the blob is malformed, or this is not the schema"
    );
    assert_eq!(
        box_at(&mut app, 1, "pack_size"),
        [
            "pack_size: 5",
            "how many elements this one packed wire record holds"
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
            "repeated",
            "this field may occur more than once",
            "optional is the default and is never printed",
        ]
    );
    assert_eq!(
        box_at(&mut app, 0, "[packed=true]"),
        [
            "[packed=true]",
            "these elements share one length-prefixed record",
        ]
    );
    assert_eq!(
        box_at(&mut app, 0, "= 85"),
        [
            "= 85",
            "the field's number in its .proto message",
            "the number is on the wire; the name is not",
        ],
        "the sign and the digits are one target, so one answer"
    );
    assert_eq!(box_at(&mut app, 0, "#@")[0], "#@");

    // `required` is the other label, and says which dialect it is from.
    let mut app = doc_app(&["q: 1  #@ required int32 = 1"]);
    assert_eq!(
        box_at(&mut app, 0, "required")[1],
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
        [
            "bytes",
            "wire type 2 — a length prefix, then that many bytes"
        ],
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
        ["name", "the field's name, from the schema"]
    );
    assert_eq!(
        box_at(&mut app, 1, "999"),
        ["999", "a field number: no schema declared this field"]
    );
    assert_eq!(
        box_at(&mut app, 2, "[acme.blades]"),
        [
            "[acme.blades]",
            "an extension field, named by its full path"
        ]
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
        ["1", "the field's value, as protoc --decode prints it"],
        "the value is the token; the annotation behind it is not part \
         of it, and nothing is decoded"
    );
    assert_eq!(
        box_at(&mut app, 1, "GREEN"),
        [
            "GREEN",
            "the field's value, as protoc --decode prints it",
            "the schema's name for the 2 on the wire",
        ]
    );
    assert_eq!(
        box_at(&mut app, 2, "0x4005bf0a")[2],
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
            "#@",
            "opens a prototext annotation: the rest of this line",
            "is how the bytes were encoded, not part of the message",
            "prototext is textproto, plus these annotations",
        ],
        "the marker introduces the annotation rather than being it"
    );
}
