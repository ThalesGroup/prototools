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

use super::super::wire::{wire_spans_recorded, Framing, PackedCursor, WirePalette};
use super::super::*;
use super::support::*;
use crate::decode::{decode, DescriptorContext, RootType};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use prost_types::field_descriptor_proto::{Label, Type};
use ratatui::layout::Rect;

/// Spec 0328 S5: the left margin of a wire row that carries no bar —
/// exactly the blank `wire.rs` used to build for itself, which is what
/// keeps these tests measuring the hex and nothing else.
fn blank_margin(indent: usize) -> Vec<ratatui::text::Span<'static>> {
    vec![ratatui::text::Span::raw(
        " ".repeat(render::FOLD_FIELD_WIDTH + indent),
    )]
}

/// A row's spans as plain text, with the indent and connector of spec
/// 0225 S5 taken off — what these tests assert about is the hex.
fn hex_of(spans: &[ratatui::text::Span<'static>]) -> String {
    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
    let hex = text.trim_start();
    hex.strip_prefix(wire::WIRE_CONNECTOR)
        .unwrap_or(hex)
        .trim()
        .to_string()
}

/// One line's wire row as plain text, indentation trimmed. Empty when
/// the line claims no bytes at all.
fn wire_text(app: &App, line: usize, memo: &mut PackedCursor) -> String {
    let pos = app.line_pos(line).expect("line is inside the document");
    let palette = WirePalette::for_test();
    app.wire_row(pos, blank_margin(0), memo, Some(&palette))
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
            app.preview_wire_row(i, blank_margin(0), Some(&palette))
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

fn ctrl(app: &mut App, c: char) {
    app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
}

/// Spec 0268 test plan 9: the whole document shows its bytes — the span
/// the pre-0268 whole-pane `w` amounted to, and what the viewport tests
/// below were always testing.
///
/// Called a second time it turns them off, because the span's own first
/// line is then already shown (S2). Not a key press: `sibling_leaves_app`
/// makes each record a separate *root*, so there is no one node for `W`
/// to climb to.
fn wire_everything(app: &mut App) {
    let last = app.total_lines() - 1;
    let span = app.wire_span_of_lines(0, last);
    let probe = app.cursor_display_row();
    app.set_wire_span(span, probe);
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

    // Spec 0307 test 3. This root is bracketed, so its head is exactly
    // `Blob`'s synthetic prefix — drawn like any other LEN header, and
    // set in italic rather than withheld. Spec 0225 S3 refused
    // the node and 0306 narrowed the prefix away; both left the row
    // empty, and 0306 left it the one LEN node on screen with no
    // framing.
    assert_eq!(rows[0], "2|08:0d[", "the wrapper root shows its header");

    // S4: the record's tag and length ride on element 0's row, and each
    // later element gets its own row. The run closes on the last one.
    assert_eq!(rows[1], "2|08:03[05");
    assert_eq!(rows[2], "06");
    assert_eq!(rows[3], "07]");

    // A message header shows only its tag and length — its payload is
    // its children's, and so is the closing `]` (test plan 1).
    assert_eq!(rows[4], "2|10:02[");
    assert_eq!(rows[5], "0|18[05]");
    // Every byte under `tail` is claimed by the line above, so its
    // footer row has nothing left to show (test plan 2).
    assert_eq!(rows[6], "");

    assert_eq!(rows[7], "0|18[2a]");
    assert_eq!(rows[8], "0|20[2b]");
    assert_eq!(rows[9], "", "the wrapper root's footer claims no bytes");
}

/// One field 1 declaring 16 payload bytes with only the 5 of `"short"`
/// present, and nothing before it. The probe disqualifies the file
/// (spec 0266) and it renders as a single flat line — which is the shape
/// spec 0306 is about.
///
/// Nothing before the cut is load-bearing: spec 0312 forgives a cut tail
/// once enough whole fields precede it, so a payload with a complete
/// field in front of this one would descend and stop being flat.
const CUT_AT_ONCE: &[u8] = b"\x0a\x10short";

/// An untyped `App` — what opening a file with neither a descriptor set
/// nor a `--type` builds. A twin of `override_message.rs`'s `untyped_app`;
/// kept local because the two files assert about different halves of the
/// same document and neither owns the other's fixture.
fn untyped_wire_app(bytes: &[u8]) -> App {
    let mut ctx = DescriptorContext::empty_for_test();
    let decoded = decode(wrapped(bytes), &mut ctx, RootType::Raw, 2).unwrap();
    fixture_app(decoded, ctx)
}

/// Spec 0307 test 1. The wrapper root is flat here, so its head runs
/// from `Blob`'s two invented bytes to the end of the buffer. Spec 0225
/// S3 refused the node to hide the prefix and threw the file away with
/// it — and since the document is this one line, it threw away every
/// byte the wire view exists to show. Spec 0306 narrowed the prefix off
/// instead, which cost the row its framing.
#[test]
fn w_on_a_flat_wrapper_root_shows_the_whole_file() {
    let app = untyped_wire_app(CUT_AT_ONCE);
    assert_eq!(app.total_lines(), 1, "the probe declines: one flat line");

    let pos = app.line_pos(0).expect("line 0 is inside the document");
    let mut memo = PackedCursor::default();
    let slice = app
        .wire_slice(pos, &mut memo)
        .expect("the wrapper root's payload is the user's file");

    assert_eq!(
        slice.bytes,
        0..app.blob.len(),
        "S1: the whole node, prefix included",
    );
    assert_eq!(
        &app.blob[app.wrapper_offset..],
        CUT_AT_ONCE,
        "and past the prefix those bytes are the file, byte for byte",
    );
    assert!(
        matches!(slice.framing, Framing::Tagged),
        "G1: framed like any other LEN node",
    );
    assert_eq!(
        slice.synthetic_end, app.wrapper_offset,
        "S1: and the prefix is flagged as protolens' own",
    );
}

/// Spec 0307 test 2 — G2 as an assertion rather than as a review rule.
///
/// Over both root shapes, because they draw different amounts of the
/// prefix: the flat root above shows it and the whole file behind it,
/// the bracketed one of `each_line_claims_its_own_bytes` shows the
/// prefix and nothing else. Every fabricated byte must say so, and no
/// byte of the file may.
#[test]
fn a_fabricated_byte_is_drawn_as_one() {
    let flat = untyped_wire_app(CUT_AT_ONCE);
    let (bracketed, _run, _tail, _a, _b) = packed_run_with_tail_fixture();

    for app in [&flat, &bracketed] {
        assert!(app.wrapper_offset > 0, "the fixture must be wrapped");
        let mut memo = PackedCursor::default();
        for line in 0..app.total_lines() {
            let pos = app.line_pos(line).expect("line is inside the document");
            // The same predicate the production guard uses, so the test
            // cannot drift from it by assuming the root is slot 0.
            if app.parent(pos.node).is_some() {
                continue;
            }
            let Some(slice) = app.wire_slice(pos, &mut memo) else {
                continue;
            };
            let row = wire_spans_recorded(
                &app.blob,
                &slice,
                ThemeKind::Dark,
                Some(&WirePalette::for_test()),
            );
            let record = row.record.as_ref().expect("a recorded run notes its cells");
            // One (glyph, style) per drawn column, so a cell's columns
            // can be looked up whatever the spans they were split
            // across — a tag byte is three of them: `2`, `|` and `08`.
            // The separator space between two hex pairs is one of a
            // cell's columns and carries no glyph, so nothing it wears
            // says anything.
            let cols: Vec<(char, Style)> = row
                .spans
                .iter()
                .flat_map(|span| span.content.chars().map(|c| (c, span.style)))
                .collect();
            for cell in &record.cells {
                let fabricated = cell.at < app.wrapper_offset;
                for col in cell.cols.clone() {
                    let (glyph, style) = cols[col];
                    if glyph == ' ' {
                        continue;
                    }
                    assert_eq!(
                        style.add_modifier.contains(theme::SYNTHETIC),
                        fabricated,
                        "column {col} (`{glyph}`) of byte {} on line \
                         {line} is drawn {style:?}, and the synthetic \
                         prefix is the first {} bytes",
                        cell.at,
                        app.wrapper_offset,
                    );
                }
            }
        }
    }
}

/// A row's hex, with every split tag byte put back together: a tag's
/// first byte is drawn as `2|08`, which is the byte `0x0a` spelled as
/// its wire type and the rest of it.
fn rejoined_hex(row: &str) -> String {
    let mut out = String::new();
    let mut chars = row.chars().peekable();
    while let Some(c) = chars.next() {
        if !c.is_ascii_hexdigit() {
            continue;
        }
        if chars.peek() != Some(&'|') {
            out.push(c);
            continue;
        }
        chars.next();
        let hi = chars.next().expect("a split tag has its second half");
        let lo = chars.next().expect("a split tag has its second half");
        let rest = u8::from_str_radix(&format!("{hi}{lo}"), 16).expect("hex");
        let wtype = c.to_digit(16).expect("hex") as u8;
        out.push_str(&format!("{:02x}", rest | wtype));
    }
    out
}

/// Test plan 9, and the reason S2 is stated as a partition rather than
/// as three cases: concatenated in document order, the wire rows are
/// the blob.
#[test]
fn every_byte_appears_exactly_once_in_document_order() {
    let (app, _run, _tail, _a, _b) = packed_run_with_tail_fixture();
    let hex: String = wire_rows(&app)
        .iter()
        .map(|row| rejoined_hex(row))
        .collect();
    // The whole blob, `Blob`'s prefix included (spec 0307 test 4). That
    // is the partition `head_or_tail` guarantees by construction, and
    // the wrapper root was its one exception for as long as its prefix
    // was withheld.
    let expected: String = app.blob.iter().map(|b| format!("{b:02x}")).collect();
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
    assert_eq!(wire_text(&app, header, &mut memo), "3|28");
    assert_eq!(wire_text(&app, footer, &mut memo), "4|28");
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
    assert_eq!(committed, ["2|08:02[", "0|08[05]", ""]);

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
    assert_eq!(committed, ["2|08:03[05", "06", "07]"]);

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
    wire_everything(&mut app);
    assert!(app.wire.is_some());
    assert_eq!(app.document_pane_height(), 5);
    wire_everything(&mut app);
    assert!(app.wire.is_none());
    assert_eq!(app.document_pane_height(), 10);

    // A pane one row tall still shows a line, cut in half or not.
    app.main_area = Rect::new(0, 0, 40, 1);
    wire_everything(&mut app);
    assert_eq!(app.document_pane_height(), 1);
}

/// Spec 0268 G3: a span shorter than the document costs the pane only
/// the rows it covers — three lines showing their bytes take three rows
/// away, not half the pane.
#[test]
fn rows_outside_the_run_stay_one_terminal_row() {
    let texts: Vec<String> = (0..24).map(|i| format!("f{i}: 0")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut app = sibling_leaves_app(&refs);
    app.main_area = Rect::new(0, 0, 40, 10);

    let span = app.wire_span_of_lines(2, 4);
    app.set_wire_span(span, 2);
    assert_eq!(app.document_pane_height(), 7, "three lines cost three rows");
    // The whole-document span is what halves it, and nothing less does.
    wire_everything(&mut app);
    assert_eq!(app.document_pane_height(), 5);
}

/// Which visible rows show their bytes, asked the way the renderer asks
/// it: spec 0268 S4's height map is the one answer both it and the
/// viewport read.
fn shown_rows(app: &App) -> Vec<usize> {
    let heights = app.row_heights();
    (0..app.composed_row_count())
        .filter(|&row| heights.height(row) > 1)
        .collect()
}

/// The document rows drawn with their bytes underneath, read back off a
/// real frame. Spec 0225 S5's connector points *up* at the row whose
/// bytes it shows, so the row above each wire row is the answer.
fn drawn_wire_rows(app: &mut App) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let buffer = terminal.backend().buffer();
    let rows: Vec<String> = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect();
    (1..rows.len())
        .filter(|&y| rows[y].contains(wire::WIRE_CONNECTOR))
        .map(|y| rows[y - 1].trim().to_string())
        .collect()
}

/// Spec 0268 S4 × spec 0185 S2: the shown run is resolved in committed
/// rows and read in display rows, so a preview overlay standing in for
/// a block of a different height has to displace it.
///
/// The regression this catches is visible: with a span on, opening the
/// override pane and arrowing down the candidate list moved the bytes
/// off the rows they belonged to and onto whatever now sat that far
/// down, differently for each candidate, because the overlay's height
/// changes with the type being proposed.
///
/// Ten one-line leaves, so committed row `r` is line `r` and the map is
/// the only thing an assertion can be reading. The overlay covers rows
/// 2..5 and is tried shorter than, equal to and longer than them.
#[test]
fn a_preview_overlay_displaces_the_shown_run() {
    let texts: Vec<String> = (0..10).map(|i| format!("f{i}: 0")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();

    // (span lines, expected shown display rows as a function of the
    // overlay's own height) — below the block, above it, and across it.
    /// The span's first and last *line*, and what display rows it must
    /// show for an overlay of a given height.
    type Case = (usize, usize, fn(usize) -> Vec<usize>);
    let cases: [Case; 3] = [
        (6, 7, |len| vec![3 + len, 4 + len]),
        (0, 1, |_| vec![0, 1]),
        (3, 6, |len| (2..=3 + len).collect()),
    ];

    for (first, last, expected) in cases {
        for overlay_len in [1usize, 3, 6] {
            let mut app = sibling_leaves_app(&refs);
            let span = app.wire_span_of_lines(first, last);
            app.set_wire_span(span, first);
            app.preview_overlay = Some(PreviewOverlay {
                first_row: 2,
                covered_rows: 3,
                lines: vec!["x".to_string(); overlay_len],
                spans: Vec::new(),
                bytes: Vec::new(),
                tier: PreviewTier::Clean,
                tier_column: 0,
                ellipsis_row: None,
            });

            assert_eq!(
                shown_rows(&app),
                expected(overlay_len),
                "lines {first}..={last} under an overlay of {overlay_len} row(s)"
            );
        }
    }
}

/// Spec 0185 S2: the overlay's rows are atomic. A run reaching into the
/// block they stand in for shows bytes under *every* one of them — they
/// have no node to be named by, and their spans index the preview's own
/// possibly-truncated buffer rather than the blob, so there is nothing
/// finer to divide them on.
#[test]
fn an_overlay_row_is_in_the_run_or_out_of_it_with_the_rest() {
    let texts: Vec<String> = (0..10).map(|i| format!("f{i}: 0")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut app = sibling_leaves_app(&refs);

    // A single committed row — the first the overlay covers — is enough
    // to put all four of the overlay's rows in the run.
    let span = app.wire_span_of_lines(2, 2);
    app.set_wire_span(span, 2);
    app.preview_overlay = Some(PreviewOverlay {
        first_row: 2,
        covered_rows: 3,
        lines: vec!["x".to_string(); 4],
        spans: Vec::new(),
        bytes: Vec::new(),
        tier: PreviewTier::Clean,
        tier_column: 0,
        ellipsis_row: None,
    });

    assert_eq!(shown_rows(&app), vec![2, 3, 4, 5]);
    // And the committed row after the block is not in it: the run ended
    // at line 2, and widening it to the overlay must not widen it past.
    assert_eq!(app.composed_row_count(), 11);
}

/// Anchor on `first`, caret at the end of `last` — spec 0242's
/// selection, left the way a drag across those rows leaves it.
fn select_lines(app: &mut App, first: usize, last: usize) {
    let start = app.line_pos(first).expect("the first line must exist");
    app.select_anchor = Some(CursorPos {
        node: start.node,
        line_in_node: start.line_in_node,
        column: 0,
    });
    app.select_engaged = true;
    let end = app.line_pos(last).expect("the last line must exist");
    app.cursor = end.node;
    app.cursor_line_in_node = end.line_in_node;
    app.caret_to_line_end();
    app.select_caret = Some(app.cursor_pos());
}

/// Spec 0268 test plan 1, and G1: with nothing selected, `w` shows the
/// bytes of the caret's own line and of no other.
#[test]
fn w_with_no_selection_shows_only_the_caret_line() {
    let (mut app, _run, _tail, _a, _b) = packed_run_with_tail_fixture();
    app.main_area = Rect::new(0, 0, 80, 22);
    for _ in 0..7 {
        app.move_down();
    }
    assert_eq!(app.cursor_line(), 7, "the fixture's `a: 42`");

    press(&mut app, 'w');
    assert_eq!(shown_rows(&app), vec![7]);

    let drawn = drawn_wire_rows(&mut app);
    assert_eq!(drawn.len(), 1, "one wire row on screen: {drawn:?}");
    assert!(
        drawn[0].contains("a: 42"),
        "and it hangs under the caret's line: {drawn:?}"
    );
}

/// Spec 0268 test plan 2: with a selection, `w` shows exactly the
/// selected lines — including, here, the middle of a submessage.
#[test]
fn w_over_a_selection_shows_exactly_those_lines() {
    let (mut app, _run, _tail, _a, _b) = packed_run_with_tail_fixture();
    app.main_area = Rect::new(0, 0, 80, 22);
    select_lines(&mut app, 4, 7);

    press(&mut app, 'w');
    assert_eq!(shown_rows(&app), vec![4, 5, 6, 7]);
}

/// The reader's amendment: the *caret's* line is what decides the new
/// state, and the selection only widens what the decision applies to.
///
/// The discriminating case is a selection whose first line is already
/// lit while the caret's is not: reading the state off the first line
/// would turn the run *off*, and the reader who put the caret on a dark
/// line asked for the opposite.
#[test]
fn the_caret_decides_and_the_selection_follows() {
    let (mut app, _run, _tail, _a, _b) = packed_run_with_tail_fixture();
    app.main_area = Rect::new(0, 0, 80, 22);
    for _ in 0..4 {
        app.move_down();
    }
    press(&mut app, 'w');
    assert_eq!(shown_rows(&app), vec![4]);

    select_lines(&mut app, 4, 7);
    assert_eq!(app.cursor_line(), 7, "the caret is the selection's end");
    press(&mut app, 'w');
    assert_eq!(
        shown_rows(&app),
        vec![4, 5, 6, 7],
        "the caret's line was dark, so the whole selection lights up"
    );

    // And with the caret's line lit, the same gesture turns the run off.
    select_lines(&mut app, 4, 7);
    press(&mut app, 'w');
    assert!(shown_rows(&app).is_empty());
}

/// Spec 0268 test plan 3, and the reader's own example: `W` at the root
/// lights the document, and a `w` anywhere inside that run turns the
/// whole run off rather than carving a hole in it (N1).
#[test]
fn w_inside_a_shown_run_turns_the_whole_run_off() {
    let (mut app, _run, _tail, _a, _b) = packed_run_with_tail_fixture();
    app.main_area = Rect::new(0, 0, 80, 22);

    press(&mut app, 'W');
    assert_eq!(shown_rows(&app), (0..10).collect::<Vec<_>>());

    for _ in 0..7 {
        app.move_down();
    }
    press(&mut app, 'w');
    assert!(shown_rows(&app).is_empty());
    assert!(app.wire.is_none());
    assert!(
        drawn_wire_rows(&mut app).is_empty(),
        "and no row in the frame has bytes under it"
    );
}

/// Spec 0268 S8: `Ctrl-w` clears the run from anywhere, and does
/// nothing at all when there is no run.
///
/// The discriminating case is a caret *outside* the shown run: `w`
/// there would light the caret's own line instead of clearing, which is
/// the gap S8 exists to fill.
#[test]
fn ctrl_w_clears_the_run_from_outside_it() {
    let (mut app, _run, _tail, _a, _b) = packed_run_with_tail_fixture();
    app.main_area = Rect::new(0, 0, 80, 22);
    for _ in 0..4 {
        app.move_down();
    }
    press(&mut app, 'W');
    assert_eq!(shown_rows(&app), vec![4, 5, 6]);

    // Off the run entirely, where `w` would light line 0 rather than
    // put anything away.
    app.move_home();
    assert_eq!(app.cursor_line(), 0);

    ctrl(&mut app, 'w');
    assert!(app.wire.is_none());
    assert!(shown_rows(&app).is_empty());
    assert!(
        drawn_wire_rows(&mut app).is_empty(),
        "and no row in the frame has bytes under it"
    );

    // Again on nothing: still nothing, and no row has become tall.
    ctrl(&mut app, 'w');
    assert!(app.wire.is_none());
    assert!(shown_rows(&app).is_empty());
}

/// Spec 0268 test plan 4, and G2: `W` on a submessage shows that
/// message's own lines — header, body and footer — and leaves its
/// siblings dark.
#[test]
fn capital_w_shows_a_subtree_and_nothing_beside_it() {
    let (mut app, _run, _tail, _a, _b) = packed_run_with_tail_fixture();
    app.main_area = Rect::new(0, 0, 80, 22);
    for _ in 0..4 {
        app.move_down();
    }
    assert_eq!(app.cursor_line(), 4, "the fixture's `tail {{`");

    press(&mut app, 'W');
    assert_eq!(shown_rows(&app), vec![4, 5, 6]);

    // Two rows drawn, not three: a LEN submessage's closing brace claims
    // no bytes of its own — only a group's footer does, which is what
    // `a_group_footer_row_shows_the_end_tag` is about — so its row is
    // two terminal rows tall with the second one blank.
    let drawn = drawn_wire_rows(&mut app);
    assert_eq!(drawn.len(), 2, "the header and the body: {drawn:?}");
    assert!(
        drawn.iter().any(|r| r.contains("id: 5")),
        "the body included: {drawn:?}"
    );
    assert!(
        !drawn.iter().any(|r| r.contains("a: 42")),
        "the sibling excluded: {drawn:?}"
    );
}

/// Spec 0268 test plan 5: with a selection spanning two subtrees, `W`
/// climbs to the deepest node holding both — here from `id: 5`, inside
/// `tail`, and `a: 42` outside it, which meet only at the root.
#[test]
fn capital_w_with_a_selection_climbs_to_the_common_ancestor() {
    let (mut app, _run, _tail, _a, _b) = packed_run_with_tail_fixture();
    app.main_area = Rect::new(0, 0, 80, 22);
    select_lines(&mut app, 5, 7);

    press(&mut app, 'W');
    assert_eq!(
        shown_rows(&app),
        (0..10).collect::<Vec<_>>(),
        "the root is the only node containing both ends"
    );
}

/// Spec 0268 S3's packed-record rule. A packed run's elements are drawn
/// as the message's own fields — `vals: 5` at the same indent as
/// `a: 42`, no header, no brace — so two of them are siblings, and the
/// subtree a `W` over them names is the message. The arena collapsing
/// the run into one node (spec 0216 S22) is an implementation detail the
/// reader is not looking at.
#[test]
fn capital_w_over_two_packed_elements_climbs_to_the_message() {
    let (mut app, run, _tail, _a, _b) = packed_run_with_tail_fixture();
    app.main_area = Rect::new(0, 0, 80, 22);
    select_lines(&mut app, 1, 2);
    assert_eq!(
        app.line_pos(1).unwrap().node,
        run,
        "both ends must be the one packed node, or this proves nothing"
    );
    assert_eq!(app.line_pos(2).unwrap().node, run);

    press(&mut app, 'W');
    assert_eq!(
        shown_rows(&app),
        (0..10).collect::<Vec<_>>(),
        "the run's third element, `tail`, `a` and `b` all come with it"
    );
}

/// The other side of that rule: one element is a *leaf*, exactly as any
/// other single child of the message is, so `W` on it lights that
/// element and nothing else — the same answer `w` gives. The run is not
/// a subtree the reader can see, so `W` never names it.
#[test]
fn capital_w_on_one_packed_element_lights_that_element() {
    for selected in [false, true] {
        let (mut app, _run, _tail, _a, _b) = packed_run_with_tail_fixture();
        app.main_area = Rect::new(0, 0, 80, 22);
        if selected {
            select_lines(&mut app, 2, 2);
        } else {
            for _ in 0..2 {
                app.move_down();
            }
        }

        press(&mut app, 'W');
        assert_eq!(shown_rows(&app), vec![2], "selected: {selected}");
    }
}

/// Spec 0268 test plan 6, and the reason S1's ends are `AnchorLine`s
/// rather than line numbers: `W` on a node that is still baking has to
/// cover the lines that have not arrived yet. `Footer` says "the node's
/// last line, whatever the count has become", so the run grows with it.
///
/// The bake alone no longer makes the document taller — spec 0323 S4
/// and spec 0329 S1 together mean everything it reveals arrives folded,
/// so the rows it adds are rows nobody asked to see. Opening them is
/// the gesture that does, and it is the same question for `W`: rows
/// that did not exist when it was pressed.
#[test]
fn a_growing_subtree_keeps_its_bytes() {
    let mut app = nested_any_fixture();
    app.splash = false;
    app.bounded_confirms = true;
    app.main_area = Rect::new(0, 0, 40, App::MIN_EXPAND_ROWS as u16);
    let root = app.first_node;
    app.splice_override(
        root,
        Some("acme.Level1".to_string()),
        Some(App::MIN_EXPAND_ROWS),
    )
    .expect("a bounded splice must succeed");

    app.cursor = root;
    app.cursor_line_in_node = 0;
    app.wire_subtree();
    let short = app.composed_row_count();
    assert_eq!(shown_rows(&app), (0..short).collect::<Vec<_>>());

    while app.bake_step() != BakeStep::Idle {}
    unfold_every_node(&mut app);
    let full = app.composed_row_count();
    assert!(
        full > short,
        "the baked-then-opened subtree must be taller: {short} -> {full}"
    );
    assert_eq!(
        shown_rows(&app),
        (0..full).collect::<Vec<_>>(),
        "and the run must have grown with the subtree it names"
    );
}

/// Which *terminal* row the cursor is drawn on — the one the user is
/// looking at, and the thing `set_wire_span` is trying to hold still.
fn cursor_terminal_row(app: &App) -> isize {
    app.terminal_row_of(app.cursor_display_row())
}

/// Test plan 27. `w` changes what a document line costs, not where the
/// user is looking: the cursor stays on the terminal row it was on.
#[test]
fn toggling_wire_mode_keeps_the_cursor_on_its_terminal_row() {
    let texts: Vec<String> = (0..40).map(|i| format!("f{i}: 0")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut app = sibling_leaves_app(&refs);
    app.main_area = Rect::new(0, 0, 40, 10);
    for _ in 0..20 {
        app.move_down();
    }
    app.clamp_pan_offset();
    assert_eq!(app.cursor_display_row(), 20);
    assert_eq!(cursor_terminal_row(&app), 9);

    // Nine is odd, so holding it puts the pane's top edge in the middle
    // of a document line — spec 0230's `scroll.skip` is what lets it.
    wire_everything(&mut app);
    assert_eq!(cursor_terminal_row(&app), 9);
    assert_eq!((app.scroll.index, app.scroll.skip), (15, 1));
    // The pane's first row is the second half of line 15, and a click on
    // it still names line 15.
    assert_eq!(app.main_pane_line_idx(0, 0), Some(15));
    assert_eq!(app.main_pane_line_idx(0, 1), Some(16));
    wire_everything(&mut app);
    assert_eq!(cursor_terminal_row(&app), 9);
    assert_eq!((app.scroll.index, app.scroll.skip), (11, 0));

    // An even offset needs no half line, and is held just the same.
    app.scroll.index = 14;
    app.last_cursor_row = None;
    app.clamp_pan_offset();
    assert_eq!(cursor_terminal_row(&app), 6);
    wire_everything(&mut app);
    assert_eq!(cursor_terminal_row(&app), 6);
    assert_eq!((app.scroll.index, app.scroll.skip), (17, 0));
    wire_everything(&mut app);
    assert_eq!(cursor_terminal_row(&app), 6);
}

/// Turning the wire rows *off* wants twice as many document lines above
/// the cursor as were on screen, and near the top of the document they
/// do not exist. Holding the row then means starting the document part
/// way down the pane, which a negative `scroll.skip` is exactly what
/// records — and it is spent again, a row at a time, by the first
/// downward scrolling that reaches it.
#[test]
fn turning_wire_mode_off_near_the_top_pads_above_the_first_line() {
    let texts: Vec<String> = (0..40).map(|i| format!("f{i}: 0")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut app = sibling_leaves_app(&refs);
    app.main_area = Rect::new(0, 0, 40, 10);
    wire_everything(&mut app);
    for _ in 0..3 {
        app.move_down();
    }
    app.clamp_pan_offset();
    assert_eq!(app.cursor_display_row(), 3);
    assert_eq!(app.scroll.index, 0, "all four lines fit in five");
    assert_eq!(cursor_terminal_row(&app), 6);

    wire_everything(&mut app);
    assert_eq!(app.scroll.index, 0, "there is nothing above line 0");
    assert_eq!(app.scroll.skip, -3, "so three blank rows stand in");
    assert_eq!(cursor_terminal_row(&app), 6);
    assert_eq!(app.main_pane_line_idx(0, 0), None, "a blank row is no line");
    assert_eq!(app.main_pane_line_idx(0, 3), Some(0));

    // The padding is not permanent: it is the first thing a downward
    // move eats into once the cursor reaches the bottom of the pane.
    for _ in 0..4 {
        app.move_down();
    }
    app.clamp_pan_offset();
    assert_eq!((app.scroll.index, app.scroll.skip), (0, -2));
    assert_eq!(cursor_terminal_row(&app), 9);
}

/// Spec 0244 test-plan item 4, the reported defect. Turning the wire
/// rows off near the top leaves blank rows above line 0, and the old
/// bound at `0` meant the very next Ctrl-Up threw them away and snapped
/// the viewport back down — an upward key that moved the document *up*
/// the screen. Now the pan continues upward from where the toggle left
/// it.
#[test]
fn pan_up_after_a_wire_toggle_does_not_snap_back() {
    let texts: Vec<String> = (0..40).map(|i| format!("f{i}: 0")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut app = sibling_leaves_app(&refs);
    app.main_area = Rect::new(0, 0, 40, 20);
    wire_everything(&mut app);
    for _ in 0..3 {
        app.move_down();
    }
    app.clamp_pan_offset();
    wire_everything(&mut app);
    assert_eq!(app.scroll_top(), -3, "three blank rows above line 0");

    app.pan_vertical_up();
    assert_eq!(
        app.scroll_top(),
        -3 - PAN_STEP as isize,
        "Ctrl-Up must carry on upward, not snap back to the top"
    );
}

/// Spec 0244 test-plan item 5. The bounds are counted in terminal rows,
/// which in wire mode are half a document line — so each end may settle
/// one row past where a whole-line bound would have stopped it, showing
/// a wire row whose document row is off the top, or a document row whose
/// wire row is off the bottom. Test 6 is why that is the right choice.
#[test]
fn wire_mode_bounds_are_terminal_rows() {
    let texts: Vec<String> = (0..40).map(|i| format!("f{i}: 0")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut app = sibling_leaves_app(&refs);
    app.main_area = Rect::new(0, 0, 40, 10);
    wire_everything(&mut app);

    // 40 lines two rows thick, so the bottom bound is row 79 — odd, and
    // the pane's first row is line 39's wire row alone. Spec 0286 makes
    // it a bound to be pushed through rather than panned to.
    pan_to_the_bound(&mut app, Pannable::Main, true);
    assert_eq!(app.scroll_top(), 40 * 2 - 1);
    assert_eq!((app.scroll.index, app.scroll.skip), (39, 1));
    assert_eq!(app.main_pane_line_idx(0, 0), Some(39));

    // The top bound is `1 - height`, again odd against a two-row line:
    // nine blank rows, then line 0's document row with its wire row
    // already off the bottom.
    app.set_scroll_top(0);
    pan_to_the_bound(&mut app, Pannable::Main, false);
    assert_eq!(app.scroll_top(), 1 - 10);
    assert_eq!((app.scroll.index, app.scroll.skip), (0, -9));
    assert_eq!(app.main_pane_line_idx(0, 8), None, "a blank row is no line");
    assert_eq!(app.main_pane_line_idx(0, 9), Some(0));
}

/// Spec 0244 test-plan item 6, and the reason item 5's bounds are in
/// terminal rows rather than whole document lines: `w` holds the cursor
/// on the terminal row it was drawn on, and at either bound that row is
/// the only content row there is. A whole-line bound would have to move
/// it.
#[test]
fn a_wire_toggle_keeps_the_cursor_row_at_the_bounds() {
    let texts: Vec<String> = (0..40).map(|i| format!("f{i}: 0")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut app = sibling_leaves_app(&refs);
    app.main_area = Rect::new(0, 0, 40, 10);
    for _ in 0..39 {
        app.move_down();
    }

    // The bottom bound: the last line alone on the pane's first row.
    app.set_scroll_top(40 - 1);
    assert_eq!(cursor_terminal_row(&app), 0);
    wire_everything(&mut app);
    assert_eq!(cursor_terminal_row(&app), 0);

    // The top bound: the first line alone on the pane's last row.
    wire_everything(&mut app);
    for _ in 0..39 {
        app.move_up();
    }
    app.set_scroll_top(1 - 10);
    assert_eq!(cursor_terminal_row(&app), 9);
    wire_everything(&mut app);
    assert_eq!(cursor_terminal_row(&app), 9);
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
    wire_everything(&mut app);
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

    wire_everything(&mut app);
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

    wire_everything(&mut app);

    assert!(app.wire.is_some());
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

/// Test plan 20, on the real document: `pack_size` is an ordinary
/// modifier in both rows (spec 0267).
///
/// The two ends of one chain — the rendered text through the grammar,
/// and the blob through `tier_of` — used to meet on a violet accent
/// invented for this one keyword. It counts a record's elements, which
/// accuses nobody of anything, so the annotation says it in the comment
/// color and the wire row draws the length prefix it describes from the
/// same length hue every other record's prefix gets.
#[test]
fn pack_size_and_its_wire_bytes_are_both_ordinary() {
    use crate::colorize::{self, SyntaxRole};

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
    assert_eq!(roles, [SyntaxRole::Comment]);

    let mut memo = PackedCursor::default();
    let pos = app.line_pos(1).expect("line 1 is inside the document");
    let palette = WirePalette::for_test();
    let spans = app
        .wire_row(pos, blank_margin(0), &mut memo, Some(&palette))
        .expect("it claims bytes");
    let prefix = spans
        .iter()
        .find(|span| span.content.as_ref() == "03")
        .unwrap_or_else(|| panic!("the record's length prefix: {}", hex_of(&spans)));
    assert_eq!(prefix.style, palette.len, "row was {}", hex_of(&spans));
    // And nothing on the row is picked out in the foreground.
    assert!(
        spans.iter().all(|span| span.style.fg.is_none()),
        "row was {}",
        hex_of(&spans),
    );
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
    wire_everything(&mut app);
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
/// — no deeper — and points at it with `tree`'s elbow.
#[test]
fn a_wire_row_is_aligned_with_its_document_row() {
    let app = hue_fixture();
    let mut memo = PackedCursor::default();
    // Line 1 is `  name: "id"`, itself indented one level under the
    // wrapper root.
    let display = app.committed_row(1).expect("line 1 is drawn");
    let source = app.display_row_text(display);
    let indent = source.len() - source.trim_start().len();
    let pos = app.line_pos(1).expect("line 1 is inside the document");
    let row: String = app
        .wire_row(pos, blank_margin(indent), &mut memo, None)
        .expect("it claims bytes")
        .iter()
        .map(|s| s.content.as_ref())
        .collect();

    let margin = render::FOLD_FIELD_WIDTH + indent;
    assert_eq!(
        row,
        format!(
            "{}{}2|08:02[69 64]",
            " ".repeat(margin),
            wire::WIRE_CONNECTOR
        )
    );

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
    assert_eq!(palette.tag, theme::banded(name, app.theme));
    // Dimmer, however the terminal can express it: a leveled band when
    // there is a color to level, the DIM attribute when there is not.
    assert!(
        palette.tag.bg != name.fg || palette.tag.add_modifier.contains(Modifier::DIM),
        "the wire row is the dimmer of two",
    );
}

/// Spec 0225 test plan 14: the payload follows the *value*'s color —
/// one color for every kind of value (spec 0232), so a string field and
/// a numeric one paint their bytes alike, and both differ from the tag.
#[test]
fn the_payload_takes_the_values_hue() {
    use crate::colorize::SyntaxRole;
    use crate::theme;

    let mut app = hue_fixture();
    let string_row = drawn_palette(&mut app, 1);
    let number_row = drawn_palette(&mut app, 2);
    let value = theme::banded(
        theme::style_for(SyntaxRole::StringLiteral, app.theme),
        app.theme,
    );
    assert_eq!(string_row.payload, value);
    assert_eq!(number_row.payload, value);
    assert_ne!(string_row.payload, string_row.tag);
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
        theme::banded(theme::style_for(SyntaxRole::Comment, app.theme), app.theme)
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
    wire_everything(&mut app);
    let mut terminal = ratatui::Terminal::new(TestBackend::new(60, 20)).unwrap();

    // Line 1 is the packed run's first element — the one row here whose
    // bytes carry a tier at all.
    let text = app.row_content(app.committed_row(1).expect("line 1 is drawn"));
    let colored = |app: &App, palette: Option<&WirePalette>| {
        let mut memo = PackedCursor::default();
        let pos = app.line_pos(1).expect("the packed run's first element");
        app.wire_row(pos, blank_margin(0), &mut memo, palette)
            .expect("it claims bytes")
            .iter()
            .filter(|s| s.style.fg.is_some() || s.style.bg.is_some())
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

    // Spec 0245 S3: the viewport has to move for spec 0223's clear to
    // apply at all — a pending frame over the *same* window keeps the
    // hints it already has, which is what stops a stalled wheel from
    // flickering the pane gray.
    app.input_pending = true;
    let one_row = app.row_heights().height(app.scroll.index) as isize;
    app.set_scroll_top(app.scroll_top() + one_row);
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
