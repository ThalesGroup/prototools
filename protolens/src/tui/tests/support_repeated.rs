// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Fixtures whose subject is repetition: repeated scalar and message
//! fields, and the packed runs that encode a repetition as a single
//! record.

use super::super::*;

/// `Outer { repeated int32 vals = 1; }`, packed, 3 elements (`5, 6,
/// 7`) — the smallest packed run, returned with the node that draws it.
///
/// Spec 0184 already had the 3 elements share one wire record and hence
/// one positional path `/1`; spec 0216 finishes the job by making them
/// one *node*, drawing three rows. So this fixture no longer offers
/// three of anything: a test that needs three siblings wants
/// `repeated_message_fixture`.
pub(super) fn repeated_scalar_fixture() -> (App, usize) {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    let outer_desc = DescriptorProto {
        name: Some("Outer".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("vals".to_string()),
            number: Some(1),
            label: Some(Label::Repeated as i32),
            r#type: Some(Type::Int32 as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_repeated_scalar.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![outer_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-repeated-scalar-descriptor-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // vals: field 1 (tag 0x0A, LEN/packed), length 3, payload
    // [0x05, 0x06, 0x07] (three one-byte varint elements).
    let blob = [0x0Au8, 0x03, 0x05, 0x06, 0x07];
    let decoded = decode(wrapped(&blob), &mut ctx, RootType::Named("test.Outer"), 2).unwrap();
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

    let run = app
        .tree
        .iter()
        .position(|n| n.span.packed_record_start != NO_PACKED_RECORD)
        .expect("fixture must contain the packed run");
    assert_eq!(
        app.tree[run].lines_total, 3,
        "the run draws one row per element"
    );
    (app, run)
}

/// `Outer { repeated Item items = 1; }` with 3 `Item { int32 v = 1; }`
/// submessages — three genuinely distinct sibling nodes at `/1`, `/2`,
/// `/3` that nonetheless share a parent type and field number.
///
/// This is the shape the manage pane's ambiguity machinery (spec 0134)
/// needs: an `FqdnField`/`PathField` origin matching several nodes that
/// derive *different* `Path` origins. `repeated_scalar_fixture` used to
/// serve that role, until spec 0184 made a packed run occupy a single
/// positional ordinal — an unpacked repeated *message* field is one
/// wire record per element and so keeps distinct paths by construction.
pub(super) fn repeated_message_fixture() -> (App, Vec<usize>) {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    let item_desc = DescriptorProto {
        name: Some("Item".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("v".to_string()),
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
            name: Some("items".to_string()),
            number: Some(1),
            label: Some(Label::Repeated as i32),
            r#type: Some(Type::Message as i32),
            type_name: Some(".test.Item".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_repeated_message.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![outer_desc, item_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-repeated-message-descriptor-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // items: field 1 (tag 0x0A, LEN), thrice, each wrapping
    // Item { v: 5 | 6 | 7 } (field 1, tag 0x08, one-byte varint).
    let blob = [
        0x0Au8, 0x02, 0x08, 0x05, //
        0x0A, 0x02, 0x08, 0x06, //
        0x0A, 0x02, 0x08, 0x07,
    ];
    let decoded = decode(wrapped(&blob), &mut ctx, RootType::Named("test.Outer"), 2).unwrap();
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

    // Spec 0216: the root is slot 0 — the wrapper — and the `Item`s are
    // its children, wherever the current interpretation puts them.
    let items: Vec<usize> = (0..app.child_count(app.first_node))
        .map(|k| {
            app.nth_child(app.first_node, k)
                .expect("k is below the child count")
        })
        .collect();
    assert_eq!(items.len(), 3, "fixture must contain 3 Item submessages");
    (app, items)
}

/// `Outer { repeated int32 vals = 1; Inner tail = 2; int32 a = 3;
/// int32 b = 4; }` — a packed run of 3 elements followed by three
/// ordinary siblings. Returns `(app, run, tail, a, b)`.
///
/// The shape spec 0184 is about: the run must occupy exactly one
/// positional ordinal (`/1`) so that `tail`/`a`/`b` sit at `/2`/`/3`/
/// `/4` whatever the run's element count and whatever its override
/// state, while `a` and `b` — two consecutive *non-packed* scalars —
/// must still get distinct ordinals of their own (the S1 trap).
///
/// Spec 0216 S22 turns that rule into arithmetic: the run is one node
/// drawing three rows, so `run` is a single index rather than three.
pub(super) fn packed_run_with_tail_fixture() -> (App, usize, usize, usize, usize) {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    let scalar = |name: &str, number: i32| FieldDescriptorProto {
        name: Some(name.to_string()),
        number: Some(number),
        label: Some(Label::Optional as i32),
        r#type: Some(Type::Int32 as i32),
        ..Default::default()
    };
    let inner_desc = DescriptorProto {
        name: Some("Inner".to_string()),
        field: vec![scalar("id", 3)],
        ..Default::default()
    };
    let outer_desc = DescriptorProto {
        name: Some("Outer".to_string()),
        field: vec![
            FieldDescriptorProto {
                name: Some("vals".to_string()),
                number: Some(1),
                label: Some(Label::Repeated as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("tail".to_string()),
                number: Some(2),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Message as i32),
                type_name: Some(".test.Inner".to_string()),
                ..Default::default()
            },
            scalar("a", 3),
            scalar("b", 4),
        ],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_packed_run_with_tail.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![outer_desc, inner_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-packed-run-with-tail-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // vals: field 1 (tag 0x0A, LEN/packed), 3 one-byte elements;
    // tail: field 2 (tag 0x12, LEN) wrapping Inner { id: 5 };
    // a: field 3 (tag 0x18, varint) = 42; b: field 4 (tag 0x20) = 43.
    let blob = [
        0x0Au8, 0x03, 0x05, 0x06, 0x07, //
        0x12, 0x02, 0x18, 0x05, //
        0x18, 0x2A, //
        0x20, 0x2B,
    ];
    let decoded = decode(wrapped(&blob), &mut ctx, RootType::Named("test.Outer"), 2).unwrap();
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

    // Spec 0216 S22: the run is one node, so the root has four children
    // — the record, then tail, a, b — where it used to have six.
    let children: Vec<usize> = (0..app.child_count(app.first_node))
        .map(|k| {
            app.nth_child(app.first_node, k)
                .expect("k is below the child count")
        })
        .collect();
    assert_eq!(
        children.len(),
        4,
        "fixture must decode the packed run plus tail, a, b"
    );
    assert!(
        app.tree[children[0]].span.packed_record_start != NO_PACKED_RECORD,
        "the first child must be the packed run"
    );
    (app, children[0], children[1], children[2], children[3])
}

/// `Holder { int32 pad = 1; bytes blob = 2; }` whose `blob` holds an
/// encoded `Payload { repeated int32 vals = 1; }` — i.e. a packed run
/// that only exists *after* an override retypes `blob`, and that the
/// splice therefore has to translate from the retyped node's own byte
/// frame into the document's.
///
/// `pad` is there to make that translation observable: the field being
/// retyped has to start somewhere other than byte 0, or the two frames
/// coincide and any missing translation is invisible.
///
/// Byte layout, which the tests assert against directly:
///
/// ```text
///   0: 0A 09        RootType::Named's synthetic wrapper field
///   2: 08 01        pad = 1
///   4: 12 05        blob, LEN 5            <- retyped node's tag
///   6: 0A 03        Payload.vals, packed   <- the packed record's tag
///   8: 05 06 07     the three elements
/// ```
pub(super) fn nested_packed_run_fixture() -> App {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    let payload_desc = DescriptorProto {
        name: Some("Payload".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("vals".to_string()),
            number: Some(1),
            label: Some(Label::Repeated as i32),
            r#type: Some(Type::Int32 as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let holder_desc = DescriptorProto {
        name: Some("Holder".to_string()),
        field: vec![
            FieldDescriptorProto {
                name: Some("pad".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("blob".to_string()),
                number: Some(2),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Bytes as i32),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_nested_packed_run.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![holder_desc, payload_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-nested-packed-run-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    let blob = [
        0x08u8, 0x01, //
        0x12, 0x05, //
        0x0A, 0x03, 0x05, 0x06, 0x07,
    ];
    let decoded = decode(wrapped(&blob), &mut ctx, RootType::Named("test.Holder"), 2).unwrap();
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
    app
}
