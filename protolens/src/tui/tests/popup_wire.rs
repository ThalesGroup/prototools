// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0282: the box a wire byte earns.

use super::super::popup::HoverTarget;
use super::super::wire::{PackedCursor, Region, WireHit, WirePalette, WirePart};
use super::super::*;
use super::support::*;
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::FileDescriptorSet;
use ratatui::layout::Rect;

/// How tall a box may be in these tests unless the test is *about* the
/// height — comfortably more than any body S6-S13 builds.
const TALL: usize = 40;

/// `test.Sample`, the fixture every byte-shape test below encodes
/// against: one field per wire type this spec has a family for.
fn sample_fds() -> FileDescriptorSet {
    proto3_fds(
        "test_popup_wire.proto",
        vec![message(
            "Sample",
            vec![
                field("name", 1, Label::Optional, Type::String),
                field("d", 2, Label::Optional, Type::Double),
                field("f", 3, Label::Optional, Type::Float),
                field("raw", 4, Label::Optional, Type::Bytes),
                field("n", 5, Label::Optional, Type::Int32),
            ],
        )],
    )
}

/// An app over `blob`, with wire mode on for every line and a pane wide
/// enough that no row is panned off the right edge.
fn wire_app(fds: &FileDescriptorSet, root: &str, blob: &[u8]) -> App {
    let mut app = fixture_under("popup-wire", fds, root, blob);
    app.splash = false;
    app.main_area = Rect::new(0, 0, 240, 40);
    let last = app.total_lines() - 1;
    let span = app.wire_span_of_lines(0, last);
    let probe = app.cursor_display_row();
    app.set_wire_span(span, probe);
    app
}

/// `blob` read as `test.Sample`. Line 0 is the synthetic wrapper root,
/// which claims no bytes of the user's file (0225 S3), so the first
/// field is on line 1.
fn sample_app(blob: &[u8]) -> App {
    wire_app(&sample_fds(), "test.Sample", blob)
}

/// The same, for `packed_run_with_tail_fixture`'s richer document.
fn packed_app() -> App {
    let (mut app, _run, _tail, _a, _b) = packed_run_with_tail_fixture();
    app.splash = false;
    app.main_area = Rect::new(0, 0, 240, 40);
    let last = app.total_lines() - 1;
    let span = app.wire_span_of_lines(0, last);
    let probe = app.cursor_display_row();
    app.set_wire_span(span, probe);
    app
}

/// Line `line`'s wire row as plain text — the string whose character
/// indices are the painter's own columns, which is what the hit test
/// looks a column up in.
fn hex(app: &App, line: usize) -> String {
    let pos = app.line_pos(line).expect("a drawn line");
    let palette = WirePalette::for_test();
    let spans = app
        .wire_row(
            pos,
            // Spec 0328 S5: the margin a bar-free wire row carries, which
            // is what the `trim_start` below then discards.
            vec![ratatui::text::Span::raw(
                " ".repeat(render::FOLD_FIELD_WIDTH),
            )],
            &mut PackedCursor::default(),
            Some(&palette),
        )
        .expect("that line claims bytes");
    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
    let body = text.trim_start();
    body.strip_prefix(wire::WIRE_CONNECTOR)
        .unwrap_or(body)
        .trim_end()
        .to_string()
}

/// The pane point that lands on hex column `col` of line `line`'s wire
/// row.
fn point(app: &App, line: usize, col: usize) -> (u16, u16) {
    let row = (0..app.main_area.height)
        .find(|r| {
            app.main_pane_line_part(app.main_area.x + render::HEAT_FIELD_WIDTH as u16, *r)
                == Some((line, 1))
        })
        .expect("that line's wire row is on screen");
    let pos = app.line_pos(line).expect("a drawn line");
    // The line's own indent, measured where `render` measures it — and
    // not on `row_content`, which has the fold field in front of it
    // already.
    let text = app.display_row_text(app.committed_row_at(line, pos));
    let indent = text.len() - text.trim_start().len();
    let index = render::FOLD_FIELD_WIDTH + indent + wire::WIRE_CONNECTOR.chars().count() + col;
    (
        app.main_area.x + render::HEAT_FIELD_WIDTH as u16 + (index - app.pan_offset) as u16,
        app.main_area.y + row,
    )
}

fn hit(app: &App, line: usize, col: usize) -> Option<WireHit> {
    let (column, row) = point(app, line, col);
    app.wire_part_at(column, row)
}

