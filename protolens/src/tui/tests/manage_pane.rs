// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::super::*;
use super::support::*;

/// Spec 0124 G1: Left/Right in the manage pane circulate the
/// main-pane cursor among the fields the highlighted entry's origin
/// matches, with wraparound, never touching focus; a zero-match
/// origin is a no-op.
#[test]
fn manage_pane_left_right_circulate_affected_fields() {
    let (mut app, items) = repeated_scalar_fixture();
    app.manage_focus = true;
    app.manage_open = true;

    // `PathField` origin: parent `/`, field `1` -> all 3 elements.
    let origin = OverrideOrigin::PathField {
        path: "/".to_string(),
        field: 1,
    };
    app.overrides.activate(origin, None);
    app.manage_highlight = app.overrides.entries().len() - 1;

    app.cursor = items[0];
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.cursor, items[1]);
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.cursor, items[2]);
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.cursor, items[0], "Right must wrap around");
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(app.cursor, items[2], "Left must wrap around");
    assert!(app.manage_focus, "focus must not change");

    // Zero-match origin: no-op.
    app.overrides.activate(
        OverrideOrigin::PathField {
            path: "/".to_string(),
            field: 99,
        },
        None,
    );
    app.manage_highlight = app.overrides.entries().len() - 1;
    let before = app.cursor;
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.cursor, before, "zero matches must be a no-op");
}

///// Item 10 (2026-07-17 feedback): clicking an entry that is already
/// the manage-pane's own highlighted entry (i.e. the "current"
/// override) — anywhere outside the radio marker column, which keeps
/// its own toggle-active click behavior — does the same as pressing
/// `Right`: circulate the main-pane cursor to the next node the
/// entry's origin matches.
#[test]
fn clicking_the_current_override_advances_to_the_next_impacted_node() {
    let (mut app, items) = repeated_scalar_fixture();
    app.manage_focus = true;
    app.manage_open = true;
    app.side_area = Rect::new(0, 0, 40, 20);
    app.manage_list_height = 10;
    app.manage_scroll = 0;
    app.manage_pan_offset = 0;

    let origin = OverrideOrigin::PathField {
        path: "/".to_string(),
        field: 1,
    };
    app.overrides.activate(origin, None);
    let idx = app.overrides.entries().len() - 1;
    app.manage_highlight = idx;
    app.cursor = items[0];

    let row = app
        .manage_display_rows()
        .iter()
        .position(|r| matches!(r, ManageRow::Entry(i) if *i == idx))
        .expect("entry must have a display row") as u16;

    // Column 10 is well clear of `MANAGE_MARKER_COL` (2). Each click
    // below resets `last_manage_row_click` first — these are meant to
    // simulate separate, deliberate single clicks (item 10's own
    // behavior), not a rapid double-click (item 11's, which opens the
    // selection pane instead — there's no real clock to wait out in a
    // synchronous unit test).
    app.handle_manage_click(10, row, false);
    assert_eq!(
        app.manage_highlight, idx,
        "click must not disturb the highlight"
    );
    assert_eq!(app.cursor, items[1]);

    app.last_manage_row_click = None;
    app.handle_manage_click(10, row, false);
    assert_eq!(app.cursor, items[2]);
    app.last_manage_row_click = None;
    app.handle_manage_click(10, row, false);
    assert_eq!(app.cursor, items[0], "must wrap around, same as Right");
}

/// A click on an entry that is *not* already highlighted only moves
/// the highlight (existing behavior) — it must not also advance the
/// main-pane cursor, since the entry just became current rather than
/// having been clicked while already current.
#[test]
fn clicking_a_different_override_only_moves_the_highlight() {
    let (mut app, items) = repeated_scalar_fixture();
    app.manage_focus = true;
    app.manage_open = true;
    app.side_area = Rect::new(0, 0, 40, 20);
    app.manage_list_height = 10;
    app.manage_scroll = 0;
    app.manage_pan_offset = 0;

    let origin = OverrideOrigin::PathField {
        path: "/".to_string(),
        field: 1,
    };
    app.overrides.activate(origin, None);
    let idx = app.overrides.entries().len() - 1;
    app.manage_highlight = 0;
    assert_ne!(app.manage_highlight, idx);
    app.cursor = items[0];

    let row = app
        .manage_display_rows()
        .iter()
        .position(|r| matches!(r, ManageRow::Entry(i) if *i == idx))
        .expect("entry must have a display row") as u16;

    app.handle_manage_click(10, row, false);
    assert_eq!(app.manage_highlight, idx);
    assert_eq!(app.cursor, items[0], "cursor must not move on first click");
}

/// Item 11 (2026-07-17 feedback): `Enter` on a highlighted management-
/// pane entry opens the override selection pane on it (not the old
/// close-the-pane behavior), hiding the management pane, rather than
/// closing it outright.
#[test]
fn enter_in_manage_pane_opens_the_selection_pane_on_the_highlighted_entry() {
    let (mut app, items) = repeated_scalar_fixture();
    app.manage_focus = true;
    app.manage_open = true;
    app.term_width = 120;

    let origin = OverrideOrigin::Path {
        path: app.positional_path(items[0]),
    };
    app.overrides
        .activate(origin.clone(), Some("sint32".to_string()));
    let idx = app.overrides.entries().len() - 1;
    app.manage_highlight = idx;

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.manage_open, "selection pane replaces the manage pane");
    assert!(app.override_focus);
    assert_eq!(app.override_target, Some(items[0]));
}

/// Item 11: cancelling (`Esc`) out of a selection pane opened this way
/// returns to the management pane — not the main pane — with the same
/// entry still highlighted, and without mutating it.
#[test]
fn esc_after_opening_from_manage_returns_to_manage_pane_without_mutating() {
    let (mut app, items) = repeated_scalar_fixture();
    app.manage_focus = true;
    app.manage_open = true;
    app.term_width = 120;

    let origin = OverrideOrigin::Path {
        path: app.positional_path(items[0]),
    };
    app.overrides
        .activate(origin.clone(), Some("sint32".to_string()));
    let idx = app.overrides.entries().len() - 1;
    app.manage_highlight = idx;
    let before = app.overrides.entries()[idx].clone();

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.override_focus);

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.manage_open, "must return to the management pane");
    assert!(app.manage_focus);
    assert!(!app.override_focus);
    assert_eq!(app.override_target, None);
    assert_eq!(app.manage_highlight, idx);
    assert_eq!(
        app.overrides.entries()[idx],
        before,
        "cancelling must not mutate the entry"
    );
}

/// Item 11: confirming (`Enter`) a new type from a selection pane
/// opened this way still lands back in the management pane (spec
/// 0119 G3's existing unconditional behavior), same as when the pane
/// is opened via `t`/item 3.
#[test]
fn enter_confirm_after_opening_from_manage_returns_to_manage_pane() {
    let (mut app, items) = repeated_scalar_fixture();
    app.manage_focus = true;
    app.manage_open = true;
    app.term_width = 120;

    let origin = OverrideOrigin::Path {
        path: app.positional_path(items[0]),
    };
    app.overrides
        .activate(origin.clone(), Some("sint32".to_string()));
    let idx = app.overrides.entries().len() - 1;
    app.manage_highlight = idx;

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.override_focus);

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.manage_open, "must land back in the management pane");
    assert!(app.manage_focus);
    assert!(!app.override_focus);
    assert_eq!(app.override_target, None);
}

