// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::super::*;
use super::support::*;

/// Spec 0126 G2, re-spelled by spec 0242 S8: `Ctrl-Down`/`Ctrl-Up`
/// alias `Ctrl-j`/`Ctrl-k`'s sibling-skip move, no-op-with-message on a
/// childless-of-siblings node either way.
#[test]
fn ctrl_down_up_alias_sibling_skip_move() {
    let mut app = message_node_app();
    app.splash = false;

    let start = app.cursor;
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
    let via_j = app.cursor;

    app.cursor = start;
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL));
    assert_eq!(app.cursor, via_j, "Ctrl-Down must match Ctrl-j's result");

    app.cursor = via_j;
    app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    let via_k = app.cursor;

    app.cursor = via_j;
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL));
    assert_eq!(app.cursor, via_k, "Ctrl-Up must match Ctrl-k's result");
}

/// Spec 0114 §1.1: the virtual encompassing wrapper protobuf makes
/// "the node under the cursor" unambiguous even at the top level, and
/// display coordinates (byte ranges, positional paths) are corrected
/// back to exactly what they were pre-wrap. Mirrors the synthetic
/// `Outer { inner: Inner { id: 5 } }` fixture in
/// `extract::tests::extract_binary_message_round_trips_through_a_fresh_decode`.
#[test]
fn wrapper_offset_and_display_range_restore_pre_wrap_coordinates() {
    use prost_types::field_descriptor_proto::{Label, Type};

    let fds = proto3_fds(
        "test_wrapper_offset.proto",
        vec![
            message(
                "Outer",
                vec![field_of(
                    "inner",
                    1,
                    Label::Optional,
                    Type::Message,
                    ".test.Inner",
                )],
            ),
            message("Inner", vec![field("id", 1, Label::Optional, Type::Int32)]),
        ],
    );

    // Inner: field 1 varint 5 -> tag 0x08, value 0x05.
    // Outer wraps it as field 1 (LEN): tag (1<<3)|2 = 0x0A, len 2.
    let blob = [0x0Au8, 0x02, 0x08, 0x05];

    let app = fixture_under("wrapper-offset", &fds, "test.Outer", &blob);
    // tag(1 byte) + length-varint(1 byte, blob.len() == 4 fits in 1 byte).
    assert_eq!(app.wrapper_offset, 2);
    assert_eq!(app.blob.len(), blob.len() + 2);

    // The level-0 node is the wrapper's sole field, standing in for
    // the entire original message (spec 0114 §1.1) — it did not exist
    // pre-wrap.
    let outer_idx =
        node_with_type(&app, "test.Outer").expect("tree must contain the Outer stand-in node");
    // Its whole-message payload, offset-corrected, is exactly the
    // caller's original blob.
    assert_eq!(app.display_range(outer_idx), 0..blob.len());
    // The wrapper's own node displays as bare "/".
    assert_eq!(app.positional_path(outer_idx), "/");

    let inner_idx =
        node_with_type(&app, "test.Inner").expect("tree must contain the Inner submessage");
    // Byte offsets 2..4 of the *original* blob, not the wrapped one.
    assert_eq!(app.display_range(inner_idx), 2..blob.len());
    // Leading `/1` leg (descent into the wrapper's sole field) is
    // dropped — matches the path this node would have had pre-wrap.
    assert_eq!(app.positional_path(inner_idx), "/1");
}

/// `display_range` on a scalar node starts at the payload, same as a
/// message/group node: the field's own tag (and, for length-delimited
/// scalars, the length prefix) is stripped. A packed-repeated field is
/// the length-delimited case, but `IndexingTextSink::scalar_field`
/// pushes one `NodeSpan` *per element* (spec 0115), each already
/// bare-payload (`packed_record_start: Some(...)`) — so `display_range`
/// on one of those element nodes returns that element's own byte
/// unstripped, not the whole record's tag+length-stripped payload.
#[test]
fn display_range_strips_tag_and_length_for_scalars_including_packed() {
    use prost_types::field_descriptor_proto::{Label, Type};

    let fds = proto3_fds(
        "test_display_range_scalars.proto",
        vec![message(
            "Msg",
            vec![
                field("id", 1, Label::Optional, Type::Int32),
                field("vals", 2, Label::Repeated, Type::Int32),
            ],
        )],
    );

    // id: field 1 varint 5 -> tag 0x08, value 0x05.
    // vals: field 2 (LEN, packed) tag (2<<3)|2 = 0x12, len 3, payload
    // [0x01, 0x02, 0x03] (three varint elements 1, 2, 3).
    let app = fixture_under(
        "display-range-scalars",
        &fds,
        "test.Msg",
        &[0x08u8, 0x05, 0x12, 0x03, 0x01, 0x02, 0x03],
    );

    let id_idx = app
        .nth_child(app.first_node, 0)
        .expect("tree must contain the id field");
    assert!(!app.tree[id_idx].span.is_message);
    // Tag (1 byte) stripped: just the varint value byte.
    assert_eq!(app.display_range(id_idx), 1..2);

    // Spec 0216: the whole packed record is one node, drawing one row
    // per element, so there is one slot to find rather than three.
    let vals_idx = app
        .nth_child(app.first_node, 1)
        .expect("tree must contain the vals record");
    assert_eq!(app.tree[vals_idx].span.field_number, 2);
    assert!(!app.tree[vals_idx].span.is_message);
    assert_ne!(
        app.tree[vals_idx].span.packed_record_start,
        NO_PACKED_RECORD
    );
    assert_eq!(app.tree[vals_idx].lines_total, 3, "one row per element");
    // The record's payload, already bare — no further tag/length
    // stripping applied.
    assert_eq!(app.display_range(vals_idx), 4..7);
}

/// Regression test: the always-reserved heat-cue gutter column (spec
/// 0138 N1) leaves only `main_area.width - 1` columns for line text, so
/// `pan_right`'s clamp must account for it or panning stops one
/// character short of the line's true end.
///
/// Spec 0193 S1 puts the fold field *inside* the panned row rather than
/// beside it, so it lengthens the row `max_visible_line_len` measures —
/// unlike the heat-cue column, which is prepended after panning.
#[test]
fn pan_right_reaches_the_true_end_of_the_longest_visible_line() {
    let line = "x".repeat(50);
    let mut app = sibling_leaves_app(&[&line]);
    app.splash = false;
    app.main_area = Rect::new(0, 0, 10, 5);

    for _ in 0..20 {
        app.pan_right();
    }

    let usable_width = app.main_area.width as usize - 1;
    assert_eq!(
        app.pan_offset,
        line.len() + render::FOLD_FIELD_WIDTH - usable_width,
        "pan_offset must clamp so the last column of the pane shows the \
         line's last character, leaving room for the 1-column heat-cue \
         gutter"
    );
}