/// The box a hover at hex column `col` opens, in at most `avail` lines.
fn lines_at(app: &App, line: usize, col: usize, avail: usize) -> Vec<BoxLine> {
    let hit = hit(app, line, col).expect("that column names a part");
    let popup = Popup {
        body: PopupBody::Wire(app.wire_box(&hit)),
        anchor: (0, 0),
    };
    App::popup_lines(&popup, avail)
}

/// The same, as the text alone — what every test that is not about
/// spec 0283's mark wants.
fn box_at(app: &App, line: usize, col: usize, avail: usize) -> Vec<String> {
    lines_at(app, line, col, avail)
        .into_iter()
        .map(|line| line.text)
        .collect()
}

/// The mark spec 0283 puts on the line whose text starts with `name`.
fn mark_of(app: &App, line: usize, col: usize, name: &str) -> Option<Range<usize>> {
    lines_at(app, line, col, TALL)
        .into_iter()
        .find(|line| line.text.starts_with(name))
        .unwrap_or_else(|| panic!("no {name} line"))
        .mark
}

/// The characters `mark` covers of that same line.
fn marked_text(app: &App, line: usize, col: usize, name: &str) -> String {
    let at = mark_of(app, line, col, name).expect("a mark");
    lines_at(app, line, col, TALL)
        .into_iter()
        .find(|line| line.text.starts_with(name))
        .expect("the line")
        .text
        .chars()
        .skip(at.start)
        .take(at.end - at.start)
        .collect()
}

/// The first hex column of `needle` in line `line`'s wire row.
fn col_of(app: &App, line: usize, needle: &str) -> usize {
    let row = hex(app, line);
    let at = row.find(needle).expect("the row draws it");
    row[..at].chars().count()
}

/// Spec 0282 test plan 1 / S6: the wire-type digit names its wire type
/// and the type the bytes are read as; the bar beside it names nothing.
#[test]
fn a_hover_over_the_type_digit_names_the_wire_type() {
    let app = packed_app();
    // Line 7 is `a: 42`, drawn `0|18[2a]`.
    assert_eq!(hex(&app, 7), "0|18[2a]");

    let lines = box_at(&app, 7, 0, TALL);
    assert_eq!(lines[0], "wire type 0 — VARINT");
    assert_eq!(lines[1], "reads as: int32");
    assert_eq!(
        hit(&app, 7, 0).map(|h| h.part),
        Some(WirePart::Region(Region::Type))
    );

    // N1: the bar is punctuation between the two halves and carries no
    // byte, so pointing at it points at nothing.
    assert!(hit(&app, 7, 1).is_none(), "the bar is not a target");

    // Line 4 is `tail {`, a LEN record — the other spelling.
    assert_eq!(box_at(&app, 4, 0, TALL)[0], "wire type 2 — LEN");
    assert_eq!(box_at(&app, 4, 0, TALL)[1], "reads as: test.Inner");
}

/// Spec 0282 test plan 2 / S7: the field number, and the name only when
/// a schema gives one.
#[test]
fn a_hover_over_the_field_number_names_the_field() {
    let app = packed_app();
    assert_eq!(box_at(&app, 7, 2, TALL)[0], "field 3 — \"a\"");
    assert_eq!(
        hit(&app, 7, 2).map(|h| h.part),
        Some(WirePart::Region(Region::Tag))
    );

    // A number this schema does not declare: no placeholder name, and
    // the line below says that is why.
    let app = sample_app(&[0x40, 0x07]);
    let lines = box_at(&app, 1, 2, TALL);
    assert_eq!(lines[0], "field 8");
    assert_eq!(lines[1], "no schema declares this field");
}

/// Spec 0282 test plan 3 / S3: a tag's continuation bytes are the same
/// recorded part as its first byte's number half, so the dwell does not
/// restart as the pointer slides across them.
#[test]
fn a_multi_byte_tag_is_one_field_number_target() {
    // Field 20 is a two-byte tag: 0xA0 0x01, wire type varint.
    let app = sample_app(&[0xA0, 0x01, 0x07]);
    assert_eq!(hex(&app, 1), "0|a0 01[07]");

    let first = hit(&app, 1, 2).expect("the first byte's number half");
    let second = hit(&app, 1, 5).expect("the continuation byte");
    assert_eq!(first.part, WirePart::Region(Region::Tag));
    // Everything but `byte`, which spec 0283 S2 added and which is
    // *supposed* to differ: these are two bytes of one part.
    assert!(first.same_part(&second), "one part, so one target");
    assert_ne!(first.byte, second.byte, "but two bytes of it");
    assert_eq!(box_at(&app, 1, 2, TALL)[0], "field 20");
}

