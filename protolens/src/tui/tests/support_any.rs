// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Fixtures for the two containers whose payload is described by the
//! data rather than by the schema: `google.protobuf.Any` and MessageSet.

use super::super::*;

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

    use crate::decode::{decode, DescriptorContext, RootType};

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
    // Deliberately not removed — see `type_as_fixture`'s note: a test
    // built on this fixture hashes the descriptor set, and that hash is a
    // fresh read from disk.

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

    let decoded = decode(
        wrapped(&blob),
        &mut ctx,
        RootType::Named("ms_test.Container"),
        2,
    )
    .unwrap();
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

/// `acme.Level1 { Level2 l2 { Level3 l3 { google.protobuf.Any payload
/// } } }`, the `Any` carrying `acme.Payload { label: "hello" }` — the
/// same `Any` shape the inline fixtures in this crate's tests already
/// use, but buried under two plain message levels that carry no
/// override of their own.
///
/// Spec 0183 needs exactly that burial. Once `is_message` leaves the
/// recursion gate, a plain message is descended into only if something
/// beneath it was marked, so `Level2` and `Level3` are the ancestors
/// the seed scan's upward marking walk has to set bits on. A fixture
/// with the `Any` directly under the root would still pass with the
/// upward walk missing entirely, and would prove nothing.
pub(super) fn nested_any_fixture() -> App {
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

    // A single-field message wrapping `type_name` at field 1.
    let wrapper = |name: &str, type_name: &str, field_name: &str| DescriptorProto {
        name: Some(name.to_string()),
        field: vec![FieldDescriptorProto {
            name: Some(field_name.to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Message as i32),
            type_name: Some(type_name.to_string()),
            ..Default::default()
        }],
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
    let acme_file = FileDescriptorProto {
        name: Some("acme_nested_any.proto".to_string()),
        syntax: Some("proto2".to_string()),
        package: Some("acme".to_string()),
        dependency: vec!["google/protobuf/any.proto".to_string()],
        message_type: vec![
            payload_msg,
            wrapper("Level3", ".google.protobuf.Any", "payload"),
            wrapper("Level2", ".acme.Level3", "l3"),
            wrapper("Level1", ".acme.Level2", "l2"),
        ],
        ..Default::default()
    };
    let fds = FileDescriptorSet {
        file: vec![any_file, acme_file],
    };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-nested-any-descriptor-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    let mut blob = vec![0x0au8, b"hello".len() as u8];
    blob.extend_from_slice(b"hello");
    let type_url = b"type.googleapis.com/acme.Payload";
    let mut any_bytes = vec![0x0au8, type_url.len() as u8];
    any_bytes.extend_from_slice(type_url);
    any_bytes.push(0x12);
    any_bytes.push(blob.len() as u8);
    any_bytes.append(&mut blob);
    // Three LEN wrappers, all field 1, all short enough for a one-byte
    // length varint: `Level3.payload`, then `Level2.l3`, then
    // `Level1.l2` — the last of which *is* `Level1`'s body, which is
    // what `decode` is handed.
    let mut blob = any_bytes;
    for _ in 0..3 {
        let mut next = vec![0x0au8, blob.len() as u8];
        next.append(&mut blob);
        blob = next;
    }

    let decoded = decode(wrapped(&blob), &mut ctx, RootType::Named("acme.Level1"), 2).unwrap();
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

/// `ms_test.Top { Mid m { Container c { extensions: TestMessageSet {
/// Item { type_id: 100, message: ExtPayload { label: "hi" } } } } } }`
/// — `message_set_fixture`'s document buried under two plain message
/// levels, for the same reason `nested_any_fixture` buries its `Any`.
///
/// MessageSet is the half of spec 0183 S1 that is easy to break
/// silently: tier 1 (the `Item` group wrapper) is deliberately *not* an
/// `is_auto_expand_candidate`, on the recorded grounds that the
/// `is_message` disjunct reaches it. Delete that disjunct without
/// widening the predicate and MessageSet expansion stops, with no
/// panic. The 1.1 MB `FileDescriptorSet` used for profiling contains no
/// MessageSet at all, so only a fixture like this one can catch it.
pub(super) fn nested_message_set_fixture() -> App {
    use prost::Message as _;
    use prost_types::descriptor_proto::ExtensionRange;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
        MessageOptions,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

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
    let wrapper = |name: &str, type_name: &str, field_name: &str| DescriptorProto {
        name: Some(name.to_string()),
        field: vec![FieldDescriptorProto {
            name: Some(field_name.to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Message as i32),
            type_name: Some(type_name.to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("ms_test_nested.proto".to_string()),
        syntax: Some("proto2".to_string()),
        package: Some("ms_test".to_string()),
        message_type: vec![
            message_set_msg,
            ext_payload_msg,
            container_msg,
            wrapper("Mid", ".ms_test.Container", "c"),
            wrapper("Top", ".ms_test.Mid", "m"),
        ],
        extension: vec![extension_field],
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path = std::env::temp_dir().join(format!(
        "protolens-tui-nested-message-set-descriptor-{n}.pb"
    ));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    let ext_payload_bytes = [0x0au8, 0x02, b'h', b'i'];
    let mut item_bytes = vec![0x0bu8, 0x10, 100u8];
    item_bytes.push(0x1a);
    item_bytes.push(ext_payload_bytes.len() as u8);
    item_bytes.extend_from_slice(&ext_payload_bytes);
    item_bytes.push(0x0c); // END_GROUP
    let mut container_bytes = vec![0x12u8, item_bytes.len() as u8];
    container_bytes.extend_from_slice(&item_bytes);
    // `Mid { c: Container }` — and then `Top`'s own body is just
    // `m: Mid`, which is what `decode` is handed.
    let mut mid_bytes = vec![0x0au8, container_bytes.len() as u8];
    mid_bytes.append(&mut container_bytes);
    let mut blob = vec![0x0au8, mid_bytes.len() as u8];
    blob.append(&mut mid_bytes);

    let decoded = decode(wrapped(&blob), &mut ctx, RootType::Named("ms_test.Top"), 2).unwrap();
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