/// 2026-07-19 feedback item 1: Ctrl-Down/Ctrl-Up pan the main pane's
/// viewport without moving the cursor, bounded only by the content
/// itself — no longer tied to keeping the cursor's own row in view.
///
/// Spec 0244 test-plan items 2 and 3: the bounds are no longer `0` and
/// `total - height`. Each end leaves exactly one terminal row of content
/// on screen, so the top edge runs from `1 - height` to `total - 1`.
/// 24 sibling leaf lines, a 5-row pane.
///
/// Spec 0286 puts a wall on the way there: an ordinary pan settles on the
/// content's own last line, and the over-pan asserted here is what
/// pushing on past it buys.
#[test]
fn ctrl_up_down_pan_the_main_pane_without_moving_the_cursor() {
    let lines: Vec<String> = (0..24).map(|i| i.to_string()).collect();
    let texts: Vec<&str> = lines.iter().map(String::as_str).collect();
    let mut app = sibling_leaves_app(&texts);
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 5);
    app.cursor = 19;
    app.scroll.index = 15;
    let max_top = 24 - 1;
    let min_top = 1 - 5;
    let natural_max = 24 - 5;

    app.pan_vertical_down();
    assert_eq!(
        app.scroll_top(),
        (15 + PAN_STEP as isize).min(natural_max),
        "must scroll by PAN_STEP, clamped at the content's own last line"
    );
    pan_to_the_bound(&mut app, Pannable::Main, true);
    assert_eq!(
        app.scroll_top(),
        max_top,
        "the last line alone on the pane's first row, and no further"
    );
    assert_eq!(app.cursor, 19, "panning must not move the cursor");

    app.pan_vertical_up();
    assert_eq!(app.scroll_top(), max_top - PAN_STEP as isize);

    app.set_scroll_top(0);
    pan_to_the_bound(&mut app, Pannable::Main, false);
    assert_eq!(
        app.scroll_top(),
        min_top,
        "the first line alone on the pane's last row, and no further"
    );
    app.pan_vertical_up();
    assert_eq!(app.scroll_top(), min_top, "already at the top bound");
    assert_eq!(app.cursor, 19, "panning must not move the cursor");
}

/// Spec 0244 test-plan item 1: an upward pan from the very top of the
/// document leaves blank rows above line 0, and the frame draws them.
#[test]
fn pan_up_may_leave_blank_rows_above_the_first_line() {
    let lines: Vec<String> = (0..24).map(|i| i.to_string()).collect();
    let texts: Vec<&str> = lines.iter().map(String::as_str).collect();
    let mut app = sibling_leaves_app(&texts);
    app.splash = false;
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    assert_eq!(app.scroll_top(), 0, "the document starts at its own top");

    // Spec 0286: the first line is a wall now, so this pushes through it.
    // The blank rows past it are still there — they cost a shove.
    pan_to_the_bound(&mut app, Pannable::Main, false);
    assert!(
        app.scroll_top() < 0,
        "pushing on past the top must leave the document's first line, \
         not refuse to move: {}",
        app.scroll_top()
    );
    let blank = app.scroll_top().unsigned_abs();

    terminal.draw(|f| app.render(f)).unwrap();
    let buffer = terminal.backend().buffer();
    for y in 0..blank as u16 {
        let row: String = (app.main_area.x..app.main_area.x + app.main_area.width)
            .map(|x| buffer[(x, app.main_area.y + y)].symbol().to_string())
            .collect();
        assert_eq!(row.trim(), "", "row {y} must be blank");
    }
    let first = (app.main_area.x..app.main_area.x + app.main_area.width)
        .map(|x| {
            buffer[(x, app.main_area.y + blank as u16)]
                .symbol()
                .to_string()
        })
        .collect::<String>();
    assert!(
        first.contains('0'),
        "the document's first line must sit just under the blank rows: {first:?}"
    );
}

/// Spec 0142: `Down` from `inner`'s header must land on `inner`'s own
/// footer line as a distinct cursor stop, not skip straight to the
/// next node. Layout (`type_as_fixture`):
/// `0:"1 {"(outer) 1:"  inner {"(inner) 2:"    id: 5"(id) 3:"  }"(inner
/// footer) 4:"}"(outer footer, doc's true last line)`.
#[test]
fn down_from_header_reaches_own_footer_line() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.splash = false;
    app.cursor = inner_idx;
    app.cursor_line_in_node = 0;

    app.move_down(); // inner header -> id header
    app.move_down(); // id header -> inner footer

    assert_eq!(app.cursor, inner_idx);
    assert!(
        app.cursor_on_footer(),
        "cursor must be resting on inner's own }} line"
    );
}

/// Spec 0142: `Down` from a node's own footer reaches the document's
/// true last visible line (the root's own footer), not nothing.
#[test]
fn down_from_footer_reaches_the_document_true_last_line() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.splash = false;
    let outer_idx = app.parent(inner_idx).unwrap();
    app.cursor = inner_idx;
    app.cursor_line_in_node = app.tree[inner_idx].lines_total - 1;

    app.move_down();

    assert_eq!(app.cursor, outer_idx);
    assert!(
        app.cursor_on_footer(),
        "cursor must be resting on the root's own }} line"
    );
}

/// Spec 0142: `Up` is the exact mirror of `Down` across header/footer
/// stops.
#[test]
fn up_from_footer_and_from_next_header_are_symmetric_with_down() {
    let (mut app, inner_idx, id_idx) = type_as_fixture();
    app.splash = false;

    app.cursor = inner_idx;
    app.cursor_line_in_node = app.tree[inner_idx].lines_total - 1;
    app.move_up();
    assert_eq!(
        app.cursor, id_idx,
        "Up from inner's footer must reach id's header"
    );
    assert!(!app.cursor_on_footer());

    let outer_idx = app.parent(inner_idx).unwrap();
    app.cursor = outer_idx;
    app.cursor_line_in_node = app.tree[outer_idx].lines_total - 1;
    app.move_up();
    assert_eq!(app.cursor, inner_idx);
    assert!(
        app.cursor_on_footer(),
        "Up from the root's footer must reach inner's own footer"
    );
}

/// Spec 0142 G3: `move_end` (`End`/`G`) reaches the document's true
/// last visible line — the root's own footer — not just the last
/// content node's header.
#[test]
fn move_end_reaches_the_document_true_last_line() {
    let (mut app, inner_idx, id_idx) = type_as_fixture();
    app.splash = false;
    let outer_idx = app.parent(inner_idx).unwrap();
    app.cursor = id_idx;
    app.cursor_line_in_node = 0;

    app.move_end();

    assert_eq!(app.cursor, outer_idx);
    assert!(app.cursor_on_footer());
}

/// `gg`/`Home` must reach the true first line even when the cursor
/// already rests on the root's own footer (`self.cursor ==
/// self.first_node` in that case too, since the closing `}` belongs
/// to the root node, not a distinct one) -- the plain `cursor !=
/// first_node` check alone would falsely treat this as already home.
#[test]
fn move_home_from_the_root_footer_reaches_the_document_true_first_line() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.splash = false;
    let outer_idx = app.parent(inner_idx).unwrap();
    app.cursor = outer_idx;
    app.cursor_line_in_node = app.tree[outer_idx].lines_total - 1;

    app.move_home();

    assert_eq!(app.cursor, app.first_node);
    assert!(!app.cursor_on_footer());
}

/// Spec 0142 G6.2: folding the node the cursor's footer currently
/// rests on must snap the cursor back to that node's own header,
/// since the footer line stops being visible.
#[test]
fn folding_the_node_under_a_footer_cursor_snaps_back_to_header() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.splash = false;
    app.cursor = inner_idx;
    app.cursor_line_in_node = app.tree[inner_idx].lines_total - 1;

    app.toggle_fold(inner_idx);

    assert_eq!(app.cursor, inner_idx);
    assert!(
        !app.cursor_on_footer(),
        "footer is hidden by the fold, cursor must fall back to header"
    );
}

/// Folding a node while the cursor rests on one of its descendants
/// (reachable via a fold-marker click on an ancestor, not just the
/// cursor's own node) must snap the cursor up to the folded node
/// itself — its row is the nearest still-visible ancestor once the
/// descendant's own row stops being drawn. Generalizes spec 0142 G6.2
/// beyond the footer-only case.
#[test]
fn folding_an_ancestor_of_the_cursor_snaps_the_cursor_up_to_it() {
    let (mut app, inner_idx, id_idx) = type_as_fixture();
    app.splash = false;
    app.cursor = id_idx;
    app.cursor_line_in_node = 0;

    app.toggle_fold(inner_idx);

    assert_eq!(
        app.cursor, inner_idx,
        "cursor must snap up to the folded ancestor, not stay stuck on a hidden descendant"
    );
    assert!(!app.cursor_on_footer());
}