/// Spec 0282 test plan 4 / S8: the length prefix, and `pack_size` shown
/// as the landmark it is rather than as a flaw.
#[test]
fn a_hover_over_the_length_names_its_value() {
    let app = packed_app();
    // Line 1 carries the packed record's own tag and length.
    assert_eq!(hex(&app, 1), "2|08:03[05");
    let lines = box_at(&app, 1, 5, TALL);
    assert_eq!(lines, ["length 3 bytes — packed run"]);
    assert_eq!(
        hit(&app, 1, 5).map(|h| h.part),
        Some(WirePart::Region(Region::Len))
    );

    // An ordinary LEN record says only how long it is.
    assert_eq!(hex(&app, 4), "2|10:02[");
    assert_eq!(box_at(&app, 4, 5, TALL), ["length 2 bytes"]);
}

/// Spec 0282 test plan 5 / S9: every proto type the wire type admits,
/// the declared one first and marked.
#[test]
fn a_varint_value_lists_every_varint_type() {
    let app = packed_app();
    let lines = box_at(&app, 7, col_of(&app, 7, "2a"), TALL);
    assert_eq!(
        lines,
        [
            "int32    42  ← declared",
            "int64    42",
            "uint32   42",
            "uint64   42",
            "sint32   21",
            "sint64   21",
            "bool     true",
        ]
    );
}

/// Spec 0282 test plan 6 / S9: the I64 and I32 families, through the
/// same formatters the document itself uses.
#[test]
fn a_fixed_value_reads_as_a_double_and_as_a_float() {
    // d = 1.5 (I64), f = 1.5 (I32).
    let app = sample_app(&[
        0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xF8, 0x3F, //
        0x1D, 0x00, 0x00, 0xC0, 0x3F,
    ]);

    let lines = box_at(&app, 1, col_of(&app, 1, "00"), TALL);
    assert_eq!(
        lines,
        [
            "double   1.5  ← declared",
            "fixed64  4609434218613702656",
            "sfixed64 4609434218613702656",
        ]
    );

    let lines = box_at(&app, 2, col_of(&app, 2, "00"), TALL);
    assert_eq!(
        lines,
        [
            "float    1.5  ← declared",
            "fixed32  1069547520",
            "sfixed32 1069547520",
        ]
    );
}

/// Spec 0282 test plan 7 / S9: a LEN payload reads as a string and as
/// bytes, and says so rather than showing lossy text when it is not
/// valid UTF-8.
#[test]
fn a_len_payload_reads_as_a_string_and_as_bytes() {
    let app = sample_app(&[0x0A, 0x05, b'h', b'e', b'l', b'l', b'o']);
    let lines = box_at(&app, 1, col_of(&app, 1, "68"), TALL);
    assert_eq!(
        lines,
        ["string   \"hello\"  ← declared", "bytes    \"hello\""]
    );

    // Field 4 is `bytes`, so the declared reading is the byte one and
    // the string reading is the alternative that cannot be had.
    let app = sample_app(&[0x22, 0x02, 0xFF, 0xFE]);
    let lines = box_at(&app, 1, col_of(&app, 1, "ff"), TALL);
    assert_eq!(
        lines,
        [
            "bytes    \"\\377\\376\"  ← declared",
            "string   not valid UTF-8"
        ]
    );
}

/// Spec 0282 test plan 8 / S4: a flaw is filed under the part it is
/// about, not under the part the pen was in when it was found.
#[test]
fn a_flawed_part_names_its_keyword() {
    // `n = 7` with a redundant continuation byte on the *value*.
    let app = sample_app(&[0x28, 0x87, 0x00]);
    assert_eq!(hex(&app, 1), "0|28[87 00]");

    let value = box_at(&app, 1, col_of(&app, 1, "87"), TALL);
    assert_eq!(
        value.last().map(String::as_str),
        Some("val_ohb — this varint is padded, not minimal")
    );
    // The tag beside it is well formed and says nothing about the
    // padding of the value.
    assert_eq!(box_at(&app, 1, 2, TALL), ["field 5 — \"n\""]);
    assert!(!box_at(&app, 1, 0, TALL)
        .iter()
        .any(|l| l.contains("val_ohb")));
}

