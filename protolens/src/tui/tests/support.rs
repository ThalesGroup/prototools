// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::super::heat_cue::HEAT_CUE_PREVIEW;
use super::super::*;
pub(super) use prototext_core::helpers::{WT_LEN, WT_VARINT};
pub(super) use prototext_core::serialize::render_text::NodeSpan;
use prototext_graph::build_scoring_graph::build_from_strings;
use prototext_graph::score::load::LoadedGraph;
pub(super) use ratatui::backend::TestBackend;

pub(super) fn empty_app() -> App {
    let decoded = Decoded {
        lines: Vec::new(),
        tree: Vec::new(),
        root_type: "google.protobuf.Empty".to_string(),
        blob: Vec::new(),
        wrapper_offset: 0,
        style_hints: Vec::new(),
        root_type_deferred: false,
    };
    App::new(
        decoded,
        "empty.pb",
        PathBuf::from("empty.pb"),
        2,
        DescriptorContext::empty_for_test(),
        ThemeKind::Dark,
        None,
    )
}

/// A single-node tree whose root is a message/group node — the
/// minimal fixture needed to exercise `t`'s override-target
/// validation (spec 0114 §1).
pub(super) fn message_node_app() -> App {
    let lines: Vec<String> = vec!["message_type {".to_string(), "}".to_string()];
    let node = TreeNode {
        span: NodeSpan {
            field_number: 4,
            raw_range: 0..10,
            text_range: 0..2,
            level: 0,
            type_fqdn: Some("google.protobuf.DescriptorProto".to_string()),
            is_message: true,
            packed_record_start: None,
            wire_type: WT_LEN,
        },
        parent: None,
        first_child: None,
        last_child: None,
        next_sibling: None,
        prev_sibling: None,
        doc_next: None,
        doc_prev: None,
        rendered_as: None,
    };
    let decoded = Decoded {
        lines,
        tree: vec![node],
        root_type: "google.protobuf.FileDescriptorProto".to_string(),
        // Tag `0x22` = field 4 << 3 | WT_LEN(2), length varint `0x08`
        // = 8, then 8 zero payload bytes — a real, `raw_range`-
        // consistent blob, needed since spec 0132's live preview now
        // splices this node's contents at pane-open time.
        blob: vec![0x22, 0x08, 0, 0, 0, 0, 0, 0, 0, 0],
        wrapper_offset: 0,
        style_hints: vec![Vec::new(); 2],
        root_type_deferred: false,
    };
    App::new(
        decoded,
        "test.pb",
        PathBuf::from("test.pb"),
        2,
        DescriptorContext::empty_for_test(),
        ThemeKind::Dark,
        None,
    )
}

/// A minimal, real, in-memory scoring graph (spec 0152 test plan) —
/// `HEAT_CUE_PREVIEW` messages, each with a single `uint64` field 1 —
/// built with zero file I/O via `build_from_strings` + `Box::leak` +
/// `LoadedGraph::from_static_bytes` (as spec 0151's own notes
/// anticipated). At least `HEAT_CUE_PREVIEW` non-vetoed candidates are
/// needed for `heat_cue_for`'s `[0, HEAT_CUE_PREVIEW)` window to ever
/// be satisfiable — a single-entry graph (as `heat_worker.rs`'s own,
/// lower-level round-trip test uses) is never enough here.
pub(super) fn test_scoring_graph() -> LoadedGraph {
    let mut yaml = String::from("entries:\n");
    for i in 0..HEAT_CUE_PREVIEW {
        yaml.push_str(&format!("- Msg{i}\n"));
    }
    yaml.push_str("messages:\n");
    for i in 0..HEAT_CUE_PREVIEW {
        yaml.push_str(&format!(
            "  Msg{i}:\n    fields:\n    - number: 1\n      type: uint64\n"
        ));
    }
    let (bytes, _, _) =
        build_from_strings(&[yaml], false, false, |_, _| {}).expect("test graph must build");
    let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    LoadedGraph::from_static_bytes(bytes).expect("test graph must load")
}