/// Item 11: double-clicking an entry outside its marker column is
/// equivalent to `Enter` on it — opens the selection pane, hiding the
/// management pane.
#[test]
fn double_click_on_a_non_marker_cell_opens_the_selection_pane() {
    let (mut app, items) = repeated_scalar_fixture();
    app.manage_focus = true;
    app.manage_open = true;
    app.term_width = 120;
    app.side_area = Rect::new(0, 0, 40, 20);
    app.manage_list_height = 10;
    app.manage_scroll = 0;
    app.manage_pan_offset = 0;

    let origin = OverrideOrigin::Path {
        path: app.positional_path(items[0]),
    };
    app.overrides
        .activate(origin.clone(), Some("sint32".to_string()));
    let idx = app.overrides.entries().len() - 1;

    let row = app
        .manage_display_rows()
        .iter()
        .position(|r| matches!(r, ManageRow::Entry(i) if *i == idx))
        .expect("entry must have a display row") as u16;

    // Column 10 is well clear of `MANAGE_MARKER_COL` (2).
    app.handle_manage_click(10, row, false);
    app.handle_manage_click(10, row, false);
    assert!(!app.manage_open);
    assert!(app.override_focus);
    assert_eq!(app.override_target, Some(items[0]));
}

// Spec 0134 G2: a single derivable candidate is used even when the
/// main-pane cursor isn't on the node that produced it — rotating
/// onto a colliding origin deactivates the other entry (existing
/// `activate`-style invariant, reused unchanged).
#[test]
fn manage_pane_z_single_candidate_resolves_without_cursor_match() {
    let (mut app, items) = repeated_scalar_fixture();
    app.manage_focus = true;
    app.manage_open = true;

    let path_origin = OverrideOrigin::Path {
        path: app.positional_path(items[0]),
    };
    app.overrides.activate(path_origin.clone(), None);
    app.manage_highlight = app.overrides.entries().len() - 1;

    // Also seed a colliding PathField entry (parent `/`, field `1`),
    // active, so the rotation-collision path is exercised.
    let collide_origin = OverrideOrigin::PathField {
        path: "/".to_string(),
        field: 1,
    };
    app.overrides.activate(collide_origin.clone(), None);
    let collide_idx = app.overrides.entries().len() - 1;
    assert!(app.overrides.entries()[collide_idx].active);

    // `path_origin` only ever matches `items[0]` itself, so there is
    // exactly one candidate under `PathField` regardless of where
    // the cursor sits — put it elsewhere to confirm the single
    // candidate is used anyway.
    app.cursor = items[1];
    let entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| e.origin == path_origin)
        .expect("original Path entry must still exist");
    app.manage_highlight = entry_idx;
    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));

    // `handle_key` leaves `manage_highlight` on the rotated entry
    // (spec 0124 G2) — look it up by index, not by origin: two
    // entries now share `collide_origin` (the rotated one and the
    // pre-existing one it collided with), so origin alone is
    // ambiguous.
    let rotated = &app.overrides.entries()[app.manage_highlight];
    assert_eq!(rotated.origin, collide_origin);
    assert!(rotated.active, "rotated entry must stay active");
    assert!(!rotated.auto, "rotation always resets auto to false");
    let other = app
        .overrides
        .entries()
        .iter()
        .filter(|e| e.origin == collide_origin)
        .count();
    assert_eq!(other, 2, "duplicates now coexist under the same origin");
    assert_eq!(
        app.overrides
            .entries()
            .iter()
            .filter(|e| e.origin == collide_origin && e.active)
            .count(),
        1,
        "only one entry per origin stays active"
    );
    assert!(app.manage_pending_kind.is_none());
}

/// Spec 0134 G2/G3: 2+ distinct candidates with the cursor not on
/// any of them prompts instead of resolving; a same-key retry with
/// the cursor unchanged advances to the next kind in the barrel
/// instead of repeating the identical ambiguous outcome.
#[test]
fn manage_pane_z_ambiguous_candidates_advance_on_repeated_press() {
    let (mut app, items) = repeated_message_fixture();
    app.manage_focus = true;
    app.manage_open = true;

    let fqdn_origin = app
        .origin_for_kind(items[0], OverrideKind::FqdnField)
        .expect("field's parent type is known");
    app.overrides.activate(fqdn_origin.clone(), None);
    app.manage_highlight = app.overrides.entries().len() - 1;

    let outside = app
        .tree
        .iter()
        .position(|n| n.parent.is_none())
        .expect("root node must exist");
    app.cursor = outside;

    // FqdnField -> Path: all 3 Item submessages derive distinct Path
    // origins, and the cursor isn't on any of them -> ambiguous.
    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
    assert_eq!(app.message, "z: pick an override target (<-/->)");
    assert_eq!(
        app.overrides.entries()[app.manage_highlight].origin,
        fqdn_origin,
        "entry unchanged while ambiguous"
    );
    assert!(app.manage_pending_kind.is_some());

    // Same key, cursor still unchanged -> advances Path -> PathField.
    // All 3 elements share the same parent/field, so PathField
    // dedups to a single candidate and resolves immediately even
    // though the cursor still isn't on any of them.
    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
    let expected = OverrideOrigin::PathField {
        path: "/".to_string(),
        field: 1,
    };
    assert_eq!(
        app.overrides.entries()[app.manage_highlight].origin,
        expected
    );
    assert!(app.manage_pending_kind.is_none());
}

/// Spec 0134 G2/G3: moving the main-pane cursor between two `z`
/// attempts retries the same stuck kind (instead of advancing past
/// it) and resolves via the cursor-match branch.
#[test]
fn manage_pane_z_ambiguous_then_resolved_via_cursor_move() {
    let (mut app, items) = repeated_message_fixture();
    app.manage_focus = true;
    app.manage_open = true;

    let fqdn_origin = app
        .origin_for_kind(items[0], OverrideKind::FqdnField)
        .expect("field's parent type is known");
    app.overrides.activate(fqdn_origin.clone(), None);
    app.manage_highlight = app.overrides.entries().len() - 1;

    let outside = app
        .tree
        .iter()
        .position(|n| n.parent.is_none())
        .expect("root node must exist");
    app.set_cursor(outside);

    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
    assert_eq!(app.message, "z: pick an override target (<-/->)");

    // Move the cursor onto one of the affected nodes, then retry —
    // must retry the same `Path` attempt (not advance to
    // `PathField`) and resolve via the cursor match.
    app.set_cursor(items[1]);
    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
    let expected = OverrideOrigin::Path {
        path: app.positional_path(items[1]),
    };
    assert_eq!(
        app.overrides.entries()[app.manage_highlight].origin,
        expected
    );
    assert!(app.manage_pending_kind.is_none());
}