/// Spec 0285 test plan 2 / G2: a keyword reads the same in both boxes,
/// because there is one copy of the sentence.
///
/// The fixture is `a_flawed_part_names_its_keyword`'s, and that is the
/// point: one blob draws `val_ohb` on the wire row and again in the
/// document row's `#@` annotation, so the two boxes can be opened over
/// the same fault and compared. What would drift is a second table;
/// what this asserts is that the wire box's line is the document box's
/// clause with the keyword written in front of it.
#[test]
fn a_keyword_reads_the_same_in_both_boxes() {
    let mut app = sample_app(&[0x28, 0x87, 0x00]);

    let wire = box_at(&app, 1, col_of(&app, 1, "87"), TALL)
        .last()
        .cloned()
        .expect("the padded varint has a flaw line");

    // The same line's *document* row, which spells the same keyword.
    let pos = app.line_pos(1).expect("a drawn line");
    let content = app.row_content(app.committed_row_at(1, pos));
    let at = content.find("val_ohb").expect("the annotation names it");
    let row = (0..app.main_area.height)
        .find(|r| {
            app.main_pane_line_part(app.main_area.x + render::HEAT_FIELD_WIDTH as u16, *r)
                == Some((1, 0))
        })
        .expect("its document row is on screen");
    let column = app.main_area.x
        + render::HEAT_FIELD_WIDTH as u16
        + (content[..at].chars().count() - app.pan_offset) as u16;

    let hit = app
        .doc_element_at_point(column, app.main_area.y + row)
        .expect("the keyword is a target");
    let doc: Vec<String> = popup_doc::doc_lines(&hit, heat_cue::HEAT_ANCHOR_DEFAULT)
        .into_iter()
        .map(|line| line.text)
        .collect();

    let clause = doc.last().expect("the box explains it");
    assert_eq!(wire, format!("val_ohb — {clause}"), "{doc:?}");
}

/// Spec 0282 test plan 9 / S10: the flaws outrank the alternatives, so
/// a short pane keeps them and drops the readings instead.
#[test]
fn a_short_terminal_keeps_the_flaws_and_drops_the_readings() {
    let app = sample_app(&[0x28, 0x87, 0x00]);
    let col = col_of(&app, 1, "87");

    let tall = box_at(&app, 1, col, TALL);
    assert_eq!(tall.len(), 8, "seven readings and the flaw: {tall:?}");

    let short = box_at(&app, 1, col, 4);
    assert_eq!(short.len(), 4);
    assert_eq!(short[0], tall[0], "the declared reading is the answer");
    assert_eq!(short[2], "…", "and the `…` stands before the flaws");
    assert_eq!(
        short.last(),
        tall.last(),
        "the same flaw survives the cut (S10)"
    );

    // With no flaw at all, the `…` is simply the last line.
    let app = sample_app(&[0x28, 0x07]);
    let short = box_at(&app, 1, col_of(&app, 1, "07"), 3);
    assert_eq!(short.len(), 3);
    assert_eq!(short.last().map(String::as_str), Some("…"));
}

/// Spec 0282 test plan 10 / S11: the `??` says what is missing, and
/// carries the same accusation the row's coloring does.
#[test]
fn a_hover_over_the_truncation_mark_says_what_is_missing() {
    // `name` declares five bytes and the blob holds two.
    let app = sample_app(&[0x0A, 0x05, b'h', b'e']);
    let row = hex(&app, 1);
    assert!(row.contains("??×3"), "{row}");

    let col = col_of(&app, 1, "??");
    assert!(matches!(
        hit(&app, 1, col).map(|h| h.part),
        Some(WirePart::Truncated {
            missing: Some(3),
            ..
        })
    ));
    let lines = box_at(&app, 1, col, TALL);
    assert_eq!(lines[0], "?? — bytes the message does not contain");
    assert_eq!(
        lines[1],
        "this record needs 3 more bytes; the blob ends here"
    );
    assert_eq!(
        lines.last().map(String::as_str),
        Some("TRUNCATED_BYTES — the declared length runs past the end of the message")
    );

    // The count is glued to the mark, so it is the same target.
    assert_eq!(hit(&app, 1, col), hit(&app, 1, col + 2));
}

