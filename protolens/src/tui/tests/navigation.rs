// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::super::*;
use super::support::*;

/// Spec 0126 G2: Shift-Down/Shift-Up alias `J`/`K`'s sibling-skip
/// move, no-op-with-message on a childless-of-siblings node either
/// way.
#[test]
fn shift_down_up_alias_sibling_skip_move() {
    let mut app = message_node_app();
    app.splash = false;

    let start = app.cursor;
    app.handle_key(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::NONE));
    let via_j = app.cursor;

    app.cursor = start;
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
    assert_eq!(app.cursor, via_j, "Shift-Down must match J's result");

    app.cursor = via_j;
    app.handle_key(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::NONE));
    let via_k = app.cursor;

    app.cursor = via_j;
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));
    assert_eq!(app.cursor, via_k, "Shift-Up must match K's result");
}

/// Spec 0114 §1.1: the virtual encompassing wrapper protobuf makes
/// "the node under the cursor" unambiguous even at the top level, and
/// display coordinates (byte ranges, positional paths) are corrected
/// back to exactly what they were pre-wrap. Mirrors the synthetic
/// `Outer { inner: Inner { id: 5 } }` fixture in
/// `extract::tests::extract_binary_message_round_trips_through_a_fresh_decode`.
#[test]
fn wrapper_offset_and_display_range_restore_pre_wrap_coordinates() {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext};

    let inner_desc = DescriptorProto {
        name: Some("Inner".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("id".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Int32 as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let outer_desc = DescriptorProto {
        name: Some("Outer".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("inner".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Message as i32),
            type_name: Some(".test.Inner".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_wrapper_offset.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![outer_desc, inner_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    let descriptor_path = std::env::temp_dir().join("protolens-tui-wrapper-offset-descriptor.pb");
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // Inner: field 1 varint 5 -> tag 0x08, value 0x05.
    let inner_bytes = [0x08u8, 0x05];
    // Outer wraps it as field 1 (LEN): tag (1<<3)|2 = 0x0A, len 2.
    let blob = [0x0Au8, 0x02, inner_bytes[0], inner_bytes[1]];

    let decoded = decode(&blob, &mut ctx, Some("test.Outer"), 2, false).unwrap();
    // tag(1 byte) + length-varint(1 byte, blob.len() == 4 fits in 1 byte).
    assert_eq!(decoded.wrapper_offset, 2);
    assert_eq!(decoded.blob.len(), blob.len() + 2);

    let app = App::new(
        decoded,
        "test.pb",
        PathBuf::from("test.pb"),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );

    // The level-0 node is the wrapper's sole field, standing in for
    // the entire original message (spec 0114 §1.1) — it did not exist
    // pre-wrap.
    let outer_idx = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some("test.Outer"))
        .expect("tree must contain the Outer stand-in node");
    // Its whole-message payload, offset-corrected, is exactly the
    // caller's original blob.
    assert_eq!(app.display_range(outer_idx), 0..blob.len());
    // The wrapper's own node displays as bare "/".
    assert_eq!(app.positional_path(outer_idx), "/");

    let inner_idx = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some("test.Inner"))
        .expect("tree must contain the Inner submessage");
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
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext};

    let msg_desc = DescriptorProto {
        name: Some("Msg".to_string()),
        field: vec![
            FieldDescriptorProto {
                name: Some("id".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("vals".to_string()),
                number: Some(2),
                label: Some(Label::Repeated as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_display_range_scalars.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![msg_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    let descriptor_path =
        std::env::temp_dir().join("protolens-tui-display-range-scalars-descriptor.pb");
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // id: field 1 varint 5 -> tag 0x08, value 0x05.
    // vals: field 2 (LEN, packed) tag (2<<3)|2 = 0x12, len 3, payload
    // [0x01, 0x02, 0x03] (three varint elements 1, 2, 3).
    let blob = [0x08u8, 0x05, 0x12, 0x03, 0x01, 0x02, 0x03];

    let decoded = decode(&blob, &mut ctx, Some("test.Msg"), 2, false).unwrap();
    let app = App::new(
        decoded,
        "test.pb",
        PathBuf::from("test.pb"),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );

    let id_idx = app
        .tree
        .iter()
        .position(|n| n.span.field_number == 1)
        .expect("tree must contain the id field");
    assert!(!app.tree[id_idx].span.is_message);
    // Tag (1 byte) stripped: just the varint value byte.
    assert_eq!(app.display_range(id_idx), 1..2);

    let vals_indices: Vec<usize> = app
        .tree
        .iter()
        .enumerate()
        .filter(|(_, n)| n.span.field_number == 2)
        .map(|(i, _)| i)
        .collect();
    // One NodeSpan per packed element (spec 0115), not one for the
    // whole record.
    assert_eq!(vals_indices.len(), 3);
    for idx in &vals_indices {
        assert!(!app.tree[*idx].span.is_message);
        assert!(app.tree[*idx].span.packed_record_start.is_some());
    }
    // Each element's own byte, already bare-payload — no further
    // tag/length stripping applied.
    assert_eq!(app.display_range(vals_indices[0]), 4..5);
    assert_eq!(app.display_range(vals_indices[1]), 5..6);
    assert_eq!(app.display_range(vals_indices[2]), 6..7);
}

/// Regression test: the always-reserved heat-cue gutter column (spec
/// 0138 N1) leaves only `main_area.width - 1` columns for line text, so
/// `pan_right`'s clamp must account for it or panning stops one
/// character short of the line's true end.
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
        line.len() - usable_width,
        "pan_offset must clamp so the last column of the pane shows the \
         line's last character, leaving room for the 1-column heat-cue \
         gutter"
    );
}

/// 2026-07-19 feedback item 1: Ctrl-Down/Ctrl-Up pan the main pane's
/// viewport without moving the cursor, bounded only by the content
/// itself (`0` at the top, `total - height` at the bottom) — no longer
/// tied to keeping the cursor's own row in view. 24 sibling leaf lines,
/// a 5-row pane.
#[test]
fn ctrl_up_down_pan_the_main_pane_without_moving_the_cursor() {
    let lines: Vec<String> = (0..24).map(|i| i.to_string()).collect();
    let texts: Vec<&str> = lines.iter().map(String::as_str).collect();
    let mut app = sibling_leaves_app(&texts);
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 5);
    app.cursor = 19;
    app.scroll_offset = 15;
    let max_scroll = 24 - 5;

    app.pan_vertical_down();
    assert_eq!(
        app.scroll_offset,
        (15 + PAN_STEP).min(max_scroll),
        "must scroll by PAN_STEP, clamped at the content's own bottom edge"
    );
    app.pan_vertical_down();
    assert_eq!(
        app.scroll_offset, max_scroll,
        "already at the content's own bottom edge, must not scroll past it"
    );
    assert_eq!(app.cursor, 19, "panning must not move the cursor");

    app.pan_vertical_up();
    assert_eq!(app.scroll_offset, max_scroll.saturating_sub(PAN_STEP));

    app.scroll_offset = 0;
    app.pan_vertical_up();
    assert_eq!(
        app.scroll_offset, 0,
        "already at the content's own top edge, must not scroll past it"
    );
    assert_eq!(app.cursor, 19, "panning must not move the cursor");
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
    app.cursor_footer = false;

    app.move_down(); // inner header -> id header
    app.move_down(); // id header -> inner footer

    assert_eq!(app.cursor, inner_idx);
    assert!(
        app.cursor_footer,
        "cursor must be resting on inner's own }} line"
    );
}

/// Spec 0142: `Down` from a node's own footer reaches the document's
/// true last visible line (the root's own footer), not nothing.
#[test]
fn down_from_footer_reaches_the_document_true_last_line() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.splash = false;
    let outer_idx = app.tree[inner_idx].parent.unwrap();
    app.cursor = inner_idx;
    app.cursor_footer = true;

    app.move_down();

    assert_eq!(app.cursor, outer_idx);
    assert!(
        app.cursor_footer,
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
    app.cursor_footer = true;
    app.move_up();
    assert_eq!(
        app.cursor, id_idx,
        "Up from inner's footer must reach id's header"
    );
    assert!(!app.cursor_footer);

    let outer_idx = app.tree[inner_idx].parent.unwrap();
    app.cursor = outer_idx;
    app.cursor_footer = true;
    app.move_up();
    assert_eq!(app.cursor, inner_idx);
    assert!(
        app.cursor_footer,
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
    let outer_idx = app.tree[inner_idx].parent.unwrap();
    app.cursor = id_idx;
    app.cursor_footer = false;

    app.move_end();

    assert_eq!(app.cursor, outer_idx);
    assert!(app.cursor_footer);
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
    let outer_idx = app.tree[inner_idx].parent.unwrap();
    app.cursor = outer_idx;
    app.cursor_footer = true;

    app.move_home();

    assert_eq!(app.cursor, app.first_node);
    assert!(!app.cursor_footer);
}

/// Spec 0142 G6.2: folding the node the cursor's footer currently
/// rests on must snap the cursor back to that node's own header,
/// since the footer line stops being visible.
#[test]
fn folding_the_node_under_a_footer_cursor_snaps_back_to_header() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.splash = false;
    app.cursor = inner_idx;
    app.cursor_footer = true;

    app.toggle_fold(inner_idx);

    assert_eq!(app.cursor, inner_idx);
    assert!(
        !app.cursor_footer,
        "footer is hidden by the fold, cursor must fall back to header"
    );
}

/// Folding a node while the cursor rests on one of its descendants
/// (reachable via a fold-marker click on an ancestor, not just the
/// cursor's own node) must snap the cursor up to the folded node
/// itself — its row is the nearest still-visible ancestor once the
/// descendant's own row disappears from `visible_rows`. Generalizes
/// spec 0142 G6.2 beyond the footer-only case.
#[test]
fn folding_an_ancestor_of_the_cursor_snaps_the_cursor_up_to_it() {
    let (mut app, inner_idx, id_idx) = type_as_fixture();
    app.splash = false;
    app.cursor = id_idx;
    app.cursor_footer = false;

    app.toggle_fold(inner_idx);

    assert_eq!(
        app.cursor, inner_idx,
        "cursor must snap up to the folded ancestor, not stay stuck on a hidden descendant"
    );
    assert!(!app.cursor_footer);
}

/// Spec 0142: a mouse click directly on a node's own closing `}` line
/// moves the cursor there (`cursor_footer = true`) without toggling
/// the node's fold state (footer lines never carry a fold marker).
#[test]
fn clicking_a_closing_brace_line_moves_cursor_there_without_folding() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.splash = false;
    app.main_area = Rect::new(0, 0, 40, 10);
    app.rebuild_visible_rows();
    app.cursor = 0;
    app.cursor_footer = false;

    // Line 3 ("  }") is inner's own footer line — row 3 within the pane.
    app.handle_click(2, 3);

    assert_eq!(app.cursor, inner_idx);
    assert!(app.cursor_footer);
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
    let outer_idx = app.tree[inner_idx].parent.unwrap();
    app.cursor = outer_idx;
    app.cursor_footer = false;

    app.move_down(); // outer header -> inner header
    assert_eq!(app.cursor, inner_idx);
    assert!(!app.cursor_footer);

    app.move_down(); // inner header -> inner's own footer
    assert_eq!(app.cursor, inner_idx);
    assert!(
        app.cursor_footer,
        "empty message must still have a reachable footer stop"
    );

    app.move_down(); // inner footer -> outer footer
    assert_eq!(app.cursor, outer_idx);
    assert!(app.cursor_footer);

    app.move_up();
    assert_eq!(app.cursor, inner_idx);
    assert!(app.cursor_footer);
}

/// Spec 0142 empty-message fix: an empty-but-bracketed message is
/// still foldable — it has its own fold marker/handle (`has_children`)
/// and can be folded via `Left`.
#[test]
fn empty_bracketed_message_is_foldable() {
    let (mut app, inner_idx) = empty_message_fixture();
    app.splash = false;

    assert!(
        app.has_children(inner_idx),
        "an empty message is still a two-line bracketed node"
    );

    app.cursor = inner_idx;
    app.cursor_footer = false;
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));

    assert!(
        app.folded.contains(&inner_idx),
        "Left must fold an empty message"
    );
}

/// 2026-07-25 bug: `visible_rows` holds every unfolded rendered line,
/// but only some of them are cursor stops — a malformed record's line
/// (`IndexSink::malformed` pushes no `NodeSpan`) and a virtual
/// scalar's line (Any's `type_url`, MessageSet's `type_id`) are
/// display-only. `move_down`/`move_up` used to test only the
/// immediately adjacent row and give up if it didn't resolve, making
/// any such line an absorbing barrier: reported as "I cannot `Down`
/// past /3/1" on a node whose mistyped preview rendered an
/// `INVALID_TAG_TYPE` line.
#[test]
fn move_down_and_up_step_over_display_only_lines() {
    let mut app = sibling_leaves_app(&["a", "b", "c"]);
    app.splash = false;
    app.rebuild_visible_rows();
    assert_eq!(app.visible_rows, vec![0, 1, 2]);

    // Line 1 is rendered but has no node behind it, exactly as a
    // malformed or virtual-scalar line does.
    app.line_to_node.remove(&1);

    app.cursor = 0;
    app.cursor_footer = false;
    app.move_down();
    assert_eq!(
        app.cursor, 2,
        "move_down must skip the span-less line, not stall on it"
    );

    app.move_up();
    assert_eq!(
        app.cursor, 0,
        "move_up must skip the span-less line, not stall on it"
    );
}
