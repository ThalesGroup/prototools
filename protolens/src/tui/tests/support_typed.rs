// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Fixtures for retyping a node: a message, an empty message, an enum, a
//! group. Each is a document paired with the descriptor pool that gives
//! the override pane something to offer for it.

use super::super::*;
use super::support_build::{fixture_app, fixture_under, TempFile};
use super::support_inspect::node_with_type;
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::{DescriptorProto, EnumDescriptorProto, EnumValueDescriptorProto};
use prost_types::{FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet};

use crate::decode::{decode, RootType};

/// `Outer { optional Inner inner = 1; }` over `Inner { optional int32
/// id = 1; }` — the schema shared by `type_as_fixture` and
/// `empty_message_fixture`, which differ only in `inner`'s payload.
///
/// `file_name` is a parameter, and the two callers pass different names,
/// because a descriptor's own `.proto` file name is *observable*: `v`
/// resolves it under `proto_root` and reports it by name when it is not
/// there.
fn outer_inner_fds(file_name: &str) -> FileDescriptorSet {
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
        name: Some(file_name.to_string()),
        package: Some("test".to_string()),
        message_type: vec![outer_desc, inner_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    FileDescriptorSet { file: vec![file] }
}

/// Builds the same `Outer { inner: Inner { id: 5 } }` fixture as
/// `enter_key_applies_override_and_closes_pane`, for the `:type-as`/
/// `:type-as-raw` command tests (spec 0114 §7).
pub(super) fn type_as_fixture() -> (App, usize, usize) {
    let descriptor = TempFile::descriptor("type-as", &outer_inner_fds("test_type_as.proto"));
    let mut ctx = descriptor.load();
    // Deliberately kept, unlike every other fixture here: the
    // `:save`/`:restore` tests built on this one hash the descriptor set,
    // which `descriptor_sha256` re-reads from disk on demand (spec 0197
    // §S6), and that read is now an error rather than an empty hash.
    let _kept = descriptor.keep();

    // Outer { inner: Inner { id: 5 } }.
    let blob = [0x0Au8, 0x02, 0x08, 0x05];
    let decoded = decode(wrapped(&blob), &mut ctx, RootType::Named("test.Outer"), 2).unwrap();
    let mut app = fixture_app(decoded, ctx);
    // The fixture writes a bare `.pb` with no `index.rkyv` beside it, so
    // `App::new` seeds the status line with spec 0197 §S3's eager-fallback
    // warning. Clear it: tests here assert on messages *they* provoke.
    app.message.clear();

    let inner_idx =
        node_with_type(&app, "test.Inner").expect("tree must contain the Inner submessage");
    let id_idx = app
        .first_child(inner_idx)
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
    // Outer { inner: Inner {} } — field 1 (LEN), length 0, no payload.
    let app = fixture_under(
        "empty-message",
        &outer_inner_fds("test_empty_message.proto"),
        "test.Outer",
        &[0x0Au8, 0x00],
    );

    let inner_idx =
        node_with_type(&app, "test.Inner").expect("tree must contain the empty Inner submessage");
    assert!(
        app.first_child(inner_idx).is_none(),
        "fixture must exercise the no-children case"
    );
    (app, inner_idx)
}

/// `Outer { inner: Inner { id: 5, 9: 7 } }` — `type_as_fixture`'s
/// schema and one extra field no descriptor declares, so exactly one
/// row is defective and every node above it must roll up to
/// `Status::Unknown` (spec 0247).
///
/// Returns the `Inner` node and its two children, the known one first.
pub(super) fn unknown_field_fixture() -> (App, usize, usize, usize) {
    // `0A 04` — Outer field 1, four payload bytes — then `Inner`'s own
    // `08 05` (`id: 5`) and `48 07` (field 9, varint, no schema).
    let app = fixture_under(
        "unknown-field",
        &outer_inner_fds("test_unknown_field.proto"),
        "test.Outer",
        &[0x0Au8, 0x04, 0x08, 0x05, 0x48, 0x07],
    );
    let inner = node_with_type(&app, "test.Inner").expect("tree must contain the Inner submessage");
    let known = app.nth_child(inner, 0).expect("Inner renders `id: 5`");
    let unknown = app.nth_child(inner, 1).expect("Inner renders field 9");
    (app, inner, known, unknown)
}

/// `Outer3 { durability: Durability = 0 (EPHEMERAL) }` — a scalar
/// enum-typed field, for the enum-inclusive `natural_type` regression
/// tests (2026-07-18 feedback: `t` then `Esc` on an enum field, and
/// `t`'s initial highlight/mode on an enum field with no active
/// override).
pub(super) fn enum_field_fixture() -> (App, usize) {
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

    // Outer3 { durability: EPHEMERAL (0) } — field 1 (tag 0x08),
    // varint value 0.
    let app = fixture_under("enum-field", &fds, "test.Outer3", &[0x08u8, 0x00]);

    // Not a scan for field 1: the wrapper occupies slot 0 and is field 1
    // too (spec 0216 S1), so a scan would find it first.
    let durability_idx = app
        .nth_child(app.first_node, 0)
        .expect("tree must contain the durability field");
    (app, durability_idx)
}

/// `Scalars { string text = 1; int32 n = 2; bytes blob = 3; }` — one
/// node for each branch of spec 0335 S1's predicate, in one document,
/// so a test can assert the whole rule and not one leg of it.
///
/// `blob`'s payload is `08 05`, which is a valid message: the carve-out
/// exists because that is the common case, and a fixture whose `bytes`
/// held something unparseable would let a wrong implementation pass.
///
/// Returns `(app, text, n, blob)`.
pub(super) fn declared_scalars_fixture() -> (App, usize, usize, usize) {
    let scalar = |name: &str, number: i32, ty: Type| FieldDescriptorProto {
        name: Some(name.to_string()),
        number: Some(number),
        label: Some(Label::Optional as i32),
        r#type: Some(ty as i32),
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_declared_scalars.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![DescriptorProto {
            name: Some("Scalars".to_string()),
            field: vec![
                scalar("text", 1, Type::String),
                scalar("n", 2, Type::Int32),
                scalar("blob", 3, Type::Bytes),
            ],
            ..Default::default()
        }],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    // text: "hi", n: 5, blob: "\010\005".
    let app = fixture_under(
        "declared-scalars",
        &fds,
        "test.Scalars",
        &[0x0A, 0x02, b'h', b'i', 0x10, 0x05, 0x1A, 0x02, 0x08, 0x05],
    );
    // Not a scan for the field numbers: the wrapper occupies slot 0 and
    // is field 1 too (spec 0216 S1), so a scan would find it first.
    let mut kids = (0..3).map(|k| {
        app.nth_child(app.first_node, k)
            .expect("the document renders all three fields")
    });
    let (text, n, blob) = (
        kids.next().unwrap(),
        kids.next().unwrap(),
        kids.next().unwrap(),
    );
    (app, text, n, blob)
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

/// What the two above share: everything but the group's own bytes.
/// Private to this file — the tests want one of the two named blobs, not
/// a blob of their own.
fn group_type_fixture_with_blob(blob: &[u8]) -> (App, usize) {
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

    let app = fixture_under("group-type", &fds, "test.Outer2", blob);

    let grp_idx =
        node_with_type(&app, "test.MyGroup").expect("tree must contain the MyGroup submessage");
    (app, grp_idx)
}