/// Spec 0282 test plan 11 / S12: the elision counts its own hidden
/// bytes, names no flaw, and names something that actually works.
#[test]
fn a_hover_over_the_elision_mark_counts_the_hidden_bytes() {
    let mut blob = vec![0x22, 0x50];
    blob.extend(std::iter::repeat_n(0x41u8, 0x50));
    let app = sample_app(&blob);

    let col = col_of(&app, 1, "…");
    let hit = hit(&app, 1, col).expect("the mark is a target");
    let WirePart::Elided { hidden } = hit.part else {
        panic!("the mark is an elision: {:?}", hit.part);
    };
    assert!(hex(&app, 1).ends_with(&format!("…×{hidden}")));

    let lines = box_at(&app, 1, col, TALL);
    assert_eq!(lines.len(), 2, "no flaw line: eliding is not one");
    assert_eq!(lines[0], format!("…×{hidden} — {hidden} bytes not shown"));
    assert!(lines[1].contains(":export --binary"), "{lines:?}");
    assert!(
        !lines[1].contains(" W ") && !lines[1].contains(" w "),
        "the cap is per row, so `w`/`W` do not reveal them: {lines:?}"
    );
}

/// Spec 0282 test plan 12 / S2: which target a point names is decided by
/// which terminal row of the pair it is in.
#[test]
fn the_document_row_of_a_pair_still_hovers_its_type() {
    let mut app = packed_app();
    // The `int32` of line 7's `#@ int32 = 3` annotation.
    let content = app.row_content(app.committed_row(7).expect("a drawn row"));
    let at = content.find("int32").expect("the row declares its type");
    let column = render::HEAT_FIELD_WIDTH as u16 + content[..at].chars().count() as u16;
    let document = (0..app.main_area.height)
        .find(|r| app.main_pane_line_part(render::HEAT_FIELD_WIDTH as u16, *r) == Some((7, 0)))
        .expect("the document row is on screen");

    app.handle_mouse(moved(column, document));
    assert!(
        matches!(
            app.hover.as_ref().map(|h| &h.target),
            Some(HoverTarget::Type(_))
        ),
        "part 0 keeps spec 0280's target"
    );

    let (column, row) = point(&app, 7, 0);
    app.handle_mouse(moved(column, row));
    assert!(
        matches!(
            app.hover.as_ref().map(|h| &h.target),
            Some(HoverTarget::Wire(_))
        ),
        "part 1 takes this spec's"
    );
}

/// Spec 0282 test plan 13 / 0280 G5: arming the dwell over a wire part
/// is not a visible change either.
#[test]
fn a_wire_hover_costs_no_frame() {
    let mut app = packed_app();
    let (column, row) = point(&app, 7, 0);

    app.event_changed_nothing = false;
    app.handle_mouse(moved(column, row));
    assert!(app.hover.is_some(), "the wire type arms the dwell");
    assert!(app.event_changed_nothing);

    // Sliding along one part must not restart the dwell (S1).
    let value = col_of(&app, 7, "2a");
    let (column, row) = point(&app, 7, value);
    app.handle_mouse(moved(column, row));
    let deadline = app.hover_deadline.expect("the payload arms it too");
    let (column, row) = point(&app, 7, value + 1);
    app.handle_mouse(moved(column, row));
    assert_eq!(app.hover_deadline, Some(deadline), "still the same part");
}

/// Spec 0282 test plan 14 / S5: the column that lands on a given byte
/// shifts by exactly the pan.
#[test]
fn a_panned_wire_row_hits_the_same_byte() {
    let mut app = packed_app();
    let unpanned = hit(&app, 7, 2).expect("the field number");
    app.pan_offset = 3;
    assert_eq!(hit(&app, 7, 2), Some(unpanned));
}