/// Spec 0142: a mouse click directly on a node's own closing `}` line
/// moves the cursor there (the cursor lands on the node's last row) without toggling
/// the node's fold state (footer lines never carry a fold marker).
#[test]
fn clicking_a_closing_brace_line_moves_cursor_there_without_folding() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 10);
    app.cursor = 0;
    app.cursor_line_in_node = 0;

    // Line 3 ("  }") is inner's own footer line — row 3 within the pane.
    app.handle_click(2, 3);

    assert_eq!(app.cursor, inner_idx);
    assert!(app.cursor_on_footer());
    assert!(
        !app.folded.contains(&inner_idx),
        "clicking the }} line must not toggle the fold"
    );
}

/// Spec 0142 empty-message fix (2026-07-18 feedback): `Down`/`Up` must
/// be able to pass through an empty-but-bracketed message's own header
/// and footer lines, not get stuck.
#[test]
fn navigation_passes_through_an_empty_bracketed_message() {
    let (mut app, inner_idx) = empty_message_fixture();
    app.splash = false;
    let outer_idx = app.parent(inner_idx).unwrap();
    app.cursor = outer_idx;
    app.cursor_line_in_node = 0;

    app.move_down(); // outer header -> inner header
    assert_eq!(app.cursor, inner_idx);
    assert!(!app.cursor_on_footer());

    app.move_down(); // inner header -> inner's own footer
    assert_eq!(app.cursor, inner_idx);
    assert!(
        app.cursor_on_footer(),
        "empty message must still have a reachable footer stop"
    );

    app.move_down(); // inner footer -> outer footer
    assert_eq!(app.cursor, outer_idx);
    assert!(app.cursor_on_footer());

    app.move_up();
    assert_eq!(app.cursor, inner_idx);
    assert!(app.cursor_on_footer());
}

/// Spec 0142 empty-message fix: an empty-but-bracketed message is
/// still foldable — it has its own fold marker/handle (`has_children`)
/// and can be folded from the keyboard. The key was `Left` until spec
/// 0194 S6 handed the unshifted arrows to the caret, then `Space` until
/// `Space` became page-down and `z` became the one-key toggle.
#[test]
fn empty_bracketed_message_is_foldable() {
    let (mut app, inner_idx) = empty_message_fixture();
    app.splash = false;

    assert!(
        app.has_children(inner_idx),
        "an empty message is still a two-line bracketed node"
    );

    app.cursor = inner_idx;
    app.cursor_line_in_node = 0;
    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));

    assert!(
        app.folded.contains(&inner_idx),
        "z must fold an empty message"
    );
}

/// `Z` folds the cursor node and every descendant with it, and unfolds
/// the same set again — the whole subtree follows the state the cursor
/// node itself took, not each node's own opposite.
///
/// The fixture's root wrapper holds three `Item` submessages, so this
/// reaches two levels down; `z` on the wrapper would leave the `Item`s
/// alone.
#[test]
fn shift_z_folds_the_whole_subtree_and_unfolds_it_again() {
    let (mut app, items) = repeated_message_fixture();
    app.splash = false;
    let root = app.first_node;
    assert!(app.has_children(root), "fixture sanity check");

    app.set_cursor(root);
    app.handle_key(KeyEvent::new(KeyCode::Char('Z'), KeyModifiers::NONE));
    assert!(app.folded.contains(&root), "`Z` folds the cursor node");
    assert!(
        items.iter().all(|i| app.folded.contains(i)),
        "and every descendant with it"
    );

    app.handle_key(KeyEvent::new(KeyCode::Char('Z'), KeyModifiers::NONE));
    assert!(app.folded.is_empty(), "`Z` opens the whole subtree again");

    // A mixed subtree has no meaningful "opposite", so `Z` follows the
    // one node the user can see: with only a descendant folded, the
    // cursor node is open, and `Z` closes everything rather than
    // toggling each node separately.
    app.folded.insert(items[0]);
    app.refresh_line_counts(items[0]);
    app.handle_key(KeyEvent::new(KeyCode::Char('Z'), KeyModifiers::NONE));
    assert!(
        app.folded.contains(&root) && items.iter().all(|i| app.folded.contains(i)),
        "the cursor node's own state decides for the subtree"
    );
}

/// Spec 0249 S3: a node folded because its body was never rendered
/// draws and behaves exactly like one the user folded — same collapsed
/// row count, same closed marker, opened by the same keystroke.
///
/// The user cannot be asked to know which set a fold came from, so the
/// asymmetry is confined to the writers.
#[test]
fn an_auto_fold_reads_and_opens_like_a_user_fold() {
    let (mut app, items) = repeated_message_fixture();
    app.splash = false;
    let idx = items[0];

    app.auto_folded.insert(idx);
    app.refresh_line_counts(idx);

    assert!(app.is_folded(idx), "an auto-fold is a fold");
    assert_eq!(
        app.tree[idx].lines_visible, 1,
        "and shows one row whatever is beneath it"
    );

    app.set_cursor(idx);
    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));

    assert!(!app.is_folded(idx), "`z` opens it like any other fold");
    assert!(
        app.auto_folded.is_empty(),
        "and takes it out of the set it was actually in"
    );
    assert!(
        app.folded.is_empty(),
        "without leaving a user fold behind that nobody asked for"
    );
}

/// Spec 0249 S3: the two sets are independent, and a node can be in
/// both — the user folds a node that a bounded render already stopped
/// at. One unfold gesture must open it, so the unfold clears both;
/// clearing one set alone must leave the other's fold standing.
#[test]
fn a_user_fold_and_an_auto_fold_survive_each_other() {
    let (mut app, items) = repeated_message_fixture();
    app.splash = false;
    let idx = items[0];

    app.auto_folded.insert(idx);
    app.folded.insert(idx);
    app.refresh_line_counts(idx);

    // A bake landing clears only its own set. The user's fold stands.
    app.auto_folded.clear();
    assert!(
        app.is_folded(idx),
        "clearing the auto-folds must not pop open a fold the user made"
    );

    // And the other way round: the user's unfold is not undone by an
    // auto-fold entry left in place.
    app.auto_folded.insert(idx);
    app.set_cursor(idx);
    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
    assert!(
        !app.is_folded(idx),
        "one unfold gesture opens a node that is in both sets"
    );
}

/// Spec 0194 test-plan item 3 (S6, N5). One character per press, and the
/// left end does not wrap onto the row above — vim's default
/// `whichwrap`, which leaves `h` line-bound.
///
/// The left end reads as "no wrap" here only because
/// `sibling_leaves_app`'s nodes are parentless, childless leaves *and*
/// `reset_caret_column` leaves the anchor `Home`: under spec 0199 S5/S6
/// a press at a voluntary `Home` falls through to `parent_move`, which
/// on this fixture has nothing to fold and no parent. A fixture with a
/// real tree gets the tree assertions instead — see
/// `h_at_a_voluntary_home_folds_before_it_moves_to_the_parent`.
///
/// The right end is the asymmetric one: with nothing to descend into,
/// a further press carries on to the first character of the next row
/// rather than stopping dead, because a leaf's last character is the one
/// place the caret could be blocked while the document plainly continued
/// below.
#[test]
fn the_caret_moves_one_character_and_wraps_only_off_the_right_end() {
    let mut app = sibling_leaves_app(&["  ab", "cd"]);
    app.cursor = 0;
    app.reset_caret_column();
    assert_eq!(app.cursor_column, 2, "a fresh caret sits on the text");

    app.caret_left();
    assert_eq!(app.cursor_column, 2, "no wrap off the left end");
    assert_eq!(app.cursor, 0, "and the left end does not change the node");
    app.caret_right();
    assert_eq!(app.cursor_column, 3);
    assert_eq!(app.cursor, 0, "still inside the first row's text");
    app.caret_right();
    assert_eq!(app.cursor, 1, "past the last character, on to the next row");
    assert_eq!(app.cursor_column, 0, "and at that row's first character");
}

