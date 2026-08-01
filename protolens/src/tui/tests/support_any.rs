// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Fixtures for the two containers whose payload is described by the
//! data rather than by the schema: `google.protobuf.Any` and MessageSet.

use super::super::*;
use super::support_build::{
    any_body, any_proto_file, field, field_of, fixture_app, fixture_under, message,
    wrap_len_field_1, wrapper_message, TempFile,
};
use prost_types::descriptor_proto::ExtensionRange;
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::{
    DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet, MessageOptions,
};

use crate::decode::{decode, RootType};

/// The MessageSet schema both fixtures below read their bytes under: a
/// `message_set_wire_format` container `TestMessageSet`, an extension
/// `ext_payload = 100` carrying `ExtPayload { optional string label = 1
/// }`, and a `Container` holding the set at field 2.
///
/// `buried_under` is appended to the file's message list — empty for the
/// direct fixture, two burial levels for the nested one.
fn ms_test_fds(file_name: &str, buried_under: Vec<DescriptorProto>) -> FileDescriptorSet {
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
    let extension_field = FieldDescriptorProto {
        extendee: Some(".ms_test.TestMessageSet".to_string()),
        ..field_of(
            "ext_payload",
            100,
            Label::Optional,
            Type::Message,
            ".ms_test.ExtPayload",
        )
    };
    let mut message_type = vec![
        message_set_msg,
        message(
            "ExtPayload",
            vec![field("label", 1, Label::Optional, Type::String)],
        ),
        message(
            "Container",
            vec![field_of(
                "extensions",
                2,
                Label::Optional,
                Type::Message,
                ".ms_test.TestMessageSet",
            )],
        ),
    ];
    message_type.extend(buried_under);
    FileDescriptorSet {
        file: vec![FileDescriptorProto {
            name: Some(file_name.to_string()),
            syntax: Some("proto2".to_string()),
            package: Some("ms_test".to_string()),
            message_type,
            extension: vec![extension_field],
            ..Default::default()
        }],
    }
}

/// `Container`'s body: `extensions: TestMessageSet { Item { type_id:
/// 100, message: ExtPayload { label: "hi" } } }`.
fn ms_test_container_bytes() -> Vec<u8> {
    let ext_payload_bytes = [0x0au8, 0x02, b'h', b'i'];
    let mut item_bytes = vec![0x0bu8, 0x10, 100u8];
    item_bytes.push(0x1a);
    item_bytes.push(ext_payload_bytes.len() as u8);
    item_bytes.extend_from_slice(&ext_payload_bytes);
    item_bytes.push(0x0c); // END_GROUP
    let mut container_bytes = vec![0x12u8, item_bytes.len() as u8];
    container_bytes.extend_from_slice(&item_bytes);
    container_bytes
}

/// Builds the shared `Container { extensions: TestMessageSet { Item {
/// type_id: 100, message: ExtPayload { label: "hi" } } } }` fixture
/// used by both the auto-expansion test and the toggle/reactivate
/// regression test below.
pub(super) fn message_set_fixture() -> App {
    let descriptor = TempFile::descriptor(
        "message-set-expand",
        &ms_test_fds("ms_test.proto", Vec::new()),
    );
    let mut ctx = descriptor.load();
    // Deliberately kept — see `type_as_fixture`'s note: a test built on
    // this fixture hashes the descriptor set, and that hash is a fresh
    // read from disk.
    let _kept = descriptor.keep();

    let blob = ms_test_container_bytes();
    let decoded = decode(
        wrapped(&blob),
        &mut ctx,
        RootType::Named("ms_test.Container"),
        2,
    )
    .unwrap();
    fixture_app(decoded, ctx)
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
    let acme_file = FileDescriptorProto {
        name: Some("acme_nested_any.proto".to_string()),
        syntax: Some("proto2".to_string()),
        package: Some("acme".to_string()),
        dependency: vec!["google/protobuf/any.proto".to_string()],
        message_type: vec![
            message(
                "Payload",
                vec![field("label", 1, Label::Optional, Type::String)],
            ),
            wrapper_message("Level3", "payload", ".google.protobuf.Any"),
            wrapper_message("Level2", "l3", ".acme.Level3"),
            wrapper_message("Level1", "l2", ".acme.Level2"),
        ],
        ..Default::default()
    };
    let fds = FileDescriptorSet {
        file: vec![any_proto_file(), acme_file],
    };

    let mut blob = any_body("type.googleapis.com/acme.Payload", b"\x0a\x05hello");
    // Three LEN wrappers, all field 1: `Level3.payload`, then
    // `Level2.l3`, then `Level1.l2` — the last of which *is* `Level1`'s
    // body, which is what `decode` is handed.
    for _ in 0..3 {
        blob = wrap_len_field_1(blob);
    }

    fixture_under("nested-any", &fds, "acme.Level1", &blob)
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
    let fds = ms_test_fds(
        "ms_test_nested.proto",
        vec![
            wrapper_message("Mid", "c", ".ms_test.Container"),
            wrapper_message("Top", "m", ".ms_test.Mid"),
        ],
    );

    // `Mid { c: Container }` — and then `Top`'s own body is just
    // `m: Mid`, which is what `decode` is handed.
    let blob = wrap_len_field_1(wrap_len_field_1(ms_test_container_bytes()));

    fixture_under("nested-message-set", &fds, "ms_test.Top", &blob)
}