/// S5, against the frame rather than against a model of it: every
/// column of a drawn wire row names the part whose glyphs are actually
/// printed there.
///
/// The helpers above rebuild `render`'s margin arithmetic, so they agree
/// with `wire_part_at` even when both are wrong — which is how the hit
/// test came to measure the line's indent on `row_content`, whose fold
/// field it then subtracted a second time. This reads the terminal.
#[test]
fn every_drawn_hex_column_names_the_byte_under_it() {
    let mut app = sample_app(&[0x0A, 0x05, b'h', b'e', b'l', b'l', b'o']);
    let area = Rect::new(0, 0, 80, 24);
    app.main_area = Rect::new(area.x, area.y, area.width, area.height - 2);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let buffer = terminal.backend().buffer().clone();

    // The wire row of line 1 — the only row on screen whose part is 1.
    let y = (app.main_area.y..app.main_area.y + app.main_area.height)
        .find(|y| {
            app.main_pane_line_part(app.main_area.x + render::HEAT_FIELD_WIDTH as u16, *y)
                == Some((1, 1))
        })
        .expect("the wire row is on screen");
    let drawn: String = (app.main_area.x..app.main_area.x + app.main_area.width)
        .map(|x| buffer[(x, y)].symbol().to_string())
        .collect();
    assert!(
        drawn.contains("2|08:05[68 65 6c 6c 6f]"),
        "the row under test: {drawn:?}"
    );

    // A column, not a byte index: the connector in front of the hex is
    // three characters and nine bytes.
    let column_of = |needle: &str| {
        let at = drawn.find(needle).expect("the row draws it");
        drawn[..at].chars().count() as u16
    };

    // `68` is the first payload byte — the `h` of `hello`, wherever the
    // fixture's wrapper put it.
    let at = column_of("68");
    let hit = app
        .wire_part_at(app.main_area.x + at, y)
        .expect("that column is a byte");
    assert_eq!(hit.part, WirePart::Region(Region::Payload));
    let hello = app
        .blob
        .windows(5)
        .position(|w| w == b"hello")
        .expect("the fixture holds it");
    assert_eq!(hit.bytes, hello..hello + 5);

    // And the two columns of the tag's wire-type digit and bar are the
    // type and nothing, immediately left of the field number.
    let tag = column_of("2|08");
    assert_eq!(
        app.wire_part_at(app.main_area.x + tag, y).map(|h| h.part),
        Some(WirePart::Region(Region::Type))
    );
    assert_eq!(app.wire_part_at(app.main_area.x + tag + 1, y), None);
    assert_eq!(
        app.wire_part_at(app.main_area.x + tag + 2, y)
            .map(|h| h.part),
        Some(WirePart::Region(Region::Tag))
    );
}

/// Spec 0283 test plan 1 / G1: the hovered byte's own escape group is
/// marked, and nothing else in the box is.
#[test]
fn a_hovered_payload_byte_marks_its_escape() {
    // Field 4 is `bytes`, and `ff fe` is not UTF-8 — so the `string`
    // reading has no glyphs to point into (S6).
    let app = sample_app(&[0x22, 0x04, 0x41, 0xFF, 0xFE, 0x42]);
    let col = col_of(&app, 1, "ff");

    assert_eq!(marked_text(&app, 1, col, "bytes"), "\\377");
    let marked: Vec<String> = lines_at(&app, 1, col, TALL)
        .into_iter()
        .filter(|line| line.mark.is_some())
        .map(|line| line.text)
        .collect();
    assert_eq!(
        marked,
        ["bytes    \"A\\377\\376B\"  ← declared"],
        "only the reading the byte spells"
    );
}

/// Spec 0283 test plan 2 / S5: byte by byte, the marks tile the reading
/// between its quotes — adjacent, ordered, no gap and no overlap.
#[test]
fn the_mark_moves_with_the_byte() {
    let app = sample_app(&[0x22, 0x04, 0x41, 0xFF, 0xFE, 0x42]);
    let text = lines_at(&app, 1, col_of(&app, 1, "41"), TALL)
        .into_iter()
        .find(|line| line.text.starts_with("bytes"))
        .expect("the bytes line")
        .text;

    // The opening quote is where the first byte's escape begins.
    let mut next = text.find('"').expect("a quoted reading") + 1;
    for byte in ["41", "ff", "fe", "42"] {
        let mark = mark_of(&app, 1, col_of(&app, 1, byte), "bytes").expect("a mark");
        assert_eq!(mark.start, next, "{byte} starts where {next} left off");
        next = mark.end;
    }
    let close = text.rfind('"').expect("a closing quote");
    assert_eq!(
        next,
        text[..close].chars().count(),
        "and the last one ends at the closing quote"
    );
}

/// Spec 0283 test plan 3 / S6: either byte of a two-byte character marks
/// that whole character, since no shorter piece of the string is what
/// the byte contributed.
#[test]
fn a_multi_byte_character_is_marked_whole() {
    // `name = "éA"`, where `é` is `c3 a9`.
    let app = sample_app(&[0x0A, 0x03, 0xC3, 0xA9, b'A']);
    let lead = col_of(&app, 1, "c3");
    let cont = col_of(&app, 1, "a9");

    assert_eq!(marked_text(&app, 1, lead, "string"), "é");
    assert_eq!(marked_text(&app, 1, cont, "string"), "é");
    assert_eq!(
        mark_of(&app, 1, lead, "string"),
        mark_of(&app, 1, cont, "string")
    );

    // The `bytes` reading still answers at byte resolution: there the
    // two halves are two separate escapes.
    assert_eq!(marked_text(&app, 1, lead, "bytes"), "\\303");
    assert_eq!(marked_text(&app, 1, cont, "bytes"), "\\251");
}