/// 2026-07-16 feedback: a real movement-tracking signal
/// (`cursor_moves`), not just comparing the cursor's numeric
/// position — a Down-then-Up round trip that lands back on the
/// exact same node still counts as movement, so a same-key `z`
/// retry afterward retries the same stuck kind instead of wrongly
/// treating it as "cursor unchanged" and advancing past it.
#[test]
fn manage_pane_z_down_then_up_round_trip_counts_as_movement() {
    let (mut app, items) = repeated_message_fixture();
    app.manage_focus = true;
    app.manage_open = true;

    let fqdn_origin = app
        .origin_for_kind(items[0], OverrideKind::FqdnField)
        .expect("field's parent type is known");
    app.overrides.activate(fqdn_origin.clone(), None);
    app.manage_highlight = app.overrides.entries().len() - 1;

    let outside = app
        .tree
        .iter()
        .position(|n| n.parent.is_none())
        .expect("root node must exist");
    app.set_cursor(outside);

    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
    assert_eq!(app.message, "z: pick an override target (<-/->)");
    assert!(app.manage_pending_kind.is_some());

    // Down then Up returns the cursor to the exact same node — but
    // it is still a real move, unlike a plain numeric-equality
    // check on `self.cursor` alone would conclude.
    app.move_down();
    app.move_up();
    assert_eq!(app.cursor, outside, "back at the same position");

    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
    assert_eq!(
        app.message, "z: pick an override target (<-/->)",
        "must retry the same kind, not advance"
    );
    assert_eq!(
        app.overrides.entries()[app.manage_highlight].origin,
        fqdn_origin,
        "entry unchanged - still ambiguous on retry"
    );
}

/// Spec 0134 G2: when no kind (other than the entry's own) applies
/// to any affected node — e.g. the wrapper root, which has no
/// parent so `PathField`/`FqdnField` both error — `z` writes "no
/// <kind> override target", leaves the entry unchanged, and clears
/// the pending state; repeating `z` reproduces the identical
/// outcome.
#[test]
fn manage_pane_z_no_target_aborts_when_no_kind_applies() {
    let (mut app, _items) = repeated_scalar_fixture();
    app.manage_focus = true;
    app.manage_open = true;

    let root_idx = app
        .tree
        .iter()
        .position(|n| n.parent.is_none())
        .expect("root node must exist");
    let root_origin = OverrideOrigin::Path {
        path: app.positional_path(root_idx),
    };
    app.overrides.activate(root_origin.clone(), None);
    app.manage_highlight = app.overrides.entries().len() - 1;
    app.cursor = root_idx;

    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
    assert_eq!(app.message, "z: no path-field override target");
    assert_eq!(
        app.overrides.entries()[app.manage_highlight].origin,
        root_origin
    );
    assert!(app.manage_pending_kind.is_none());

    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
    assert_eq!(app.message, "z: no path-field override target");
}