/// Spec 0194 test-plan item 4 (S3). The indentation is not reachable at
/// any width — including 0, where there is none, and 1, where spec 0193
/// cannot fit the fold marker into it and falls back to the reserved
/// field instead.
#[test]
fn the_caret_never_enters_the_indentation_at_any_indent_width() {
    for indent in [0usize, 1, 2, 4] {
        let line = format!("{}x: 1", " ".repeat(indent));
        let mut app = sibling_leaves_app(&[&line]);
        app.cursor = 0;
        app.reset_caret_column();
        assert_eq!(app.cursor_column, indent, "--indent {indent}");
        for _ in 0..10 {
            app.caret_left();
        }
        assert_eq!(
            app.cursor_column, indent,
            "--indent {indent}: `h` must stop at the first non-blank"
        );
        app.caret_to_line_start();
        assert_eq!(app.cursor_column, indent, "--indent {indent}: so must `0`");
    }
}

/// Spec 0194 test-plan item 5 (S1). The heat suffix is part of the
/// row's reachable range even though it is drawn outside the text, so
/// `$` lands on its last character rather than on the text's.
#[test]
fn the_heat_suffix_is_reachable_and_bounds_the_row() {
    let mut app = sibling_leaves_app(&["a: 1"]);
    app.cursor = 0;
    let suffix = " [2/7]";
    app.caret_suffix_len = suffix.chars().count();
    app.reset_caret_column();

    let text_len = "a: 1".chars().count();
    assert_eq!(
        app.caret_bounds(),
        (0, text_len - 1 + suffix.chars().count())
    );

    app.caret_to_line_end();
    assert_eq!(app.cursor_column, text_len - 1 + suffix.chars().count());
    app.caret_right();
    assert_eq!(
        app.cursor_column,
        text_len - 1 + suffix.chars().count(),
        "`l` stops on the suffix's last character"
    );

    // And it really is the suffix that extended the range: drop the cue
    // and the same `$` lands on the text's last character.
    app.caret_suffix_len = 0;
    app.caret_to_line_end();
    assert_eq!(app.cursor_column, text_len - 1);
}

/// Spec 0194 test-plan item 6 (S5). vim's desired-column rule: a
/// vertical move clamps the caret into the new row without forgetting
/// where it wanted to be, so a detour across a short row is undone on
/// the way back. A naive clamp loses the column permanently.
#[test]
fn a_detour_across_a_short_row_restores_the_desired_column() {
    let mut app = sibling_leaves_app(&["abcdef", "ab", "abcdef"]);
    app.cursor = 0;
    app.cursor_column = 4;
    app.desired_column = 4;
    // Mid-row, so neither end is anchored (spec 0199 S1) — without this
    // the fresh `App`'s first-frame `Home` would pin the caret instead.
    app.caret_anchor = CaretAnchor::Free;

    app.move_down();
    assert_eq!(app.cursor, 1);
    assert_eq!(app.cursor_column, 1, "clamped into the short row");

    app.move_down();
    assert_eq!(app.cursor, 2);
    assert_eq!(app.cursor_column, 4, "and restored on the far side");
}

/// Spec 0194 test-plan item 7 (S5). The one addition to vim's rule: a
/// caret on the first non-blank stays on the first non-blank, which is
/// what makes `j` usable down a message whose indentation changes on
/// nearly every row. A caret one column to its right does not stick —
/// otherwise the rule would swallow ordinary vertical movement.
///
/// Spec 0199 S2 is why the two cases differ by the *anchor* rather than
/// by the column: the caret can sit on a row's first non-blank without
/// having chosen to, and only the chosen one sticks.
#[test]
fn a_caret_on_the_first_non_blank_sticks_to_it_across_rows() {
    for (start, want, anchor) in [
        (4usize, 2usize, CaretAnchor::Home),
        (5, 5, CaretAnchor::Free),
    ] {
        let mut app = sibling_leaves_app(&["    abcd", "  wxyz"]);
        app.cursor = 0;
        app.cursor_column = start;
        app.desired_column = start;
        app.caret_anchor = anchor;
        app.move_down();
        assert_eq!(app.cursor, 1);
        assert_eq!(
            app.cursor_column, want,
            "from column {start} on a row indented 4, onto one indented 2"
        );
    }
}

/// Spec 0194 test-plan item 14 (S6). `%` crosses between the cursor
/// node's own braces, which means crossing between its header and
/// footer rows — so it owns `cursor_line_in_node` as well as the column.
#[test]
fn percent_moves_between_the_cursor_nodes_braces() {
    let (mut app, items) = repeated_message_fixture();
    app.set_cursor(items[1]);
    let (open, close) = app
        .cursor_brace_pair()
        .expect("a message node is bracketed");
    app.cursor_column = open.1;

    app.jump_matching_brace();
    assert!(
        app.cursor_on_footer(),
        "the closing brace is on the footer row"
    );
    assert_eq!((app.cursor_line(), app.cursor_column), close);

    app.jump_matching_brace();
    assert!(!app.cursor_on_footer());
    assert_eq!((app.cursor_line(), app.cursor_column), open);
}

/// Spec 0194 test-plan item 14, second half. A scalar has no braces to
/// cross, so `%` says so rather than moving the caret somewhere
/// arbitrary.
#[test]
fn percent_on_an_unbracketed_node_reports_rather_than_moving() {
    let mut app = sibling_leaves_app(&["a: 1"]);
    app.cursor = 0;
    app.reset_caret_column();

    app.jump_matching_brace();
    assert_eq!(app.cursor_column, 0, "the caret has not moved");
    assert!(
        app.message.contains("no matching brace"),
        "got {:?}",
        app.message
    );
}

/// 2026-07-25 bug: not every rendered line is a cursor stop, and
/// `move_down`/`move_up` used to test only the immediately adjacent row
/// and give up if it didn't resolve — making any such line an absorbing
/// barrier: reported as "I cannot `Down` past /3/1" on a node whose
/// mistyped preview rendered an `INVALID_TAG_TYPE` line.
///
/// Spec 0210 leaves exactly one kind of line with no node behind it: a
/// truncated preview's `...` marker (spec 0174 S4), which *replaces* the
/// straddling field's line and drops that field's span. A malformed
/// record now carries a `NodeSpan` of its own, and a virtual scalar
/// always did. Such a line can also only ever sit inside a node's body
/// now, because the line counters leave no room for a gap anywhere else.
#[test]
fn move_down_and_up_step_over_display_only_lines() {
    let (mut app, items) = repeated_message_fixture();
    app.splash = false;

    // `items[0]` is `items { v: 1 }` — three lines over one child. Drop
    // the child without touching the parent's counts, and its body line
    // is left belonging to nobody, exactly as a truncation marker's
    // does. Spec 0216: dropping a node is vacating its slot, there being
    // no chains left to unlink it from, and `first_child` reads the
    // vacancy straight back.
    let orphaned = app.first_child(items[0]).expect("items[0] has a child");
    app.tree_mut()[orphaned] = TreeNode::vacant();
    assert_eq!(app.tree[items[0]].lines_total, 3);

    let header = app.absolute_start(items[0]);
    assert!(
        app.line_pos(header + 1).is_none(),
        "the marker line must belong to no node, or the test is vacuous"
    );

    app.cursor = items[0];
    app.cursor_line_in_node = 0;
    app.move_down();
    assert!(
        app.cursor == items[0] && app.cursor_on_footer(),
        "move_down must skip the span-less line and land on the footer, \
         not stall on it — got {} footer={}",
        app.cursor,
        app.cursor_on_footer()
    );
    app.move_down();
    assert_eq!(app.cursor, items[1], "and then leave the node normally");

    app.move_up();
    assert!(app.cursor == items[0] && app.cursor_on_footer());
    app.move_up();
    assert!(
        app.cursor == items[0] && !app.cursor_on_footer(),
        "move_up must skip the span-less line, not stall on it"
    );
}