/// Spec 0283 test plan 4 / S6: there are no glyphs to point into when
/// the payload is not a string, so that line carries no mark — while the
/// `bytes` line beside it still does.
#[test]
fn an_invalid_string_reading_marks_nothing() {
    let app = sample_app(&[0x22, 0x02, 0xFF, 0xFE]);
    let col = col_of(&app, 1, "ff");
    assert_eq!(
        lines_at(&app, 1, col, TALL)
            .into_iter()
            .find(|line| line.text.starts_with("string"))
            .map(|line| (line.text, line.mark)),
        Some(("string   not valid UTF-8".to_string(), None))
    );
    assert_eq!(marked_text(&app, 1, col, "bytes"), "\\377");
}

/// Spec 0283 test plan 5 / N1: a varint's bytes are 7-bit groups and no
/// substring of `42` is one of them, so no numeric reading is marked.
#[test]
fn a_varint_reading_carries_no_mark() {
    let app = sample_app(&[0x28, 0x2A]);
    let lines = lines_at(&app, 1, col_of(&app, 1, "2a"), TALL);
    assert!(!lines.is_empty());
    assert!(lines.iter().all(|line| line.mark.is_none()), "{lines:?}");
}

/// Spec 0283 test plan 6 / N2: the tag, type and length boxes say one
/// fact about a whole varint and point at nothing.
#[test]
fn a_tag_box_carries_no_mark() {
    let app = sample_app(&[0x0A, 0x05, b'h', b'e', b'l', b'l', b'o']);
    for col in [0, 2, col_of(&app, 1, "05")] {
        let lines = lines_at(&app, 1, col, TALL);
        assert!(!lines.is_empty());
        assert!(
            lines.iter().all(|line| line.mark.is_none()),
            "column {col}: {lines:?}"
        );
    }
}

/// Spec 0283 test plan 8 / S3: the mark travels with its line, so a line
/// `fit` drops takes its mark with it and the survivors keep theirs
/// pointing at the right characters.
#[test]
fn a_dropped_line_takes_its_mark_with_it() {
    let app = sample_app(&[0x0A, 0x05, b'h', b'e', b'l', b'l', b'o']);
    let col = col_of(&app, 1, "6c");

    let tall = lines_at(&app, 1, col, TALL);
    assert_eq!(tall.len(), 2, "{tall:?}");
    assert!(tall.iter().all(|line| line.mark.is_some()));

    let short = lines_at(&app, 1, col, 1);
    assert!(
        !short.iter().any(|line| line.text.starts_with("bytes")),
        "the alternative is what gets dropped: {short:?}"
    );
    assert_eq!(marked_text(&app, 1, col, "string"), "l");
    assert_eq!(
        short[0].mark, tall[0].mark,
        "and the surviving line's mark is untouched"
    );
}

/// Spec 0283 test plan 9 / S7 + S9: the mark reaches the screen as the
/// style a search gives the match it landed on.
#[test]
fn the_mark_is_the_searchs_current_style() {
    let mut app = sample_app(&[0x22, 0x04, 0x41, 0xFF, 0xFE, 0x42]);
    let (column, row) = point(&app, 1, col_of(&app, 1, "ff"));
    let hit = app.wire_part_at(column, row).expect("the payload");
    app.open_wire_popup(&hit, (column, row));

    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(current_style_cells(&app, &terminal), "\\377");
}

/// Spec 0283 test plan 7 / S8: a mark the box is too narrow to draw
/// whole is dropped rather than clamped — half of `\377` marked would
/// say the byte spells three characters.
#[test]
fn a_mark_past_the_box_edge_is_dropped() {
    let mut app = sample_app(&[0x22, 0x04, 0x41, 0xFF, 0xFE, 0x42]);
    // The last byte, whose escape ends against the box's right edge.
    let (column, row) = point(&app, 1, col_of(&app, 1, "42"));
    let hit = app.wire_part_at(column, row).expect("the payload");
    app.open_wire_popup(&hit, (0, 0));

    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(current_style_cells(&app, &terminal), "B", "room for it");

    let mut narrow = Terminal::new(TestBackend::new(20, 24)).unwrap();
    narrow.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        current_style_cells(&app, &narrow),
        "",
        "and none of it when the box cannot show it whole"
    );
}