/// `message_node_app` with a real scoring graph attached via
/// `DescriptorContext::for_test_with_graph` (spec 0152 test plan) —
/// for tests that need `App.ctx.graph` to be genuinely `Some`, e.g. to
/// spawn a real `HeatWorkerHandle` end-to-end.
pub(super) fn message_node_app_with_graph() -> App {
    let mut app = message_node_app();
    app.ctx = DescriptorContext::for_test_with_graph(test_scoring_graph());
    app
}

/// `n` document-order-linked scalar sibling nodes at the root level
/// (spec 0113 D16: root-level nodes are sibling-linked despite having
/// no `parent`), one line of text each — the minimal fixture for
/// exercising main-pane search (spec 0114 §4, extended from the
/// override pane), which walks `doc_next`/`doc_prev`.
pub(super) fn sibling_leaves_app(texts: &[&str]) -> App {
    let lines: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
    let n = lines.len();
    let tree: Vec<TreeNode> = (0..n)
        .map(|i| TreeNode {
            span: NodeSpan {
                field_number: i as u64 + 1,
                raw_range: (i * 10)..(i * 10 + 5),
                text_range: i..i + 1,
                level: 0,
                type_fqdn: None,
                is_message: false,
                packed_record_start: None,
                wire_type: WT_VARINT,
            },
            parent: None,
            first_child: None,
            last_child: None,
            next_sibling: (i + 1 < n).then_some(i + 1),
            prev_sibling: i.checked_sub(1),
            doc_next: (i + 1 < n).then_some(i + 1),
            doc_prev: i.checked_sub(1),
            rendered_as: None,
        })
        .collect();
    let decoded = Decoded {
        lines,
        tree,
        root_type: "google.protobuf.FileDescriptorProto".to_string(),
        blob: Vec::new(),
        wrapper_offset: 0,
        style_hints: Vec::new(),
        root_type_deferred: false,
    };
    App::new(
        decoded,
        "test.pb",
        PathBuf::from("test.pb"),
        2,
        DescriptorContext::empty_for_test(),
        ThemeKind::Dark,
        None,
    )
}

/// `Outer { repeated int32 vals = 1; }`, packed, 3 elements (`5, 6,
/// 7`), document order — spec 0124's shared fixture: gives a
/// `PathField`/`FqdnField` origin (parent path `/`, field `1`) 3
/// matches, and a `Path` origin (e.g. `/2`) exactly 1 match. Uses a
/// packed *scalar* repeated field (one `NodeSpan` per element, spec
/// 0115) rather than a repeated message field, to keep the fixture's
/// tree shape simple (no nested-message decode involved).
pub(super) fn repeated_scalar_fixture() -> (App, Vec<usize>) {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext};

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
    let decoded = decode(&blob, &mut ctx, Some("test.Outer"), 2, false).unwrap();
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

    let mut items: Vec<usize> = app
        .tree
        .iter()
        .enumerate()
        .filter(|(_, n)| n.span.packed_record_start.is_some())
        .map(|(i, _)| i)
        .collect();
    items.sort_by_key(|&i| app.positional_path(i));
    assert_eq!(items.len(), 3, "fixture must contain 3 packed elements");
    (app, items)
}

/// Builds the same `Outer { inner: Inner { id: 5 } }` fixture as
/// `enter_key_applies_override_and_closes_pane`, for the `:type-as`/
/// `:type-as-raw` command tests (spec 0114 §7).
pub(super) fn type_as_fixture() -> (App, usize, usize) {
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
        name: Some("test_type_as.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![outer_desc, inner_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    // Unique per call (this fixture is shared by several tests that
    // may run concurrently) to avoid one test's cleanup racing
    // another's read of the same path.
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-type-as-descriptor-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // Outer { inner: Inner { id: 5 } }.
    let blob = [0x0Au8, 0x02, 0x08, 0x05];
    let decoded = decode(&blob, &mut ctx, Some("test.Outer"), 2, false).unwrap();
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

    let inner_idx = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some("test.Inner"))
        .expect("tree must contain the Inner submessage");
    let id_idx = app.tree[inner_idx]
        .first_child
        .expect("Inner has at least one child");
    (app, inner_idx, id_idx)
}