// ── Spec 0199: the arrow keys fold before they leave the node ──────────

/// Spec 0199 test-plan item 1 (G1). The distinction the whole anchor
/// exists for: a vertical move onto a shorter row leaves the caret at
/// that row's last column without the user ever having asked to be
/// there, so the anchor must stay `Free` and the next `l` must not
/// descend into the tree.
#[test]
fn a_vertical_move_onto_a_shorter_row_does_not_anchor_the_caret() {
    let mut app = sibling_leaves_app(&["abcdef", "ab"]);
    app.cursor = 0;
    app.cursor_column = 4;
    app.desired_column = 4;
    app.caret_anchor = CaretAnchor::Free;

    app.move_down();
    assert_eq!(app.cursor, 1);
    assert_eq!(app.cursor_column, 1, "clamped onto the short row's end");
    assert_eq!(
        app.caret_anchor,
        CaretAnchor::Free,
        "an end reached by clamping is not a chosen end"
    );
}

/// Spec 0199 test-plan items 2 and 3 (G7). Both ends are sticky across
/// a vertical move: `^` then `j` walks down the rows' first non-blanks
/// (spec 0194 S5's `'startofline'` rule, preserved through the rewrite)
/// and `$` then `j` walks down their last columns.
#[test]
fn both_caret_anchors_stick_across_a_vertical_move() {
    let mut app = sibling_leaves_app(&["    abcd", "  wxyz"]);
    app.cursor = 0;
    app.caret_to_line_start();
    assert_eq!(app.caret_anchor, CaretAnchor::Home);
    assert_eq!(app.cursor_column, 4);
    app.move_down();
    assert_eq!(app.cursor_column, 2, "`^` then `j` follows the indent");
    assert_eq!(app.caret_anchor, CaretAnchor::Home);

    let mut app = sibling_leaves_app(&["    abcd", "  wxyz"]);
    app.cursor = 0;
    app.caret_to_line_end();
    assert_eq!(app.caret_anchor, CaretAnchor::End);
    assert_eq!(app.cursor_column, 7);
    app.move_down();
    assert_eq!(app.cursor_column, 5, "`$` then `j` follows the row's end");
    assert_eq!(app.caret_anchor, CaretAnchor::End);
}

/// Spec 0199 test-plan item 4 (G8, S10). A click expresses *where*,
/// never *why* — so even a click that lands squarely on the row's first
/// non-blank leaves the anchor `Free`, and the `h` that follows it is
/// spent adopting the position rather than folding the node.
#[test]
fn a_click_never_anchors_the_caret() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.main_area = Rect::new(0, 0, 40, 20);
    app.set_cursor(inner_idx);
    assert_eq!(app.caret_anchor, CaretAnchor::Home);

    let line_idx = app.absolute_start(inner_idx);
    let first = app.caret_bounds().0;
    // Pane column of text column `first`: the heat-cue gutter (1) plus
    // the fold field, which `set_caret_from_click` subtracts back out.
    let column = (first + render::FOLD_FIELD_WIDTH + 1) as u16;
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row: line_idx as u16,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.cursor, inner_idx);
    assert_eq!(app.cursor_column, first, "the click landed on Home");
    assert_eq!(app.caret_anchor, CaretAnchor::Free);

    app.caret_left();
    assert!(
        !app.folded.contains(&inner_idx),
        "a click must not arm the fold"
    );
    assert_eq!(app.caret_anchor, CaretAnchor::Home);
}

/// Spec 0199 test-plan item 5. Away from either end `h` is the
/// unchanged caret motion of spec 0194 S6 — the behavior this spec must
/// not trade away to get the fold back.
#[test]
fn h_away_from_an_end_still_moves_the_caret() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.set_cursor(inner_idx);
    app.caret_to_line_end();
    let column = app.cursor_column;
    assert!(
        column > app.caret_bounds().0 + 1,
        "fixture must have a wide row"
    );

    app.caret_left();
    app.caret_left();
    assert_eq!(app.cursor_column, column - 2);
    assert!(!app.folded.contains(&inner_idx), "and folds nothing");
    assert_eq!(app.cursor, inner_idx);
}

/// Spec 0199 test-plan items 6, 7 and 8 (G2, G3). The three-press
/// sequence at the row's first column: an involuntary Home is *adopted*
/// (nothing folds, and `desired_column` collapses onto the real column,
/// which is vim's own rule for a horizontal motion), the next press
/// folds without moving the cursor, and only the third leaves the node.
#[test]
fn h_at_a_voluntary_home_folds_before_it_moves_to_the_parent() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.set_cursor(inner_idx);
    assert!(app.has_children(inner_idx), "inner must be foldable");
    // Arrive at the first column the way a vertical move would: pinned
    // there by a clamp, with a longer column still wanted.
    app.caret_anchor = CaretAnchor::Free;
    app.desired_column = app.cursor_column + 5;

    app.caret_left();
    assert!(
        !app.folded.contains(&inner_idx),
        "an involuntary Home must not fold"
    );
    assert_eq!(app.caret_anchor, CaretAnchor::Home);
    assert_eq!(app.desired_column, app.cursor_column);

    app.caret_left();
    assert!(app.folded.contains(&inner_idx), "the next press folds");
    assert_eq!(app.cursor, inner_idx, "and does not move the cursor");

    app.caret_left();
    assert_eq!(
        app.cursor,
        app.parent(inner_idx).unwrap(),
        "the third press moves to the parent"
    );
}

/// Spec 0199 test-plan item 9. A scalar node has no fold to close
/// first, so leftward costs one press rather than two — `has_children`
/// is what separates the two cases.
#[test]
fn h_on_a_scalar_moves_to_the_parent_immediately() {
    let (mut app, inner_idx, id_idx) = type_as_fixture();
    app.set_cursor(id_idx);
    assert!(!app.has_children(id_idx));
    assert_eq!(app.caret_anchor, CaretAnchor::Home);

    app.caret_left();
    assert_eq!(app.cursor, inner_idx);
    assert!(app.folded.is_empty(), "nothing was folded on the way");
}

/// Spec 0199 test-plan item 10 (N2). At the root there is no
/// sibling-wide fold to fall back on: `h` folds the root itself like
/// any other node, and the press after that reports rather than
/// widening its reach. Widening is `H`'s job now.
#[test]
fn h_on_the_root_folds_it_and_then_reports_no_parent() {
    let (mut app, inner_idx, _) = type_as_fixture();
    let root = app.parent(inner_idx).unwrap();
    assert!(app.parent(root).is_none());
    app.set_cursor(root);

    app.caret_left();
    assert!(app.folded.contains(&root), "the root folds like any node");
    app.message.clear();
    app.caret_left();
    assert_eq!(app.message, "no parent");
}