/// Rotating an *active* entry's origin kind runs `render_overrides`,
/// which can auto-seed a brand-new entry elsewhere in the tree (Any/
/// MessageSet auto-expansion) — re-sorting the whole collection out
/// from under the index `rotate_origin` already returned.
/// `manage_highlight` must still land on the just-rotated entry, not
/// whichever row the reshuffle happens to leave at that stale index
/// (feedback, 2026-07-16).
#[test]
fn manage_pane_z_rotation_survives_a_concurrent_auto_seed_reshuffle() {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    let any_msg = DescriptorProto {
        name: Some("Any".to_string()),
        field: vec![
            FieldDescriptorProto {
                name: Some("type_url".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::String as i32),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("value".to_string()),
                number: Some(2),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Bytes as i32),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let any_file = FileDescriptorProto {
        name: Some("google/protobuf/any.proto".to_string()),
        syntax: Some("proto3".to_string()),
        package: Some("google.protobuf".to_string()),
        message_type: vec![any_msg],
        ..Default::default()
    };
    let payload_msg = DescriptorProto {
        name: Some("Payload".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("label".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::String as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let container_msg = DescriptorProto {
        name: Some("Container".to_string()),
        field: vec![
            FieldDescriptorProto {
                name: Some("val".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("payload".to_string()),
                number: Some(2),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Message as i32),
                type_name: Some(".google.protobuf.Any".to_string()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let acme_file = FileDescriptorProto {
        name: Some("acme.proto".to_string()),
        syntax: Some("proto2".to_string()),
        package: Some("acme".to_string()),
        dependency: vec!["google/protobuf/any.proto".to_string()],
        message_type: vec![payload_msg, container_msg],
        ..Default::default()
    };
    let fds = FileDescriptorSet {
        file: vec![any_file, acme_file],
    };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path = std::env::temp_dir().join(format!(
        "protolens-tui-manage-z-reshuffle-descriptor-{n}.pb"
    ));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // Container {
    //   val: 42,
    //   payload: Any { type_url: "type.googleapis.com/acme.Payload",
    //                   value: Payload { label: "hi" } },
    // }
    let label = b"hi";
    let mut payload_bytes = vec![0x0au8, label.len() as u8];
    payload_bytes.extend_from_slice(label);
    let type_url = b"type.googleapis.com/acme.Payload";
    let mut any_bytes = vec![0x0au8, type_url.len() as u8];
    any_bytes.extend_from_slice(type_url);
    any_bytes.push(0x12);
    any_bytes.push(payload_bytes.len() as u8);
    any_bytes.extend_from_slice(&payload_bytes);
    let mut blob = vec![0x08u8, 0x2A]; // field 1, VARINT, value 42
    blob.push(0x12); // field 2, LEN
    blob.push(any_bytes.len() as u8);
    blob.extend_from_slice(&any_bytes);

    let decoded = decode(&blob, &mut ctx, RootType::Named("acme.Container"), 2).unwrap();
    let mut app = App::new(
        decoded,
        "test.pb",
        PathBuf::from("test.pb"),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;

    let val_idx = app
        .tree
        .iter()
        .position(|n| n.span.field_number == 1)
        .expect("must find the val node");

    // `App::new` already ran one `render_overrides` pass, which
    // auto-seeded the Any field's `value` — undo that seeding so it
    // starts out unexpanded again (no entry for it at all), letting
    // the `z`-triggered pass below re-seed it as a *fresh* entry and
    // reshuffle the collection out from under `manage_highlight`.
    let any_entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| e.auto)
        .expect("Any field must have been auto-seeded by App::new");
    app.overrides.remove(any_entry_idx);

    // Seed `val` as an explicit, active `Path` override — mirroring
    // what the override pane would do.
    let val_origin = override_pane::OverrideOrigin::Path {
        path: app.positional_path(val_idx),
    };
    app.overrides.activate(val_origin.clone(), None);
    app.manage_highlight = app
        .overrides
        .entries()
        .iter()
        .position(|e| e.origin == val_origin)
        .unwrap();
    app.manage_focus = true;
    app.manage_open = true;
    app.cursor = val_idx;
    assert_eq!(
        app.overrides.entries().len(),
        2,
        "root + val, Any field's auto-expansion not seeded yet: {:#?}",
        app.overrides.entries()
    );

    // Rotate: Path -> PathField. Since the rotated entry is active,
    // `handle_key` also runs `render_overrides`, which (for the
    // first time) walks into the still-unexpanded Any field and
    // auto-seeds a brand-new entry for it, reshuffling the whole
    // sorted collection.
    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));

    assert_eq!(
        app.overrides.entries().len(),
        3,
        "the Any field's auto-expansion must have been re-seeded by \
         the same render_overrides pass: {:#?}",
        app.overrides.entries()
    );
    let expected_origin = override_pane::OverrideOrigin::PathField {
        path: "/".to_string(),
        field: 1,
    };
    let highlighted = &app.overrides.entries()[app.manage_highlight];
    assert_eq!(
        highlighted.origin,
        expected_origin,
        "manage_highlight must still point at the just-rotated entry, \
         not the newly auto-seeded Any entry: {:#?}",
        app.overrides.entries()
    );
    assert!(highlighted.active, "rotated entry must stay active");
}

/// Spec 0124 G3: `D` duplicates the highlighted entry as a new,
/// always-inactive copy; the original and the copy coexist.
/// (Interactive feedback, 2026-07-17: swapped from `d`, which now
/// deletes — see `manage_pane_d_deletes_highlighted_entry`.)
#[test]
fn manage_pane_shift_d_duplicates_highlighted_entry_as_inactive() {
    let (mut app, items) = repeated_scalar_fixture();
    app.manage_focus = true;
    app.manage_open = true;

    let origin = OverrideOrigin::Path {
        path: app.positional_path(items[0]),
    };
    app.overrides.activate(origin.clone(), None);
    let orig_idx = app.overrides.entries().len() - 1;
    app.manage_highlight = orig_idx;

    let before_len = app.overrides.entries().len();
    app.handle_key(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::NONE));
    assert_eq!(app.overrides.entries().len(), before_len + 1);
    let new_idx = app.manage_highlight;
    assert!(!app.overrides.entries()[new_idx].active, "copy is inactive");
    assert_eq!(app.overrides.entries()[new_idx].origin, origin);

    // Activating the copy deactivates the original (existing
    // invariant, not new code).
    app.overrides.toggle_active(new_idx);
    let active_count = app
        .overrides
        .entries()
        .iter()
        .filter(|e| e.origin == origin && e.active)
        .count();
    assert_eq!(active_count, 1, "still at most one active entry per origin");
    assert!(app.overrides.entries()[new_idx].active);
}

/// Feedback, 2026-07-17: `D` on an `auto` entry produces a manual
/// (`auto == false`) copy — a duplicate is always a deliberate
/// manual entry, regardless of the original's auto/manual status.
#[test]
fn manage_pane_shift_d_duplicate_of_auto_entry_is_manual() {
    let mut app = message_set_fixture();
    app.manage_focus = true;
    app.manage_open = true;

    let item_idx = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some(decode::MESSAGE_SET_ITEM_FQDN))
        .expect("Item group must be spliced to the synthetic MessageSetItem type");
    let item_path = app.positional_path(item_idx);
    let item_entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| matches!(&e.origin, OverrideOrigin::Path { path } if *path == item_path))
        .expect("tier-1 entry must exist");
    assert!(app.overrides.entries()[item_entry_idx].auto);
    app.manage_highlight = item_entry_idx;

    app.handle_key(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::NONE));
    let new_idx = app.manage_highlight;
    assert_ne!(new_idx, item_entry_idx);
    assert!(
        !app.overrides.entries()[new_idx].auto,
        "duplicate of an auto entry must itself be manual"
    );
    assert!(
        app.overrides.entries()[item_entry_idx].auto,
        "original untouched"
    );
}

/// Interactive feedback, 2026-07-17: `d` now removes the highlighted
/// entry (swapped with `D`, see above) — same behavior as
/// `Delete`/`Backspace`, including the spec-0125 §G2 in-scope-`auto`
/// special case.
#[test]
fn manage_pane_d_deletes_highlighted_entry() {
    let (mut app, items) = repeated_scalar_fixture();
    app.manage_focus = true;
    app.manage_open = true;

    app.overrides.activate(
        OverrideOrigin::Path {
            path: app.positional_path(items[0]),
        },
        None,
    );
    let idx = app.overrides.entries().len() - 1;
    app.manage_highlight = idx;

    let before_len = app.overrides.entries().len();
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert_eq!(app.overrides.entries().len(), before_len - 1);
}

/// Interactive feedback, 2026-07-17: Shift-Down moves the highlight
/// like `Down`/`j`, and also activates the destination entry —
/// deactivating any other entry sharing its origin.
#[test]
fn manage_pane_shift_down_moves_and_activates_destination() {
    let (mut app, items) = repeated_message_fixture();
    app.manage_focus = true;
    app.manage_open = true;

    // Two distinct, inactive manual entries sharing no origin, so
    // activating one has no side effect on the other.
    app.overrides.activate(
        OverrideOrigin::Path {
            path: app.positional_path(items[0]),
        },
        None,
    );
    app.overrides
        .toggle_active(app.overrides.entries().len() - 1);
    let first_idx = app.overrides.entries().len() - 1;
    app.overrides.activate(
        OverrideOrigin::Path {
            path: app.positional_path(items[1]),
        },
        None,
    );
    let second_idx = app.overrides.entries().len() - 1;
    app.overrides.toggle_active(second_idx);
    assert!(!app.overrides.entries()[first_idx].active);
    assert!(!app.overrides.entries()[second_idx].active);

    let rows = app.manage_display_rows();
    let row_of = |idx: usize| {
        rows.iter()
            .position(|r| matches!(r, ManageRow::Entry(i) if *i == idx))
            .expect("row must exist")
    };
    let first_row = row_of(first_idx);
    let second_row = row_of(second_idx);
    assert!(second_row > first_row, "fixture ordering assumption");
    app.manage_highlight = first_idx;

    for _ in first_row..second_row {
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
    }

    assert_eq!(app.manage_highlight, second_idx);
    assert!(
        app.overrides.entries()[second_idx].active,
        "Shift-Down must activate the destination entry"
    );
    assert!(
        !app.overrides.entries()[first_idx].active,
        "Shift-Down must not activate any entry other than the destination"
    );
}

/// Item 11 (2026-07-17 feedback): `Enter` in the management pane no
/// longer closes it — it now opens the selection pane on the
/// highlighted entry (see the dedicated tests above). It still falls
/// back to the old close behavior when the collection is empty, since
/// there's nothing to open a selection pane on.
#[test]
fn manage_pane_enter_closes_pane_when_empty() {
    let mut app = message_node_app();
    app.splash = false;
    app.manage_focus = true;
    app.manage_open = true;
    while !app.overrides.entries().is_empty() {
        app.overrides.remove(0);
    }

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.manage_open);
    assert!(!app.manage_focus);
}

/// `q` closes the management pane (not just blurs focus, unlike
/// `Tab`).
#[test]
fn manage_pane_q_closes_pane() {
    let (mut app, _items) = repeated_scalar_fixture();
    app.manage_focus = true;
    app.manage_open = true;

    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(!app.manage_open);
    assert!(!app.manage_focus);
}

/// Spec 0130 §G1 (restyled 2026-07-18): manage-pane entry rows render
/// `auto == true` entries in `manage_entry_style(true, ..)`'s dedicated
/// color; `auto == false` entries render in the plain terminal default
/// (no explicit `fg`) — so only auto-derived entries stand out (no
/// `REVERSED` on either, since neither is highlighted here).
#[test]
fn manage_pane_entries_style_auto_vs_manual_distinctly() {
    let (mut app, items) = repeated_scalar_fixture();
    app.manage_focus = true;
    app.manage_open = true;

    app.overrides.activate(
        OverrideOrigin::Path {
            path: app.positional_path(items[0]),
        },
        None,
    );
    let manual_idx = app.overrides.entries().len() - 1;
    app.overrides.activate_auto(
        OverrideOrigin::Path {
            path: app.positional_path(items[1]),
        },
        Some("auto.Type".to_string()),
    );
    let auto_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| e.auto)
        .expect("auto entry must exist");
    assert_ne!(manual_idx, auto_idx);
    // Neither entry is highlighted, so `REVERSED` doesn't interfere
    // with the bold/plain assertion below.
    app.manage_highlight = app.overrides.entries().len();

    let area = Rect::new(0, 0, 120, 40);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).expect("terminal");
    terminal
        .draw(|frame| app.render_manage_pane(frame, area))
        .expect("render must not panic");
    let buffer = terminal.backend().buffer().clone();
    // Spec 0147 G1: no border — content is `area` minus the pane's own
    // `Length(1)` local statusline row.
    let inner = Rect::new(area.x, area.y, area.width, area.height - 1);

    let rows = app.manage_display_rows();
    let row_fg = |entry_idx: usize| {
        let row = rows
            .iter()
            .position(|r| matches!(r, ManageRow::Entry(i) if *i == entry_idx))
            .expect("row must exist");
        let y = inner.y + row as u16;
        (inner.x..inner.x + inner.width)
            .map(|x| &buffer[(x, y)])
            .find(|c| !c.symbol().trim().is_empty())
            .expect("row must render some text")
            .fg
    };
    assert_eq!(
        Some(row_fg(auto_idx)),
        theme::manage_entry_style(true, app.theme).fg,
        "auto entry row must use the auto color"
    );
    assert_eq!(
        theme::manage_entry_style(false, app.theme).fg,
        None,
        "manual entries must use the plain terminal default (no explicit fg)"
    );
    assert_ne!(
        row_fg(auto_idx),
        row_fg(manual_idx),
        "auto and manual entries must be visually distinct"
    );
}

