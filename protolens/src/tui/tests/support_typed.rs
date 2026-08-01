// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Fixtures for retyping a node: a message, an empty message, an enum, a
//! group. Each is a document paired with the descriptor pool that gives
//! the override pane something to offer for it.

use super::super::*;
use super::support_inspect::node_with_type;

/// Builds the same `Outer { inner: Inner { id: 5 } }` fixture as
/// `enter_key_applies_override_and_closes_pane`, for the `:type-as`/
/// `:type-as-raw` command tests (spec 0114 §7).
pub(super) fn type_as_fixture() -> (App, usize, usize) {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

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
    // Deliberately not removed, unlike every other fixture here: the
    // `:save`/`:restore` tests built on this one hash the descriptor set,
    // which `descriptor_sha256` re-reads from disk on demand (spec 0197
    // §S6), and that read is now an error rather than an empty hash.

    // Outer { inner: Inner { id: 5 } }.
    let blob = [0x0Au8, 0x02, 0x08, 0x05];
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
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

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

    let inner_idx =
        node_with_type(&app, "test.Inner").expect("tree must contain the empty Inner submessage");
    assert!(
        app.first_child(inner_idx).is_none(),
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

    use crate::decode::{decode, DescriptorContext, RootType};

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
    let decoded = decode(wrapped(&blob), &mut ctx, RootType::Named("test.Outer3"), 2).unwrap();
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

    // Not a scan for field 1: the wrapper occupies slot 0 and is field 1
    // too (spec 0216 S1), so a scan would find it first.
    let durability_idx = app
        .nth_child(app.first_node, 0)
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

/// What the two above share: everything but the group's own bytes.
/// Private to this file — the tests want one of the two named blobs, not
/// a blob of their own.
fn group_type_fixture_with_blob(blob: &[u8]) -> (App, usize) {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

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

    let decoded = decode(wrapped(blob), &mut ctx, RootType::Named("test.Outer2"), 2).unwrap();
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

    let grp_idx =
        node_with_type(&app, "test.MyGroup").expect("tree must contain the MyGroup submessage");
    (app, grp_idx)
}