/// Spec 0199 test-plan items 11, 12 and 13 (G4, S6). `l` unfolds only
/// where it has nothing else to do *and* there is something to unfold —
/// both halves of the condition, since either alone would be wrong:
/// without the fold test it would never step into a row's text, and
/// without the anchor test a folded node's `Name { ... }` row would be
/// unreachable by caret.
#[test]
fn l_unfolds_only_at_a_voluntary_home_on_a_folded_row() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.set_cursor(inner_idx);
    app.toggle_fold(inner_idx);
    app.set_cursor(inner_idx);

    // Item 13: folded, but the caret is mid-row — plain caret motion.
    // Placed with `$`/`h` rather than with `l`, since `l` from the start
    // of the row is the very key under test and would have unfolded on
    // the way out.
    app.caret_to_line_end();
    app.caret_left();
    let column = app.cursor_column;
    assert!(column > app.caret_bounds().0);
    app.caret_right();
    assert_eq!(app.cursor_column, column + 1);
    assert!(
        app.folded.contains(&inner_idx),
        "no unfold away from a voluntary Home"
    );

    // Item 11: back at Home, it unfolds and stays put.
    app.caret_to_line_start();
    app.caret_right();
    assert!(!app.folded.contains(&inner_idx), "the press unfolds");
    assert_eq!(app.cursor, inner_idx, "and does not move the cursor");

    // Item 12: expanded now, so the same key at the same anchor is
    // caret motion again.
    let first = app.caret_bounds().0;
    app.caret_to_line_start();
    app.caret_right();
    assert_eq!(app.cursor_column, first + 1);
    assert_eq!(app.cursor, inner_idx);
}

/// Spec 0199 test-plan items 14 and 15 (G2, G4). The right edge is a
/// tree edge: an involuntary End is adopted first, and a voluntary End
/// walks through the node's opening brace into its first child. Landing
/// there anchors `Home` on the child's own name (Q2).
#[test]
fn l_at_a_voluntary_end_descends_into_the_first_child() {
    let (mut app, inner_idx, id_idx) = type_as_fixture();
    app.set_cursor(inner_idx);
    let (_, last) = app.caret_bounds();
    // Item 14: pinned onto the end by a clamp, not by choice.
    app.cursor_column = last;
    app.desired_column = last + 5;
    app.caret_anchor = CaretAnchor::Free;

    app.caret_right();
    assert_eq!(app.cursor, inner_idx, "an involuntary End must not descend");
    assert_eq!(app.caret_anchor, CaretAnchor::End);
    assert_eq!(app.desired_column, app.cursor_column);

    app.caret_right();
    assert_eq!(app.cursor, id_idx, "the next press descends");
    assert_eq!(app.caret_anchor, CaretAnchor::Home);
    assert_eq!(
        app.cursor_column,
        app.caret_bounds().0,
        "landing on the child's name, which is what was descended to read"
    );

    // Item 15: `$` declares the anchor, so from there it is one press.
    let (mut app, inner_idx, id_idx) = type_as_fixture();
    app.set_cursor(inner_idx);
    app.caret_to_line_end();
    app.caret_right();
    assert_eq!(app.cursor, id_idx);
}

/// Spec 0199 S6 descends through the *opening* brace, because that is
/// where the subtree lies on screen. A bracketed node owns a second row
/// — its closing brace — and `cursor` is deliberately the same node on
/// both (spec 0142), so the descent has to be told which row it is on.
///
/// Off the closing brace the subtree is drawn *above*, so rightward
/// motion carries on to the next line like any other end of text.
/// Without the guard the caret jumped backwards into the first child.
#[test]
fn l_off_a_closing_brace_goes_on_rather_than_back_into_the_subtree() {
    let (mut app, inner_idx, id_idx) = type_as_fixture();
    app.set_cursor(inner_idx);
    app.cursor_line_in_node = app.tree[inner_idx].lines_total - 1;
    // Arrived at the row rather than aimed at its end, which is what
    // makes it the two presses the report describes: one to adopt the
    // End, one to act on it.
    let (_, last) = app.caret_bounds();
    app.cursor_column = last;
    app.desired_column = last + 5;
    app.caret_anchor = CaretAnchor::Free;

    let footer = app.cursor_line();
    let pos = LinePos {
        node: app.cursor,
        line_in_node: app.cursor_line_in_node,
    };
    assert!(
        app.row_text(app.committed_row_at(footer, pos))
            .trim_end()
            .ends_with('}'),
        "the fixture must put the caret on the closing brace"
    );

    app.caret_right();
    assert_eq!(app.cursor_line(), footer, "the first press only anchors");
    assert_eq!(app.caret_anchor, CaretAnchor::End);

    app.caret_right();
    assert_ne!(app.cursor, id_idx, "the subtree is above, not ahead");
    assert_eq!(app.cursor_line(), footer + 1, "on to the next line");
    assert_eq!(
        app.cursor_column,
        app.caret_bounds().0,
        "and at that line's first character"
    );
}

/// Spec 0199 test-plan items 16 and 17 (S7). `l` at End on a *folded*
/// node unfolds instead of descending, and stops — which is what makes
/// `h` at Home its inverse: the round trip returns both the fold set
/// and the cursor to where they started.
#[test]
fn l_at_an_end_unfolds_before_it_descends_and_h_undoes_it() {
    let (mut app, inner_idx, id_idx) = type_as_fixture();
    app.set_cursor(inner_idx);
    app.toggle_fold(inner_idx);
    app.set_cursor(inner_idx);
    app.caret_to_line_end();

    app.caret_right();
    assert!(!app.folded.contains(&inner_idx), "the first press unfolds");
    assert_eq!(app.cursor, inner_idx, "and does not descend");
    app.caret_right();
    assert_eq!(app.cursor, id_idx, "the second press descends");

    // Item 17: the inverse property the split exists for.
    let (mut app, inner_idx, _) = type_as_fixture();
    app.set_cursor(inner_idx);
    app.toggle_fold(inner_idx);
    app.set_cursor(inner_idx);
    app.caret_to_line_end();
    app.caret_right();
    app.caret_to_line_start();
    app.caret_left();
    assert!(
        app.folded.contains(&inner_idx),
        "`h` at Home closed what `l` at End opened"
    );
    assert_eq!(app.cursor, inner_idx);
}

/// Spec 0199 test-plan items 18 and 19 (G5, N3/N4), as re-spelled by
/// spec 0242 S8: `Ctrl-Left`/`Ctrl-Right` — and their `Ctrl-h`/`Ctrl-l`
/// letter aliases — are the sibling-wide fold. Unconditional, at any
/// caret column and from any anchor, and they never move the cursor. No
/// column condition, because a modified arrow has no caret meaning to
/// defer to and such a condition would be invisible to the user.
#[test]
fn ctrl_left_and_ctrl_right_fold_the_whole_sibling_level() {
    let (mut app, items) = repeated_message_fixture();
    assert!(items.len() > 1, "fixture must have siblings");
    app.set_cursor(items[0]);
    app.caret_to_line_end();
    app.caret_left();
    app.caret_anchor = CaretAnchor::Free;

    app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL));
    for &item in &items {
        assert!(app.folded.contains(&item), "`Ctrl-h` folds every sibling");
    }
    assert_eq!(app.cursor, items[0], "and moves no cursor");

    app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL));
    for &item in &items {
        assert!(
            !app.folded.contains(&item),
            "`Ctrl-l` unfolds every sibling"
        );
    }
    assert_eq!(app.cursor, items[0]);
}