/// Spec 0125 §G2: `Delete` on an `auto` entry still "in scope"
/// deactivates it instead of removing it, and shows the explanatory
/// message; `Delete` on an `auto` entry that has gone out of scope
/// (its ancestor's own override changed) actually removes it, same
/// as a manual entry.
#[test]
fn manage_pane_delete_deactivates_in_scope_auto_but_removes_out_of_scope_auto() {
    let mut app = message_set_fixture();
    app.manage_focus = true;
    app.manage_open = true;

    let item_idx = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some(decode::MESSAGE_SET_ITEM_FQDN))
        .expect("Item group must be spliced to the synthetic MessageSetItem type");
    let item_path = app.positional_path(item_idx);
    let item_entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| matches!(&e.origin, OverrideOrigin::Path { path } if *path == item_path))
        .expect("tier-1 entry must exist");
    assert!(app.overrides.entries()[item_entry_idx].auto);
    assert!(app.overrides.entries()[item_entry_idx].active);

    let message_idx = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some("ms_test.ExtPayload"))
        .expect("message field must resolve to ExtPayload");
    let message_path = app.positional_path(message_idx);
    let message_entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| matches!(&e.origin, OverrideOrigin::Path { path } if *path == message_path))
        .expect("tier-2 entry must exist");
    assert!(app.overrides.entries()[message_entry_idx].auto);

    // In-scope: `Delete` on the tier-1 entry deactivates, does not
    // remove.
    let before_len = app.overrides.entries().len();
    app.manage_highlight = item_entry_idx;
    app.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
    assert_eq!(
        app.overrides.entries().len(),
        before_len,
        "in-scope auto entry must not be removed"
    );
    assert!(
        !app.overrides.entries()[item_entry_idx].active,
        "in-scope auto entry must be deactivated"
    );
    assert_eq!(
        app.message,
        "auto-derived override deactivated (still in scope — delete would \
         just recreate it; use 'a' or wait for it to go out of scope)"
    );

    // Deactivating tier-1 makes tier-2 no longer "in scope" (spec
    // 0120's demotion): `Delete` on the tier-2 entry now actually
    // removes it, same as a manual entry.
    assert!(!app.auto_entry_in_scope(&app.overrides.entries()[message_entry_idx].clone()));
    let before_len = app.overrides.entries().len();
    app.manage_highlight = message_entry_idx;
    app.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
    assert_eq!(
        app.overrides.entries().len(),
        before_len - 1,
        "out-of-scope auto entry must be removed like a manual entry"
    );
}

/// Spec 0125 §G2: `Delete` on a manual (`auto == false`) entry is
/// unchanged — removes it outright, no special message.
#[test]
fn manage_pane_delete_removes_manual_entry_unchanged() {
    let (mut app, items) = repeated_scalar_fixture();
    app.manage_focus = true;
    app.manage_open = true;

    app.overrides.activate(
        OverrideOrigin::Path {
            path: app.positional_path(items[0]),
        },
        None,
    );
    let idx = app.overrides.entries().len() - 1;
    assert!(!app.overrides.entries()[idx].auto);
    app.manage_highlight = idx;

    let before_len = app.overrides.entries().len();
    app.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
    assert_eq!(app.overrides.entries().len(), before_len - 1);
    assert!(
        !app.message.contains("auto-derived"),
        "manual delete must not show the auto-entry message: {}",
        app.message
    );
}

/// Spec 0117 §3, extended: `/`/`?` in the management pane share the
/// same `command_buffer` as main-pane/override-pane search
/// (spec-0133-adjacent rework), but `Enter` dispatches to
/// `jump_to_manage_match`, moving `manage_highlight` rather than the
/// main-pane cursor or the override pane's own highlight.
#[test]
fn manage_pane_search_forward_and_backward() {
    let (mut app, items) = repeated_scalar_fixture();
    app.manage_focus = true;
    app.manage_open = true;

    for (item, ty) in items.iter().zip(["pkg.Alpha", "pkg.Beta", "pkg.Gamma"]) {
        let origin = OverrideOrigin::Path {
            path: app.positional_path(*item),
        };
        app.overrides.activate(origin, Some(ty.to_string()));
    }
    app.manage_highlight = 0;

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    assert!(app.command_buffer.is_some());
    for c in "gamma".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.command_buffer.is_none());
    assert_eq!(
        app.overrides.entries()[app.manage_highlight]
            .r#type
            .as_deref(),
        Some("pkg.Gamma")
    );

    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    for c in "alpha".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.overrides.entries()[app.manage_highlight]
            .r#type
            .as_deref(),
        Some("pkg.Alpha")
    );
}

