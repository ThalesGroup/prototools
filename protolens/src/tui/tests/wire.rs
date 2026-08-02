// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0225: wire mode at the level of a whole `App` — which bytes a
//! *document line* claims, and the geometry the toggle changes.
//!
//! `tui/wire.rs`'s own tests cover the painting of a single row from a
//! byte slice handed to it. These cover the other half: that the slice
//! handed to it is the right one, and that the pane is drawn and
//! navigated as pairs of rows once `w` is on.

use super::super::wire::{PackedCursor, WirePalette};
use super::super::*;
use super::support::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use prost_types::field_descriptor_proto::{Label, Type};
use ratatui::layout::Rect;

/// A row's spans as plain text, with the margin and the `\x` label of
/// spec 0225 S5 taken off — what these tests assert about is the hex.
fn hex_of(spans: &[ratatui::text::Span<'static>]) -> String {
    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
    text.trim()
        .strip_prefix("\\x")
        .expect("every wire row is labelled")
        .trim()
        .to_string()
}

/// One line's wire row as plain text, indentation trimmed. Empty when
/// the line claims no bytes at all.
fn wire_text(app: &App, line: usize, memo: &mut PackedCursor) -> String {
    let pos = app.line_pos(line).expect("line is inside the document");
    let palette = WirePalette::for_test();
    app.wire_row(pos, 0, memo, Some(&palette))
        .map(|spans| hex_of(&spans))
        .unwrap_or_default()
}

/// Every line's wire row, in document order.
fn wire_rows(app: &App) -> Vec<String> {
    let mut memo = PackedCursor::default();
    (0..app.total_lines())
        .map(|line| wire_text(app, line, &mut memo))
        .collect()
}

/// The committed wire rows for the lines `idx` draws.
fn node_wire_rows(app: &App, idx: usize) -> Vec<String> {
    let mut memo = PackedCursor::default();
    app.node_lines(idx)
        .map(|line| wire_text(app, line, &mut memo))
        .collect()
}

/// Every wire row of the preview overlay currently on screen.
fn overlay_wire_rows(app: &App) -> Vec<String> {
    let overlay = app.preview_overlay.as_ref().expect("a preview is up");
    let palette = WirePalette::for_test();
    (0..overlay.lines.len())
        .map(|i| {
            app.preview_wire_row(i, 0, Some(&palette))
                .map(|spans| hex_of(&spans))
                .unwrap_or_default()
        })
        .collect()
}

/// Put a preview of `idx` as `candidate` on screen.
fn preview(app: &mut App, idx: usize, candidate: &str) {
    app.override_target = Some(idx);
    app.override_candidates = vec![(candidate.to_string(), None)];
    app.override_highlight = 0;
    app.preview_override_highlight();
}

fn press(app: &mut App, c: char) {
    app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
}

/// Test plan 1/2/5/6/7 in one pass over one fixture, because the point
/// of S2's head/tail rule is that the *same* rule produces all of them.
///
/// The fixture is `Outer { repeated int32 vals = 1 [packed]; Inner tail
/// = 2; int32 a = 3; int32 b = 4 }`, wrapped, so the document is:
///
/// ```text
///   0  <the wrapper root's header>
///   1    vals: 5        <- the packed record's tag and length ride here
///   2    vals: 6
///   3    vals: 7
///   4    tail {
///   5      id: 5
///   6    }
///   7    a: 42
///   8    b: 43
///   9  }
/// ```
#[test]
fn each_line_claims_its_own_bytes() {
    let (app, _run, _tail, _a, _b) = packed_run_with_tail_fixture();
    let rows = wire_rows(&app);

    // S3: the wrapper is protolens' own framing, not the user's blob,
    // so its root row shows nothing.
    assert_eq!(rows[0], "", "the wrapper root claims no bytes (S3)");

    // S4: the record's tag and length ride on element 0's row, and each
    // later element gets its own row. The run closes on the last one.
    assert_eq!(rows[1], "0a:03[05");
    assert_eq!(rows[2], "06");
    assert_eq!(rows[3], "07]");

    // A message header shows only its tag and length — its payload is
    // its children's, and so is the closing `]` (test plan 1).
    assert_eq!(rows[4], "12:02[");
    assert_eq!(rows[5], "18[05]");
    // Every byte under `tail` is claimed by the line above, so its
    // footer row has nothing left to show (test plan 2).
    assert_eq!(rows[6], "");

    assert_eq!(rows[7], "18[2a]");
    assert_eq!(rows[8], "20[2b]");
    assert_eq!(rows[9], "", "the wrapper root's footer claims no bytes");
}

/// Test plan 9, and the reason S2 is stated as a partition rather than
/// as three cases: concatenated in document order, the wire rows are
/// the blob.
#[test]
fn every_byte_appears_exactly_once_in_document_order() {
    let (app, _run, _tail, _a, _b) = packed_run_with_tail_fixture();
    let hex: String = wire_rows(&app)
        .iter()
        .flat_map(|row| row.chars())
        .filter(|c| c.is_ascii_hexdigit())
        .collect();
    let expected: String = app.blob[app.wrapper_offset..]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(hex, expected);
}

/// Test plan 4: a group's END tag is a byte of its own, and the footer
/// row is the only place it can appear.
#[test]
fn a_group_footer_row_shows_the_end_tag() {
    let (app, group) = group_type_fixture();
    let mut memo = PackedCursor::default();
    let header = app.tree[group].span.text_range.start as usize;
    let footer = header + app.tree[group].lines_total as usize - 1;
    // Field 5, START_GROUP then END_GROUP — a group's tags are the
    // whole of their rows: no length, no payload beside them.
    assert_eq!(wire_text(&app, header, &mut memo), "2b");
    assert_eq!(wire_text(&app, footer, &mut memo), "2c");
}

/// Test plan 8, and the reason S9 exists at all: a preview is a
/// proposal to read the same bytes as a different type, so its wire
/// rows have to be the bytes the committed rows it stands in for show.
/// A blank row there would hide the one comparison a reader is making.
#[test]
fn a_preview_overlay_row_shows_the_preview_nodes_bytes() {
    let (mut app, inner, _id) = type_as_fixture();
    // `Outer { inner: Inner { id: 5 } }` — three lines, and the
    // preview re-renders `inner` under its own current type, so the
    // rendering is the same one and the bytes had better be too.
    let committed = node_wire_rows(&app, inner);
    assert_eq!(committed, ["0a:02[", "08[05]", ""]);

    preview(&mut app, inner, "test.Inner");
    assert_eq!(overlay_wire_rows(&app), committed);
}

/// The same for a packed run, whose preview spans are one per element
/// — the boundaries the committed path has to walk the payload for are
/// simply read off them, and the two must still agree.
#[test]
fn a_preview_of_a_packed_run_splits_its_elements_the_same_way() {
    let (mut app, run, _tail, _a, _b) = packed_run_with_tail_fixture();
    let committed = node_wire_rows(&app, run);
    assert_eq!(committed, ["0a:03[05", "06", "07]"]);

    preview(&mut app, run, "uint64");
    assert_eq!(overlay_wire_rows(&app), committed);
}

/// Test plan 23. The pane's capacity is stated in *document lines*, and
/// in wire mode each of those costs two terminal rows.
#[test]
fn the_lines_that_fit_halve_when_wire_mode_is_on() {
    let texts: Vec<String> = (0..12).map(|i| format!("f{i}: 0")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut app = sibling_leaves_app(&refs);
    app.main_area = Rect::new(0, 0, 40, 10);

    assert_eq!(app.document_pane_height(), 10);
    press(&mut app, 'w');
    assert!(app.wire);
    assert_eq!(app.document_pane_height(), 5);
    press(&mut app, 'w');
    assert!(!app.wire);
    assert_eq!(app.document_pane_height(), 10);

    // A pane one row tall still shows a line, cut in half or not.
    app.main_area = Rect::new(0, 0, 40, 1);
    press(&mut app, 'w');
    assert_eq!(app.document_pane_height(), 1);
}

/// Test plan 24. The rows are twice as thick, not separately clickable:
/// a click on the hex selects the line it describes.
#[test]
fn a_click_on_a_wire_row_selects_the_line_above_it() {
    let texts: Vec<String> = (0..12).map(|i| format!("f{i}: 0")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut app = sibling_leaves_app(&refs);
    app.main_area = Rect::new(0, 0, 40, 10);

    assert_eq!(app.main_pane_line_idx(0, 2), Some(2));
    press(&mut app, 'w');
    assert_eq!(app.main_pane_line_idx(0, 2), Some(1));
    assert_eq!(app.main_pane_line_idx(0, 3), Some(1));
    assert_eq!(app.main_pane_line_idx(0, 4), Some(2));
}

/// Test plan 25. A page is what fits on screen, so it halves with the
/// geometry rather than staying at the terminal's row count.
#[test]
fn page_down_advances_by_the_halved_height() {
    let texts: Vec<String> = (0..40).map(|i| format!("f{i}: 0")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut app = sibling_leaves_app(&refs);
    app.main_area = Rect::new(0, 0, 40, 10);

    app.move_page_down();
    assert_eq!(app.cursor_line(), 10);

    press(&mut app, 'w');
    app.move_page_down();
    assert_eq!(app.cursor_line(), 15);
    app.move_page_up();
    assert_eq!(app.cursor_line(), 10);
}

/// Test plan 26. Like `a`, `w` is a display attribute: it must not cost
/// a re-render of the document or a re-score of anything.
#[test]
fn toggling_wire_mode_invalidates_no_cache() {
    let (mut app, _run, _tail, _a, _b) = packed_run_with_tail_fixture();
    app.main_area = Rect::new(0, 0, 40, 10);
    let version = app.structural_version;
    let heat: Vec<_> = app
        .heat_states
        .iter()
        .map(|s| (s.best().map(|b| b.best_score), s.current()))
        .collect();
    let text: Vec<String> = (0..app.total_lines())
        .map(|l| app.row_content(app.committed_row(l).unwrap()))
        .collect();

    press(&mut app, 'w');

    assert!(app.wire);
    assert_eq!(app.structural_version, version);
    let after: Vec<_> = app
        .heat_states
        .iter()
        .map(|s| (s.best().map(|b| b.best_score), s.current()))
        .collect();
    assert_eq!(after, heat);
    let after_text: Vec<String> = (0..app.total_lines())
        .map(|l| app.row_content(app.committed_row(l).unwrap()))
        .collect();
    assert_eq!(after_text, text, "the document rows are untouched");
}

/// Test plan 20, and the whole point of S11's "one classifier, two
/// rows": the annotation and the wire bytes below it are two
/// independent derivations of the same fact — one from rendered text
/// through the grammar, one from the blob through `tier_of` — and a
/// reader is meant to read them as one mark. They land on the same
/// color or they are two marks.
#[test]
fn pack_size_and_its_wire_bytes_share_the_accent() {
    use crate::annotation::Tier;
    use crate::colorize::{self, SyntaxRole};
    use crate::theme;

    let (app, _run, _tail, _a, _b) = packed_run_with_tail_fixture();
    // Line 1 is the packed record's first element — the one whose
    // annotation carries `pack_size` and whose wire row carries the
    // record's tag and length.
    let text = app.row_content(app.committed_row(1).expect("line 1 is drawn"));
    let at = text
        .find("pack_size")
        .unwrap_or_else(|| panic!("the first element's annotation names the record: {text}"));

    let roles: Vec<_> = colorize::colorize(&text)
        .iter()
        .filter(|hint| hint.range.contains(&at))
        .map(|hint| hint.role)
        .collect();
    assert_eq!(roles, [SyntaxRole::AnnotationLandmark]);

    let accent = theme::tier_style(Tier::Landmark, app.theme)
        .fg
        .expect("a tier has a color");
    assert_eq!(
        theme::style_for(SyntaxRole::AnnotationLandmark, app.theme).fg,
        Some(accent),
    );

    let mut memo = PackedCursor::default();
    let pos = app.line_pos(1).expect("line 1 is inside the document");
    let palette = WirePalette::for_test();
    let spans = app
        .wire_row(pos, 0, &mut memo, Some(&palette))
        .expect("it claims bytes");
    let accented: Vec<&str> = spans
        .iter()
        .filter(|span| span.style.fg == Some(accent))
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(accented, ["0", "a", "03"], "row was {}", hex_of(&spans));
}

/// `test.Row { string name = 1; int32 num = 2; }` holding
/// `name: "id"` and `num: 5` — one string-valued row and one
/// number-valued one, which is the least that shows the payload hue
/// following the *value*'s color rather than a fixed one.
fn hue_fixture() -> App {
    let fds = proto3_fds(
        "hue.proto",
        vec![message(
            "Row",
            vec![
                field("name", 1, Label::Optional, Type::String),
                field("num", 2, Label::Optional, Type::Int32),
            ],
        )],
    );
    let mut app = fixture_under(
        "wire-hues",
        &fds,
        "test.Row",
        &[0x0A, 0x02, 0x69, 0x64, 0x10, 0x05],
    );
    app.splash = false;
    app.wire = true;
    app
}

/// The document row `line` draws, and the palette its wire row borrows
/// from it. Only valid after a frame, since the hints are this frame's.
///
/// The text is `display_row_text`, not `row_content`: `window_styles`
/// is keyed on the row's own offsets, and the fold margin `row_content`
/// prepends would shift every one of them.
fn drawn_palette(app: &mut App, line: usize) -> WirePalette {
    let mut terminal = ratatui::Terminal::new(TestBackend::new(80, 20)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let row = app.committed_row(line).expect("line is drawn");
    let text = app.display_row_text(row).into_owned();
    app.wire_palette(line, &text)
        .expect("a settled frame lends the wire row its hues")
}

/// Spec 0225 test plan 10. The row lines up with the text it describes
/// — no deeper — and says what it is.
#[test]
fn a_wire_row_is_labeled_and_aligned_with_its_document_row() {
    let app = hue_fixture();
    let mut memo = PackedCursor::default();
    // Line 1 is `  name: "id"`, itself indented one level under the
    // wrapper root.
    let display = app.committed_row(1).expect("line 1 is drawn");
    let source = app.display_row_text(display);
    let indent = source.len() - source.trim_start().len();
    let pos = app.line_pos(1).expect("line 1 is inside the document");
    let row: String = app
        .wire_row(pos, indent, &mut memo, None)
        .expect("it claims bytes")
        .iter()
        .map(|s| s.content.as_ref())
        .collect();

    let margin = render::FOLD_FIELD_WIDTH + indent;
    assert_eq!(row, format!("{}\\x 0a:02[69 64]", " ".repeat(margin)));

    // And that margin is the document row's own: the two rows start in
    // the same column, the fold gutter included.
    let drawn = app.row_content(display);
    assert_eq!(margin, drawn.len() - drawn.trim_start().len());
}

/// Spec 0225 test plan 13: the tag wears the field name's hue, moved
/// toward the background rather than reused as is.
#[test]
fn the_tag_takes_the_field_names_hue_dimmed() {
    use crate::colorize::SyntaxRole;
    use crate::theme;

    let mut app = hue_fixture();
    let palette = drawn_palette(&mut app, 1);
    let name = theme::style_for(SyntaxRole::Attribute, app.theme);
    assert_eq!(palette.tag, theme::dimmed(name, app.theme));
    // Dimmer, however the terminal can express it: a leveled color when
    // there is a color to level, the DIM attribute when there is not.
    assert!(
        palette.tag.fg != name.fg || palette.tag.add_modifier.contains(Modifier::DIM),
        "the wire row is the dimmer of two",
    );
}

/// Spec 0225 test plan 14: the payload follows the *value*'s color, so a
/// string field and a numeric one do not paint their bytes alike.
#[test]
fn the_payload_takes_the_values_hue() {
    use crate::colorize::SyntaxRole;
    use crate::theme;

    let mut app = hue_fixture();
    let string_row = drawn_palette(&mut app, 1);
    let number_row = drawn_palette(&mut app, 2);
    assert_eq!(
        string_row.payload,
        theme::dimmed(
            theme::style_for(SyntaxRole::StringLiteral, app.theme),
            app.theme
        )
    );
    assert_eq!(
        number_row.payload,
        theme::dimmed(theme::style_for(SyntaxRole::Number, app.theme), app.theme)
    );
    assert_ne!(string_row.payload, number_row.payload);
}

/// Spec 0225 test plan 15. The length prefix borrows the comment
/// *color*, not whatever comment happens to be on the row — so it does
/// not change when `--no-annotations` takes the annotation away.
#[test]
fn the_length_prefix_takes_the_comment_hue_with_or_without_an_annotation() {
    use crate::colorize::SyntaxRole;
    use crate::theme;

    let mut app = hue_fixture();
    let with = drawn_palette(&mut app, 1);
    assert_eq!(
        with.len,
        theme::dimmed(theme::style_for(SyntaxRole::Comment, app.theme), app.theme)
    );

    app.annotations = false;
    let without = drawn_palette(&mut app, 1);
    assert_eq!(without.len, with.len);
}

/// Spec 0225 test plan 16. The wire row's hues are *borrowed* from the
/// document row, so when spec 0223 drops that row to monochrome under
/// queued input there is nothing left to borrow, and the wire row goes
/// gray with it. Two rows describing one field must not disagree about
/// whether it is remarkable.
#[test]
fn a_wire_row_goes_monochrome_with_the_document_row() {
    let (mut app, _run, _tail, _a, _b) = packed_run_with_tail_fixture();
    app.splash = false;
    app.wire = true;
    let mut terminal = ratatui::Terminal::new(TestBackend::new(60, 20)).unwrap();

    // Line 1 is the packed run's first element — the one row here whose
    // bytes carry a tier at all.
    let text = app.row_content(app.committed_row(1).expect("line 1 is drawn"));
    let colored = |app: &App, palette: Option<&WirePalette>| {
        let mut memo = PackedCursor::default();
        let pos = app.line_pos(1).expect("the packed run's first element");
        app.wire_row(pos, 0, &mut memo, palette)
            .expect("it claims bytes")
            .iter()
            .filter(|s| s.style.fg.is_some())
            .count()
    };

    terminal.draw(|frame| app.render(frame)).unwrap();
    let palette = app
        .wire_palette(1, &text)
        .expect("a settled frame lends the wire row its hues");
    assert!(
        colored(&app, Some(&palette)) > 0,
        "the packed record's landmark must be colored"
    );

    app.input_pending = true;
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(
        app.wire_palette(1, &text).is_none(),
        "spec 0223 cleared the document row's hints, so there is none to borrow"
    );
    assert_eq!(
        colored(&app, None),
        0,
        "every span goes gray, the landmark included"
    );
}