/// Spec 0199 test-plan item 20, carried over to spec 0242 S4's new
/// tenants. `Shift-h` *is* `H` — one gesture with two spellings, since
/// a terminal reports a capital letter as a character and a shifted
/// arrow as a key plus a modifier. Pinned so the two rows of the
/// binding table cannot drift apart again.
#[test]
fn the_shifted_arrows_and_the_capital_letters_are_one_binding() {
    let selected_after = |code: KeyCode, mods: KeyModifiers| {
        let (mut app, items) = repeated_message_fixture();
        app.set_cursor(items[0]);
        app.caret_to_line_end();
        app.handle_key(KeyEvent::new(code, mods));
        (app.selection_span(), app.cursor, app.cursor_column)
    };

    for (letter, arrow) in [
        ('H', KeyCode::Left),
        ('L', KeyCode::Right),
        ('J', KeyCode::Down),
        ('K', KeyCode::Up),
    ] {
        let by_letter = selected_after(KeyCode::Char(letter), KeyModifiers::SHIFT);
        assert!(
            by_letter.0.is_some(),
            "`{letter}` must select something to compare"
        );
        assert_eq!(
            by_letter,
            selected_after(arrow, KeyModifiers::SHIFT),
            "`{letter}` and its shifted arrow must be one binding"
        );
    }
}

/// Spec 0199 test-plan item 21 (G6), re-spelled by spec 0242 S9: spec
/// 0113 D24's horizontal pan is now `Shift-Alt-Up`/`Shift-Alt-Down`,
/// the Ctrl-arrows having gone to the sibling fold. This is the
/// regression the amended table is most likely to cause, since
/// `contains(SHIFT)` is true of a `Shift-Alt` chord too — the pan arm
/// has to be matched before the selection's.
#[test]
fn shift_alt_arrows_still_pan_without_touching_the_caret() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.set_cursor(inner_idx);
    app.pan_offset = 4;
    let column = app.cursor_column;

    app.handle_key(KeyEvent::new(
        KeyCode::Up,
        KeyModifiers::SHIFT | KeyModifiers::ALT,
    ));
    assert!(app.pan_offset < 4, "Shift-Alt-Up must pan left");
    assert_eq!(app.cursor_column, column, "and must not move the caret");
    assert_eq!(app.cursor, inner_idx);
    assert!(app.folded.is_empty(), "nor fold anything");
    assert_eq!(app.select_anchor, None, "nor start a selection");
}

/// Spec 0199 test-plan item 22 (G6, S8). `Alt`-arrows move by word,
/// over the very boundaries the `:` prompt uses — a main pane that
/// broke words differently from the command line on the same screen
/// would be a defect, not a feature.
#[test]
fn alt_arrows_move_the_caret_by_the_command_lines_own_words() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.set_cursor(inner_idx);
    let line_idx = app.absolute_start(inner_idx);
    let chars: Vec<char> = app.document_lines()[line_idx].chars().collect();
    let home = app.cursor_column;

    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
    let forward = command_line::next_word_boundary(&chars, home);
    assert!(forward > home, "the fixture row must contain a word");
    assert_eq!(app.cursor_column, forward);

    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
    assert_eq!(
        app.cursor_column,
        command_line::prev_word_boundary(&chars, forward)
    );
    assert_eq!(app.cursor_column, home, "and the pair round-trips");

    // `Alt-h`/`Alt-l` alias the arrows, as the letters do everywhere
    // else in the table.
    app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::ALT));
    assert_eq!(app.cursor_column, forward, "`Alt-l` is `Alt-Right`");
    app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::ALT));
    assert_eq!(app.cursor_column, home, "`Alt-h` is `Alt-Left`");
}

/// Spec 0208 test-plan items 1 and 2 (S1). `Ctrl-a`/`Ctrl-e` reach the
/// same two destinations as `^`/`$`, as every other text surface the
/// user touches binds them. Asserted against the vim spellings rather
/// than against literal columns, so the pair cannot drift apart.
#[test]
fn ctrl_a_and_ctrl_e_alias_the_line_end_motions() {
    let column_after = |code: KeyCode, mods: KeyModifiers| {
        let (mut app, inner_idx, _) = type_as_fixture();
        app.set_cursor(inner_idx);
        app.handle_key(KeyEvent::new(code, mods));
        app.cursor_column
    };

    let start = column_after(KeyCode::Char('^'), KeyModifiers::NONE);
    let end = column_after(KeyCode::Char('$'), KeyModifiers::NONE);
    assert_ne!(
        start, end,
        "the fixture row must be wide enough for the two to differ"
    );

    assert_eq!(
        column_after(KeyCode::Char('a'), KeyModifiers::CONTROL),
        start,
        "`Ctrl-a` is `^`"
    );
    assert_eq!(
        column_after(KeyCode::Char('e'), KeyModifiers::CONTROL),
        end,
        "`Ctrl-e` is `$`"
    );
}

/// Spec 0199 test-plan item 23, still holding after spec 0208 S1 gave
/// `Ctrl-a` a motion to perform. `a` toggles the annotation display;
/// `Ctrl-a`, which used to reach the same arm through an unguarded
/// pattern, does not.
#[test]
fn only_an_unmodified_a_toggles_the_annotation_display() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.set_cursor(inner_idx);
    let before = app.annotations;

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    assert_eq!(app.annotations, before, "Ctrl-a must not toggle");

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    assert_ne!(app.annotations, before, "plain `a` still does");
}

/// Spec 0199 test-plan item 24 (S9). `Backspace` is a plain left motion
/// in vim's normal mode; its old parent-move binding was a protolens
/// invention with no muscle memory behind it.
#[test]
fn backspace_moves_the_caret_left_rather_than_to_the_parent() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.set_cursor(inner_idx);
    app.caret_to_line_end();
    let column = app.cursor_column;

    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(app.cursor_column, column - 1);
    assert_eq!(app.cursor, inner_idx, "and stays on the node");
    assert!(app.folded.is_empty());
}

/// Spec 0215 test-plan item 1 (S1/S2). The hoist's whole obligation:
/// a page key must land exactly where a page of single-line moves
/// lands, in every one of the four values a move writes.
///
/// The claim it is really testing is S3's — that the caret fix-ups the
/// old loop performed at the 47 intermediate rows were unobservable.
/// That holds because `carry_caret` reads only `desired_column` and
/// `caret_anchor`, which stepping never touches, and writes only
/// `cursor_column`, which nothing between the steps reads. Run from
/// every start position and under all three anchors, since a `Free`
/// caret is the one whose column depends on the row it passes over and
/// so the one a per-row clamp could have left somewhere else.
#[test]
fn a_page_key_lands_where_a_page_of_single_keys_lands() {
    // Deliberately ragged: a `Free` caret crossing a short row is
    // clamped, and the old code clamped it once per row.
    let rows = [
        "    alpha: 1",
        "  b: 2",
        "        gamma_gamma: 33",
        "   d: 4",
        "  epsilon: 5",
        "f: 6",
        "      eta: 77",
        "  theta: 8",
    ];
    const PAGE: u16 = 3;

    let fresh = |start: usize, column: usize, anchor: CaretAnchor| {
        let mut app = sibling_leaves_app(&rows);
        app.main_area = Rect::new(0, 0, 40, PAGE);
        app.cursor = start;
        app.cursor_column = column;
        app.desired_column = column;
        app.caret_anchor = anchor;
        app
    };
    let state = |app: &App| {
        (
            app.cursor,
            app.cursor_on_footer(),
            app.cursor_column,
            app.cursor_moves,
        )
    };

    for start in 0..rows.len() {
        for (column, anchor) in [
            (0usize, CaretAnchor::Home),
            (6, CaretAnchor::Free),
            (0, CaretAnchor::End),
        ] {
            let mut paged = fresh(start, column, anchor);
            paged.move_page_down();
            let mut singles = fresh(start, column, anchor);
            for _ in 0..PAGE {
                singles.move_down();
            }
            assert_eq!(
                state(&paged),
                state(&singles),
                "PageDown from {start} at column {column} ({anchor:?})"
            );

            let mut paged = fresh(start, column, anchor);
            paged.move_page_up();
            let mut singles = fresh(start, column, anchor);
            for _ in 0..PAGE {
                singles.move_up();
            }
            assert_eq!(
                state(&paged),
                state(&singles),
                "PageUp from {start} at column {column} ({anchor:?})"
            );
        }
    }
}