/// Spec 0195 G2 in the management pane — the third of the three
/// searches that were all case-blind in the same way, because they were
/// three copies of the same two lines.
#[test]
fn manage_pane_search_is_smartcase() {
    let (mut app, items) = repeated_scalar_fixture();
    app.manage_focus = true;
    app.manage_open = true;

    for (item, ty) in items.iter().zip(["pkg.alpha", "pkg.beta", "pkg.Beta"]) {
        let origin = OverrideOrigin::Path {
            path: app.positional_path(*item),
        };
        app.overrides.activate(origin, Some(ty.to_string()));
    }

    // `entries()` is sorted, not in activation order, so locate the two
    // by type rather than assuming where they landed.
    let index_of = |app: &App, ty: &str| {
        app.overrides
            .entries()
            .iter()
            .position(|e| e.r#type.as_deref() == Some(ty))
            .expect("the fixture activated this type")
    };
    let lower = index_of(&app, "pkg.beta");
    let upper = index_of(&app, "pkg.Beta");

    // A lowercase pattern folds, so searching on from the capitalized
    // entry reaches the lowercase one.
    app.manage_highlight = upper;
    app.jump_to_manage_match(SearchDir::Forward, "beta");
    assert_eq!(app.manage_highlight, lower);

    // A capitalized pattern does not, so searching on from the lowercase
    // entry walks past everything else back to the capitalized one.
    app.manage_highlight = lower;
    app.jump_to_manage_match(SearchDir::Forward, "Beta");
    assert_eq!(app.manage_highlight, upper);
}

/// `N` repeats the last management-pane search in the opposite
/// direction, as it does in vim.
#[test]
fn manage_pane_search_repeat_with_capital_n_reverses_direction() {
    let (mut app, items) = repeated_scalar_fixture();
    app.manage_focus = true;
    app.manage_open = true;

    for (item, ty) in items.iter().zip(["pkg.Alpha", "pkg.Beta", "pkg.Gamma"]) {
        let origin = OverrideOrigin::Path {
            path: app.positional_path(*item),
        };
        app.overrides.activate(origin, Some(ty.to_string()));
    }
    app.manage_highlight = 0;

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for c in "pkg.".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.overrides.entries()[app.manage_highlight]
            .r#type
            .as_deref(),
        Some("pkg.Alpha")
    );

    // `N` repeats backward (opposite of the forward `/` that landed on
    // Alpha), wrapping past the root entry to the last `pkg.`-matching
    // entry.
    app.handle_key(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::NONE));
    assert_eq!(
        app.overrides.entries()[app.manage_highlight]
            .r#type
            .as_deref(),
        Some("pkg.Gamma")
    );

    // A second `N` continues backward, landing on the entry before it.
    app.handle_key(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::NONE));
    assert_eq!(
        app.overrides.entries()[app.manage_highlight]
            .r#type
            .as_deref(),
        Some("pkg.Beta")
    );
}

/// `gg`/`G` (vim-style jump-to-first/jump-to-last, mirroring the main
/// pane's own chord) work the management pane's own highlight, same
/// as `Home`/`End` do — a lone `g` press must not itself jump (it
/// only arms the chord).
#[test]
fn manage_pane_gg_and_capital_g_jump_to_first_and_last() {
    let (mut app, items) = repeated_scalar_fixture();
    app.manage_focus = true;
    app.manage_open = true;

    for (item, ty) in items.iter().zip(["pkg.Alpha", "pkg.Beta", "pkg.Gamma"]) {
        let origin = OverrideOrigin::Path {
            path: app.positional_path(*item),
        };
        app.overrides.activate(origin, Some(ty.to_string()));
    }
    app.manage_highlight = 1;

    app.handle_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE));
    assert_eq!(app.manage_highlight, app.overrides.entries().len() - 1);

    app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    assert_eq!(
        app.manage_highlight,
        app.overrides.entries().len() - 1,
        "a lone `g` must not jump by itself"
    );
    app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    assert_eq!(app.manage_highlight, 0);
}

/// Spec 0119 §G4: `f` in the management pane opens a rename buffer
/// pre-filled from the highlighted entry's current name; `Enter`
/// confirms, mutating the entry in place and — since the entry is
/// active — triggering a re-render whose header line picks up the
/// new name (the `(type, field_name)` re-splice gate).
#[test]
fn manage_pane_rename_updates_entry_and_rerenders_active_override() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.cursor = inner_idx;
    app.run_command("type-as test.Inner");
    assert_eq!(
        app.tree[inner_idx].span.type_fqdn.as_deref(),
        Some("test.Inner")
    );

    app.toggle_manage_pane();
    assert!(app.manage_open);
    let entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| e.active && e.r#type.as_deref() == Some("test.Inner"))
        .expect("type-as must have created an active entry for test.Inner");
    app.manage_highlight = entry_idx;

    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
    assert!(app.manage_rename.is_some());
    for c in "custom_name".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.manage_rename.is_none());
    assert_eq!(
        app.overrides.entries()[entry_idx].name.as_deref(),
        Some("custom_name")
    );

    let line_idx = app.absolute_start(inner_idx);
    let header = &app.lines[line_idx];
    assert!(
        header.contains("custom_name"),
        "expected the renamed field name in the re-rendered header: {header}"
    );
}

/// The `f` rename feature (spec 0119 §G4) applies to the document root
/// exactly like any other node: `type-as`-ing the root creates an
/// active `Path { path: "/" }` entry (`override_origin_for_kind`'s
/// plain-`path` default, spec 0208 S2), which the manage pane's `f`
/// handler renames and
/// re-renders in place, same as a non-root node.
#[test]
fn manage_pane_rename_works_on_the_document_root_with_a_real_type() {
    let (mut app, _inner_idx, _) = type_as_fixture();
    app.cursor = app.first_node;
    app.run_command("type-as test.Outer");

    app.toggle_manage_pane();
    assert!(app.manage_open, "manage pane must open");
    let entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| e.active && e.r#type.as_deref() == Some("test.Outer"))
        .expect("type-as on root must have created an active entry");
    app.manage_highlight = entry_idx;

    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
    assert!(app.manage_rename.is_some(), "f must open rename buffer");
    for c in "root_name".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.manage_rename.is_none());
    assert_eq!(
        app.overrides.entries()[entry_idx].name.as_deref(),
        Some("root_name")
    );
    assert_eq!(app.field_name_for(app.first_node), "root_name");
    let header = &app.lines[0];
    assert!(
        header.contains("root_name"),
        "expected the renamed field name in the root's header line: {header}"
    );
}