/// `Outer { inner: Inner {} }` — same schema as `type_as_fixture`, but
/// `inner`'s payload is zero-length: a genuinely empty, still-bracketed
/// submessage (rendered as `inner {` then `}` on the next line, no
/// body in between). Regression fixture for spec 0142's fix (2026-07-
/// 18 feedback): an empty message has `first_child == None` yet is
/// still a real two-line bracketed node — must be foldable and its
/// footer line must be a reachable cursor stop, same as any other
/// message.
pub(super) fn empty_message_fixture() -> (App, usize) {
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
        name: Some("test_empty_message.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![outer_desc, inner_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-empty-message-descriptor-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // Outer { inner: Inner {} } — field 1 (LEN), length 0, no payload.
    let blob = [0x0Au8, 0x00];
    let decoded = decode(&blob, &mut ctx, Some("test.Outer"), 2, false).unwrap();
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

    let inner_idx = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some("test.Inner"))
        .expect("tree must contain the empty Inner submessage");
    assert!(
        app.tree[inner_idx].first_child.is_none(),
        "fixture must exercise the no-children case"
    );
    (app, inner_idx)
}

/// `Outer3 { durability: Durability = 0 (EPHEMERAL) }` — a scalar
/// enum-typed field, for the enum-inclusive `natural_type` regression
/// tests (2026-07-18 feedback: `t` then `Esc` on an enum field, and
/// `t`'s initial highlight/mode on an enum field with no active
/// override).
pub(super) fn enum_field_fixture() -> (App, usize) {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, EnumDescriptorProto, EnumValueDescriptorProto, FieldDescriptorProto,
        FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext};

    let durability_enum = EnumDescriptorProto {
        name: Some("Durability".to_string()),
        value: vec![
            EnumValueDescriptorProto {
                name: Some("EPHEMERAL".to_string()),
                number: Some(0),
                ..Default::default()
            },
            EnumValueDescriptorProto {
                name: Some("PERSISTENT".to_string()),
                number: Some(1),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let outer_desc = DescriptorProto {
        name: Some("Outer3".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("durability".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Enum as i32),
            type_name: Some(".test.Durability".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_enum_field.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![outer_desc],
        enum_type: vec![durability_enum],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-enum-field-descriptor-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // Outer3 { durability: EPHEMERAL (0) } — field 1 (tag 0x08),
    // varint value 0.
    let blob = [0x08u8, 0x00];
    let decoded = decode(&blob, &mut ctx, Some("test.Outer3"), 2, false).unwrap();
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

    let durability_idx = app
        .tree
        .iter()
        .position(|n| n.span.field_number == 1)
        .expect("tree must contain the durability field");
    (app, durability_idx)
}

/// `Outer2 { grp: MyGroup { id: 5 } }`, with `grp` declared as a
/// genuine schema wire-group field (`Type::Group`) — unlike
/// `message_set_fixture`'s auto-expanded MessageSet group items,
/// this is directly schema-resolved from the start. Also registers
/// a same-shaped sibling type `NewGroup` to override `grp` into
/// (spec 0122 Test Plan item 2).
pub(super) fn group_type_fixture() -> (App, usize) {
    // START_GROUP(5), id=5, END_GROUP(5) — minimal tag encoding.
    group_type_fixture_with_blob(&[0x2Bu8, 0x08, 0x05, 0x2Cu8])
}

/// Same schema as `group_type_fixture`, but with `grp`'s `START_GROUP`
/// tag encoded with one overhang byte (non-minimal varint: `0xAB, 0x00`
/// instead of the minimal `0x2B`) — exercises the `tag_ohb: 1` anomaly
/// modifier (spec 0122 Test Plan item 2, 3rd bullet).
pub(super) fn group_type_fixture_with_tag_ohb() -> (App, usize) {
    group_type_fixture_with_blob(&[0xABu8, 0x00, 0x08, 0x05, 0x2Cu8])
}

pub(super) fn group_type_fixture_with_blob(blob: &[u8]) -> (App, usize) {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext};

    let my_group_desc = DescriptorProto {
        name: Some("MyGroup".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("id".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Int32 as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let new_group_desc = DescriptorProto {
        name: Some("NewGroup".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("value".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Int32 as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let outer_desc = DescriptorProto {
        name: Some("Outer2".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("grp".to_string()),
            number: Some(5),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Group as i32),
            type_name: Some(".test.MyGroup".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_group_type.proto".to_string()),
        package: Some("test".to_string()),
        syntax: Some("proto2".to_string()),
        message_type: vec![outer_desc, my_group_desc, new_group_desc],
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    // Unique per call (this fixture is shared by several tests that
    // may run concurrently) to avoid one test's cleanup racing
    // another's read of the same path.
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-group-type-descriptor-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    let decoded = decode(blob, &mut ctx, Some("test.Outer2"), 2, false).unwrap();
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

    let grp_idx = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some("test.MyGroup"))
        .expect("tree must contain the MyGroup submessage");
    (app, grp_idx)
}

/// Builds the shared `Container { extensions: TestMessageSet { Item {
/// type_id: 100, message: ExtPayload { label: "hi" } } } }` fixture
/// used by both the auto-expansion test and the toggle/reactivate
/// regression test below.
pub(super) fn message_set_fixture() -> App {
    use prost::Message as _;
    use prost_types::descriptor_proto::ExtensionRange;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
        MessageOptions,
    };

    use crate::decode::{decode, DescriptorContext};

    let message_set_msg = DescriptorProto {
        name: Some("TestMessageSet".to_string()),
        options: Some(MessageOptions {
            message_set_wire_format: Some(true),
            ..Default::default()
        }),
        extension_range: vec![ExtensionRange {
            start: Some(1),
            end: Some(536870912),
            ..Default::default()
        }],
        ..Default::default()
    };
    let ext_payload_msg = DescriptorProto {
        name: Some("ExtPayload".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("label".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::String as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let extension_field = FieldDescriptorProto {
        name: Some("ext_payload".to_string()),
        number: Some(100),
        label: Some(Label::Optional as i32),
        r#type: Some(Type::Message as i32),
        type_name: Some(".ms_test.ExtPayload".to_string()),
        extendee: Some(".ms_test.TestMessageSet".to_string()),
        ..Default::default()
    };
    let container_msg = DescriptorProto {
        name: Some("Container".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("extensions".to_string()),
            number: Some(2),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Message as i32),
            type_name: Some(".ms_test.TestMessageSet".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("ms_test.proto".to_string()),
        syntax: Some("proto2".to_string()),
        package: Some("ms_test".to_string()),
        message_type: vec![message_set_msg, ext_payload_msg, container_msg],
        extension: vec![extension_field],
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    // Unique per call (this fixture is shared by several tests that
    // may run concurrently) to avoid one test's cleanup racing
    // another's read of the same path.
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path = std::env::temp_dir().join(format!(
        "protolens-tui-message-set-expand-descriptor-{n}.pb"
    ));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // Container { extensions: TestMessageSet {
    //   Item { type_id: 100, message: ExtPayload { label: "hi" } }
    // } }.
    let ext_payload_bytes = [0x0au8, 0x02, b'h', b'i'];
    let mut item_bytes = vec![0x0bu8, 0x10, 100u8];
    item_bytes.push(0x1a);
    item_bytes.push(ext_payload_bytes.len() as u8);
    item_bytes.extend_from_slice(&ext_payload_bytes);
    item_bytes.push(0x0c); // END_GROUP
    let mut blob = vec![0x12u8, item_bytes.len() as u8];
    blob.extend_from_slice(&item_bytes);

    let decoded = decode(&blob, &mut ctx, Some("ms_test.Container"), 2, false).unwrap();
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

/// `test.Outer` (in `outer.proto`, package `test`) with schema fields
/// covering `resolve_export_fields`'s G6c tiers, plus two live
/// children at an undeclared field number (8) for the repeated/
/// tier-4 cases — spec 0156's Test plan. Returns the root-cursor
/// `App`; callers add their own override entries before calling
/// `resolve_export_fields`/`export_descriptor_bytes`.
///
/// Field layout (all direct children of the root):
/// - `1` (`num`, schema `int32`) — tier 3 primitive.
/// - `2` (`msg_field`, schema message `.other.Msg`, a *different*
///   file, `other.proto`) — tier 3 message + G6d dependency.
/// - `3` (`own_type_field`, schema message `.test.OwnType`, the
///   cursor's *own* file, `outer.proto`) — tier 3 message + G6d
///   "no exclusion" of the cursor's own file.
/// - `4` (`retype_field`, schema message `.test.OwnType`) — left for
///   callers to retype via an active `PathField` override (tier 1).
/// - `6` (`retype_field2`, schema message `.test.OwnType`) — left for
///   callers to retype via an active `FqdnField` override (tier 2).
/// - `7` (`raw_field`, schema message `.test.OwnType`) — left for
///   callers to retype to raw (`target: None`) via an active override.
/// - `8` (undeclared, two live children, `WT_VARINT`) — tier 4
///   primitive guess (`int64`), `LABEL_REPEATED` from the live count.
pub(super) fn export_fields_fixture() -> App {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext};

    let msg = DescriptorProto {
        name: Some("Msg".to_string()),
        ..Default::default()
    };
    let other_file = FileDescriptorProto {
        name: Some("other.proto".to_string()),
        package: Some("other".to_string()),
        message_type: vec![msg],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };

    let own_type = DescriptorProto {
        name: Some("OwnType".to_string()),
        ..Default::default()
    };
    let field = |name: &str, number: i32| FieldDescriptorProto {
        name: Some(name.to_string()),
        number: Some(number),
        label: Some(Label::Optional as i32),
        r#type: Some(Type::Message as i32),
        type_name: Some(".test.OwnType".to_string()),
        ..Default::default()
    };
    let outer = DescriptorProto {
        name: Some("Outer".to_string()),
        field: vec![
            FieldDescriptorProto {
                name: Some("num".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("msg_field".to_string()),
                number: Some(2),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Message as i32),
                type_name: Some(".other.Msg".to_string()),
                ..Default::default()
            },
            field("own_type_field", 3),
            field("retype_field", 4),
            field("retype_field2", 6),
            field("raw_field", 7),
        ],
        ..Default::default()
    };
    let outer_file = FileDescriptorProto {
        name: Some("outer.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![own_type, outer],
        dependency: vec!["other.proto".to_string()],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet {
        file: vec![other_file, outer_file],
    };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-export-fields-descriptor-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // field 1 = 5, fields 2/3/4/6/7 = empty submessages, field 8 (twice,
    // undeclared) = varints 1 and 2.
    let blob = vec![
        0x08, 0x05, // 1: num = 5
        0x12, 0x00, // 2: msg_field {}
        0x1A, 0x00, // 3: own_type_field {}
        0x22, 0x00, // 4: retype_field {}
        0x32, 0x00, // 6: retype_field2 {}
        0x3A, 0x00, // 7: raw_field {}
        0x40, 0x01, // 8: 1
        0x40, 0x02, // 8: 2
    ];
    let decoded = decode(&blob, &mut ctx, Some("test.Outer"), 2, false).unwrap();
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

/// `test.GroupHolder` (no declared fields), whose single live child is
/// an untyped `WT_START_GROUP` field (9) — `resolve_export_fields`'s
/// tier-4 "no supported guess for a group" error case.
pub(super) fn export_fields_group_error_fixture() -> App {
    use prost::Message as _;
    use prost_types::{DescriptorProto, FileDescriptorProto, FileDescriptorSet};

    use crate::decode::{decode, DescriptorContext};

    let holder = DescriptorProto {
        name: Some("GroupHolder".to_string()),
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("group_holder.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![holder],
        syntax: Some("proto2".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-group-holder-descriptor-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // field 9 (undeclared): START_GROUP then END_GROUP.
    let blob = vec![0x4B, 0x4C];
    let decoded = decode(&blob, &mut ctx, Some("test.GroupHolder"), 2, false).unwrap();
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