/// Spec 0215 test-plan item 2 (S2). Guards the "only if a step
/// succeeded" clause, which is not decoration: `carry_caret` is not
/// idempotent on a caret that is currently somewhere else, so calling
/// it unconditionally would make a page key at a document end *move the
/// caret* — a keystroke that did nothing before now does something.
#[test]
fn a_page_key_at_a_document_end_is_a_no_op() {
    let rows = ["abcdef", "gh", "ijklmn"];
    for (cursor, key) in [(0usize, true), (rows.len() - 1, false)] {
        let mut app = sibling_leaves_app(&rows);
        app.main_area = Rect::new(0, 0, 40, 3);
        app.cursor = cursor;
        // A caret that `carry_caret` *would* relocate if it ran: the
        // desired column is off the end of the row, so a stray call
        // clamps it down to the row's last column.
        app.cursor_column = 1;
        app.desired_column = 99;
        app.caret_anchor = CaretAnchor::Free;
        let before = (
            app.cursor,
            app.cursor_on_footer(),
            app.cursor_column,
            app.cursor_moves,
        );

        if key {
            app.move_page_up();
        } else {
            app.move_page_down();
        }

        assert_eq!(
            (
                app.cursor,
                app.cursor_on_footer(),
                app.cursor_column,
                app.cursor_moves
            ),
            before,
            "a page key at a document end must change nothing at all"
        );
    }
}

/// Spec 0215 test-plan item 3 (S7). `caret_bounds` now hands
/// `row_text_of` the owner instead of letting it walk the document to
/// rediscover one. The substitution is only sound if the two agree, so
/// state that as an executable claim rather than as a comment — on both
/// kinds of cursor line, since the footer case is the one where the
/// answer is `None` rather than a node.
#[test]
fn the_caret_bounds_owner_matches_the_line_lookup() {
    let (mut app, items) = repeated_message_fixture();
    for footer in [false, true] {
        app.cursor = items[1];
        app.cursor_line_in_node = if footer {
            app.tree[items[1]].lines_total - 1
        } else {
            0
        };
        assert_eq!(
            (!app.cursor_on_footer()).then_some(app.cursor),
            app.node_at_header_line(app.cursor_line()),
            "owner disagreement on a {} line",
            if footer { "footer" } else { "header" }
        );
    }
}

/// Spec 0215 test-plan item 4 (S4-S6). Splitting the text half out of
/// `display_row_source` must not lose the fold expansion: a folded
/// node's row is drawn as a one-line `{ ... }` collapse summary, and
/// the caret can sit on it, so `caret_bounds` measures it. Losing it
/// would shorten the row the caret is clamped into.
#[test]
fn a_fold_marker_row_still_reports_its_expanded_text() {
    let (mut app, items) = repeated_message_fixture();
    let idx = items[0];
    let line = app.absolute_start(idx);
    assert!(
        !app.row_text(app.committed_row(line).unwrap())
            .contains("..."),
        "the fixture's node must start expanded"
    );

    app.folded.insert(idx);
    let text = app.row_text(app.committed_row(line).unwrap());
    assert!(
        text.contains("... }"),
        "a folded row must still expand to its collapse summary, got {text:?}"
    );
    // And the owner-carrying form agrees, since that is the one
    // `caret_bounds` calls.
    assert_eq!(
        text,
        app.row_text_of(app.committed_row(line).unwrap(), Some(idx))
    );
}

/// Spec 0194 test-plan item 9 (S8). Every node-level jump goes through
/// `set_cursor`, whose job is now to put the caret on the row's first
/// non-blank — landing on a new row with the caret still at the old
/// row's column would be arbitrary.
#[test]
fn a_node_level_jump_puts_the_caret_on_the_first_non_blank() {
    let (mut app, items) = repeated_message_fixture();
    app.set_cursor(items[0]);
    app.caret_to_line_end();
    assert!(app.cursor_column > 0, "the caret starts away from the edge");

    app.first_child_move();
    assert_ne!(app.cursor, items[0]);
    assert_eq!(app.cursor_column, app.caret_bounds().0);
    assert_eq!(app.desired_column, app.cursor_column);

    app.caret_to_line_end();
    app.parent_move();
    assert_eq!(app.cursor, items[0]);
    assert_eq!(app.cursor_column, app.caret_bounds().0);
}

/// Spec 0249 S8: opening a node a bounded render stopped at is a
/// render, not a set removal.
///
/// Without it, `z` would drop the node from `auto_folded` and draw an
/// empty pair of braces over a body that does exist — the row would
/// stop saying "not shown here" and start saying "nothing here".
///
/// `z` reaches the node at all because `has_children` is
/// `is_bracketed`, and a stop is bracketed: it emitted its header and
/// footer, just nothing between.
#[test]
fn opening_an_auto_fold_renders_the_body_it_stood_for() {
    let (mut app, items) = repeated_message_fixture();
    app.splash = false;

    let idx = items[0];

    // The same splice unbounded, as the reference. Taken through the
    // splice rather than from the fixture's own first render so that
    // both sides are the same interpretation of the same node.
    app.splice_override(app.first_node, Some("test.Outer".to_string()), None)
        .expect("an unbounded splice must succeed");
    let want_start = app.absolute_start(idx);
    let want_rows = app.tree[idx].lines_total as usize;
    let unbounded = app.document_lines();

    app.splice_override(app.first_node, Some("test.Outer".to_string()), Some(2))
        .expect("a bounded splice must succeed");
    assert!(app.auto_folded.contains(&idx), "the budget stopped here");
    assert_eq!(
        app.tree[idx].lines_total, 2,
        "and emitted a header and a footer and nothing between"
    );

    app.set_cursor(idx);
    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));

    assert!(
        !app.is_folded(idx),
        "`z` opens a stop like any other fold: {:?}",
        app.auto_folded
    );
    assert!(
        app.tree[idx].lines_total > 2,
        "and the body it stood for is there now"
    );
    assert!(app.folded.is_empty(), "with no user fold invented for it");

    // The rows it now occupies are the ones the unbounded render gives
    // at that node — the expansion is the same render, later.
    let start = app.absolute_start(idx);
    let opened = app.document_lines();
    assert_eq!(
        app.tree[idx].lines_total as usize, want_rows,
        "expanded height must match the unbounded render"
    );
    // Header included: spec 0253 gave the synthetic wrapper the node's
    // own cardinality, so the expanded header no longer drops the
    // `repeated` qualifier the unbounded render shows.
    assert_eq!(
        &opened[start..start + want_rows],
        &unbounded[want_start..want_start + want_rows],
        "expanded rows must match the unbounded render"
    );
}