/// Same as above, but the root is explicitly `type-as-raw` (an active
/// entry with `r#type: None`) rather than a real type. Renaming such a
/// node must still show up in the header line: `splice_override`'s raw
/// path has no synthetic-field placeholder to patch (no schema at all),
/// so it patches the node's own numeric field-number label instead,
/// whenever an active override entry gives it a rename.
#[test]
fn manage_pane_rename_works_on_a_raw_typed_root() {
    let (mut app, _inner_idx, _) = type_as_fixture();
    app.cursor = app.first_node;
    app.run_command("type-as-raw");

    app.toggle_manage_pane();
    assert!(app.manage_open, "manage pane must open");
    let entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| e.active && e.r#type.is_none())
        .expect("type-as-raw on root must have created an active raw entry");
    app.manage_highlight = entry_idx;

    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
    assert!(app.manage_rename.is_some(), "f must open rename buffer");
    for c in "exemplar".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.manage_rename.is_none());
    assert_eq!(app.field_name_for(app.first_node), "exemplar");
    let header = &app.lines[0];
    assert!(
        header.contains("exemplar"),
        "expected the renamed field name in the raw root's header line: {header}"
    );
}

/// Same fix as `manage_pane_rename_works_on_a_raw_typed_root`, but for
/// an ordinary non-root node — confirms the raw-header rename patch is
/// not root-specific.
#[test]
fn manage_pane_rename_works_on_a_raw_typed_non_root_node() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.cursor = inner_idx;
    app.run_command("type-as-raw");

    app.toggle_manage_pane();
    assert!(app.manage_open, "manage pane must open");
    let entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| e.active && e.r#type.is_none())
        .expect("type-as-raw must have created an active raw entry");
    app.manage_highlight = entry_idx;

    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
    assert!(app.manage_rename.is_some(), "f must open rename buffer");
    for c in "custom_raw_name".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.manage_rename.is_none());
    assert_eq!(app.field_name_for(inner_idx), "custom_raw_name");
    let line_idx = app.absolute_start(inner_idx);
    let header = &app.lines[line_idx];
    assert!(
        header.contains("custom_raw_name"),
        "expected the renamed field name in the raw node's header line: {header}"
    );
}

/// Regression test for the manage-pane toggle/reactivate bug reported
/// against spec 0120's auto-expansion seeding: deactivating a
/// MessageSet tier-1 (`MessageSetItem`) auto-derived override via
/// `toggle_active` (the manage pane's `a`/Space key) must actually
/// stick across a `render_overrides` pass — not be silently
/// resurrected, which is what happened when the seeding condition in
/// `render_overrides` only checked "no active override currently
/// resolves" rather than "no entry exists yet for this origin at
/// all". Also asserts that reactivating it re-splices the Item's
/// payload back to the expanded `ExtPayload` form.
#[test]
fn toggling_message_set_auto_override_off_and_on_sticks() {
    let mut app = message_set_fixture();

    let item_idx = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some(decode::MESSAGE_SET_ITEM_FQDN))
        .expect("Item group must be spliced to the synthetic MessageSetItem type");
    let item_path = app.positional_path(item_idx);
    let entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| matches!(&e.origin, OverrideOrigin::Path { path } if *path == item_path))
        .expect("tier-1 entry must exist");

    // Deactivate, then re-render: the entry must stay inactive (not
    // be re-seeded), and the Item node must revert to its natural
    // (un-overridden) type.
    app.overrides.toggle_active(entry_idx);
    app.render_overrides(app.first_node);
    assert!(
        !app.overrides.entries()[entry_idx].active,
        "deactivating the tier-1 override must stick across a render \
         pass, not self-heal back to active: {:#?}",
        app.overrides.entries()
    );
    assert_eq!(
        app.tree[item_idx].span.type_fqdn.as_deref(),
        None,
        "Item node must render raw/natural once its override is \
         deactivated: {:#?}",
        app.lines
    );

    // Reactivate: the same entry (same index — `toggle_active` never
    // resorts) must come back active, and the Item's payload must
    // re-expand to ExtPayload.
    app.overrides.toggle_active(entry_idx);
    app.render_overrides(app.first_node);
    assert!(
        app.overrides.entries()[entry_idx].active,
        "reactivating the tier-1 override must stick: {:#?}",
        app.overrides.entries()
    );
    assert_eq!(
        app.tree[item_idx].span.type_fqdn.as_deref(),
        Some(decode::MESSAGE_SET_ITEM_FQDN),
        "Item node must re-expand once its override is reactivated: \
         {:#?}",
        app.lines
    );
    assert!(
        app.tree
            .iter()
            .any(|n| n.span.type_fqdn.as_deref() == Some("ms_test.ExtPayload")),
        "tier-2 auto-expansion must also come back after reactivating \
         tier-1: {:#?}",
        app.lines
    );
}

/// Regression test (2026-07-17 design correction, following 2026-07-14
/// interactive feedback): deactivating a MessageSet's tier-1 (`Item`)
/// override must have NO effect on its tier-2 (`message`) override —
/// `auto`/`manual` is provenance only (how an entry was created, shown
/// via `manage_entry_style`), and must never influence whether an
/// *active* entry actually applies. Tier-2's own entry stays active and
/// keeps applying regardless of tier-1's state, the same as it would if
/// it had been created manually. Supersedes the old "demotion"
/// mechanism (spec 0120 follow-up), which cascaded a tier-1
/// deactivation into silently un-applying tier-2 even though tier-2's
/// own entry was never touched — confusing given overrides are
/// otherwise modeled as plain active/inactive flags.
#[test]
fn deactivating_tier_1_does_not_affect_the_still_active_tier_2_entry() {
    let mut app = message_set_fixture();

    let item_idx = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some(decode::MESSAGE_SET_ITEM_FQDN))
        .expect("Item group must be spliced to the synthetic MessageSetItem type");
    let item_path = app.positional_path(item_idx);
    let item_entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| matches!(&e.origin, OverrideOrigin::Path { path } if *path == item_path))
        .expect("tier-1 entry must exist");
    let message_idx = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some("ms_test.ExtPayload"))
        .expect("message field must resolve to ExtPayload");
    let message_path = app.positional_path(message_idx);
    let message_entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| matches!(&e.origin, OverrideOrigin::Path { path } if *path == message_path))
        .expect("tier-2 entry must exist");

    // Deactivate tier-1 only — tier-2's own entry is left untouched.
    app.overrides.toggle_active(item_entry_idx);
    app.render_overrides(app.first_node);

    assert!(
        app.overrides.entries()[message_entry_idx].active,
        "tier-2's own entry must remain active (untouched by \
         deactivating tier-1): {:#?}",
        app.overrides.entries()
    );
    assert!(
        app.lines.iter().any(|l| l.contains("ExtPayload")),
        "tier-2's active override must keep applying even though its \
         governing tier-1 ancestor is now deactivated — provenance \
         (auto vs manual) must have no effect on whether an active \
         override applies: {:?}",
        app.lines
    );

    // Reactivating tier-1 must not disturb tier-2 either.
    app.overrides.toggle_active(item_entry_idx);
    app.render_overrides(app.first_node);
    assert!(
        app.tree
            .iter()
            .any(|n| n.span.type_fqdn.as_deref() == Some("ms_test.ExtPayload")),
        "tier-2 must still resolve after reactivating tier-1: {:?}",
        app.lines
    );
}