/// Spec 0283 S11: the bytes of one payload are one gesture. Sliding
/// across them neither restarts the dwell nor reopens the box — the box
/// stays where it is and re-marks itself on the spot.
#[test]
fn sliding_along_a_payload_keeps_one_box() {
    let mut app = sample_app(&[0x22, 0x04, 0x41, 0xFF, 0xFE, 0x42]);
    let first = point(&app, 1, col_of(&app, 1, "41"));
    let second = point(&app, 1, col_of(&app, 1, "ff"));

    // Two bytes of one part: the dwell keeps running from where it
    // started, so the answer is not postponed forever.
    app.handle_mouse(moved(first.0, first.1));
    let deadline = app.hover_deadline.expect("the payload arms the dwell");
    app.handle_mouse(moved(second.0, second.1));
    assert_eq!(app.hover_deadline, Some(deadline), "still the same part");
    assert!(app.event_changed_nothing, "and still nothing to draw");

    // Once it has expired, the box is open on the byte the pointer is
    // on now.
    app.hover_deadline = Some(Instant::now());
    app.track_hover_dwell();
    let anchor = app.popup.as_ref().expect("the box opened").anchor;
    assert_eq!(marked_of_popup(&app), "\\377");

    // Sliding on: the same box, at the same place, marking the byte
    // under the pointer — and this one *is* owed a frame.
    let third = point(&app, 1, col_of(&app, 1, "fe"));
    app.event_changed_nothing = false;
    app.handle_mouse(moved(third.0, third.1));
    assert!(!app.event_changed_nothing, "the mark moved, so redraw");
    assert_eq!(marked_of_popup(&app), "\\376");
    assert_eq!(
        app.popup.as_ref().map(|p| p.anchor),
        Some(anchor),
        "the same box, not a new one beside it"
    );
    assert_eq!(app.hover_deadline, None, "and no second dwell to wait out");

    // Leaving the part for another one is a new question, and takes the
    // box down until its own dwell has run.
    let tag = point(&app, 1, 0);
    app.handle_mouse(moved(tag.0, tag.1));
    assert!(app.popup.is_none());
    assert!(app.hover_deadline.is_some());
}

/// Spec 0307 test 5 / S5: the box over `Blob`'s prefix leads by saying
/// those bytes are not in the file, and no other box says it.
///
/// It leads rather than trails because that is why the reader hovered:
/// the answer to "what is this odd green" comes before "field 1".
#[test]
fn a_hover_over_the_wrapper_prefix_says_the_bytes_are_not_in_the_file() {
    let app = packed_app();
    // Line 0 is the wrapper root: `Blob`'s field-1 tag, then the length
    // of everything after it. Every byte of the row is protolens' own.
    assert_eq!(hex(&app, 0), "2|08:0d[");
    let not_in_the_file =
        "these bytes are not in the file — protolens wraps every document in a field 1";

    for (col, then) in [
        (0, "wire type 2 — LEN"),
        (col_of(&app, 0, "08"), "field 1"),
        (col_of(&app, 0, "0d"), "length 13 bytes"),
    ] {
        let lines = box_at(&app, 0, col, TALL);
        assert_eq!(lines[0], not_in_the_file, "at column {col}");
        assert_eq!(lines[1], then, "at column {col}");
    }

    // Line 1 is `vals: 5`, whose tag is the user's own byte one past
    // the prefix — the nearest thing on screen that must not say it.
    assert_eq!(
        box_at(&app, 1, 0, TALL)[0],
        "wire type 2 — LEN",
        "a byte of the file is not announced as protolens'",
    );
}

/// The characters the open box marks.
fn marked_of_popup(app: &App) -> String {
    let popup = app.popup.as_ref().expect("an open box");
    let line = App::popup_lines(popup, TALL)
        .into_iter()
        .find(|line| line.mark.is_some())
        .expect("a marked line");
    let mark = line.mark.expect("checked just above");
    line.text
        .chars()
        .skip(mark.start)
        .take(mark.end - mark.start)
        .collect()
}

/// The symbols of every cell drawn in `theme::search_current_style`, in
/// reading order.
fn current_style_cells(app: &App, terminal: &Terminal<TestBackend>) -> String {
    let want = crate::theme::search_current_style(app.theme).bg;
    let buffer = terminal.backend().buffer();
    let area = *buffer.area();
    let mut out = String::new();
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            let cell = &buffer[(x, y)];
            if cell.style().bg == want {
                out.push_str(cell.symbol());
            }
        }
    }
    out
}

fn moved(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Moved,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}