/// Interactive feedback (2026-07-17): double-clicking an entry's radio
/// marker is the mouse-only alternative to Shift-click for
/// `toggle_active_cascading` (most terminal emulators intercept Shift-
/// click for native text selection before it ever reaches the app). By
/// the time the second click is recognized as a double, the first
/// click has already applied its own plain toggle
/// (`handle_manage_click`'s synchronous, timer-free double-click
/// detection) — the handler undoes it before applying the cascading
/// toggle, so the net effect matches a single Shift-click/`A` from the
/// state before the first click, not two independent plain toggles.
#[test]
fn double_click_on_marker_cascades_like_a_single_shift_click() {
    let (mut app, _items) = repeated_scalar_fixture();
    app.manage_open = true;
    app.manage_focus = true;
    app.side_area = Rect::new(0, 0, 40, 20);
    app.manage_list_height = 10;
    app.manage_scroll = 0;
    app.manage_pan_offset = 0;

    let origin = OverrideOrigin::PathField {
        path: "/".to_string(),
        field: 1,
    };
    app.overrides.activate(origin, None);
    let idx = app.overrides.entries().len() - 1;
    app.manage_highlight = idx;
    assert!(app.overrides.entries()[idx].active);

    // Look up the entry's own display row rather than assuming a fixed
    // one — the fixture already seeds an auto root entry ahead of it,
    // so it isn't necessarily the first `Entry` row.
    let row = app
        .manage_display_rows()
        .iter()
        .position(|r| matches!(r, ManageRow::Entry(i) if *i == idx))
        .expect("entry must have a display row") as u16;

    // Column 2 is `manage_pane`'s own `MANAGE_MARKER_COL`.
    app.handle_manage_click(2, row, false);
    assert!(
        !app.overrides.entries()[idx].active,
        "first click toggles off, same as a plain `a`/Space"
    );

    app.handle_manage_click(2, row, false);
    assert!(
        !app.overrides.entries()[idx].active,
        "double-click must net a single cascading toggle from the state \
         before the first click (originally active, so a single toggle \
         deactivates), not two plain toggles stacked on top of each \
         other (which would cancel out and leave it active)"
    );
}

/// Feedback (2026-07-17), tier 1: `o` places the manage-pane cursor
/// on the entry currently active for the main-pane cursor's own node,
/// when one exists.
#[test]
fn o_places_the_cursor_on_the_active_entry_for_the_cursor_node() {
    let (mut app, _, id_idx) = type_as_fixture();
    app.splash = false;
    app.term_width = 120;

    // An unrelated active entry, sorted ahead of the real one, so the
    // test can't pass by accident (e.g. always landing on index 0).
    app.overrides.activate(
        OverrideOrigin::Path {
            path: "/".to_string(),
        },
        None,
    );

    let origin = OverrideOrigin::Path {
        path: app.positional_path(id_idx),
    };
    app.overrides
        .activate(origin.clone(), Some("sint32".to_string()));
    let entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| e.origin == origin)
        .expect("entry must exist");

    app.cursor = id_idx;
    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    assert!(app.manage_open);
    assert_eq!(app.manage_highlight, entry_idx);
}

/// Feedback (2026-07-17), tier 2: with no active entry for the
/// cursor node, `o` instead picks the first (lexicographic display
/// order) entry that would apply to it if activated — not just any
/// entry, and not necessarily the collection's own first entry.
#[test]
fn o_places_the_cursor_on_the_first_inactive_entry_that_would_apply() {
    let (mut app, inner_idx, id_idx) = type_as_fixture();
    app.splash = false;
    app.term_width = 120;

    // Unrelated entry (targets `inner`, not `id`) — must be skipped.
    app.overrides.activate(
        OverrideOrigin::Path {
            path: app.positional_path(inner_idx),
        },
        None,
    );

    // Matching entry, left inactive (`activate` always leaves the
    // freshly-created entry active — deactivate it right back so this
    // exercises the "no active entry" tier-2 path, not tier 1).
    let origin = OverrideOrigin::Path {
        path: app.positional_path(id_idx),
    };
    app.overrides
        .activate(origin.clone(), Some("sint32".to_string()));
    let entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| e.origin == origin)
        .expect("entry must exist");
    app.overrides.toggle_active(entry_idx);
    assert!(!app.overrides.entries()[entry_idx].active);

    app.cursor = id_idx;
    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    assert!(app.manage_open);
    assert_eq!(app.manage_highlight, entry_idx);
}

/// Feedback (2026-07-17), tier 3: no entry (active or not) applies to
/// the cursor node at all — `o` falls back to the pane's own first
/// entry.
#[test]
fn o_falls_back_to_the_first_entry_when_none_applies_to_the_cursor() {
    let (mut app, inner_idx, id_idx) = type_as_fixture();
    app.splash = false;
    app.term_width = 120;

    app.overrides.activate(
        OverrideOrigin::Path {
            path: app.positional_path(inner_idx),
        },
        None,
    );

    app.cursor = id_idx;
    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    assert!(app.manage_open);
    assert_eq!(app.manage_highlight, 0);
}

/// Feedback (2026-07-17), subsidiary question: an empty override
/// collection is not a special case — `o` just opens onto an empty
/// list, `manage_highlight` at its harmless default of `0`. `App::new`
/// always seeds a root `Path` entry (spec 0117 §1), so a genuinely
/// empty collection can't arise from ordinary use, but the code path
/// is still reachable (e.g. deleting that seeded entry) and must not
/// panic or misbehave.
#[test]
fn o_on_an_empty_override_collection_opens_with_highlight_zero() {
    let (mut app, _, id_idx) = type_as_fixture();
    app.splash = false;
    app.term_width = 120;
    while !app.overrides.entries().is_empty() {
        app.overrides.remove(0);
    }
    assert!(app.overrides.entries().is_empty());

    app.cursor = id_idx;
    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    assert!(app.manage_open);
    assert_eq!(app.manage_highlight, 0);
}

/// Spec 0147 G5: a message set while the manage pane has focus is
/// cleared by the *next* keypress handled by `handle_manage_key`, not
/// just by a keypress that reaches main-pane handling.
#[test]
fn message_is_dismissed_by_the_next_key_in_the_manage_pane() {
    let (mut app, _items) = repeated_scalar_fixture();
    app.manage_focus = true;
    app.manage_open = true;

    app.message = "stale notice".to_string();
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert!(
        app.message.is_empty(),
        "the next manage-pane key must dismiss a stale message: {}",
        app.message
    );
}

/// `Esc` closes the manage pane when it's open but the main pane has
/// focus — consistent with the override-select pane's own `Esc`
/// behavior (`close_override` fires whenever `override_target` is
/// set, regardless of which pane currently has focus).
#[test]
fn esc_closes_the_manage_pane_from_main_pane_focus() {
    let (mut app, _items) = repeated_scalar_fixture();
    app.manage_open = true;
    app.manage_focus = false;

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert!(!app.manage_open);
    assert!(!app.manage_focus);
}
