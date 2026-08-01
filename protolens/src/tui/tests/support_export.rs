// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Fixtures for the two things done to a subtree rather than to a node:
//! exporting it as a descriptor, and pruning it.

use super::super::*;
use super::support_build::{field, field_of, fixture_under, message, proto3_fds};
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::{FileDescriptorProto, FileDescriptorSet};

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
    let other_file = FileDescriptorProto {
        name: Some("other.proto".to_string()),
        package: Some("other".to_string()),
        message_type: vec![message("Msg", Vec::new())],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };

    let own = |name: &str, number: i32| {
        field_of(
            name,
            number,
            Label::Optional,
            Type::Message,
            ".test.OwnType",
        )
    };
    let outer_file = FileDescriptorProto {
        name: Some("outer.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![
            message("OwnType", Vec::new()),
            message(
                "Outer",
                vec![
                    field("num", 1, Label::Optional, Type::Int32),
                    field_of("msg_field", 2, Label::Optional, Type::Message, ".other.Msg"),
                    own("own_type_field", 3),
                    own("retype_field", 4),
                    own("retype_field2", 6),
                    own("raw_field", 7),
                ],
            ),
        ],
        dependency: vec!["other.proto".to_string()],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet {
        file: vec![other_file, outer_file],
    };

    // field 1 = 5, fields 2/3/4/6/7 = empty submessages, field 8 (twice,
    // undeclared) = varints 1 and 2.
    fixture_under(
        "export-fields",
        &fds,
        "test.Outer",
        &[
            0x08, 0x05, // 1: num = 5
            0x12, 0x00, // 2: msg_field {}
            0x1A, 0x00, // 3: own_type_field {}
            0x22, 0x00, // 4: retype_field {}
            0x32, 0x00, // 6: retype_field2 {}
            0x3A, 0x00, // 7: raw_field {}
            0x40, 0x01, // 8: 1
            0x40, 0x02, // 8: 2
        ],
    )
}

/// `test.GroupHolder` (no declared fields), whose single live child is
/// an untyped `WT_START_GROUP` field (9) — `resolve_export_fields`'s
/// tier-4 "no supported guess for a group" error case.
pub(super) fn export_fields_group_error_fixture() -> App {
    let fds = FileDescriptorSet {
        file: vec![FileDescriptorProto {
            name: Some("group_holder.proto".to_string()),
            package: Some("test".to_string()),
            message_type: vec![message("GroupHolder", Vec::new())],
            syntax: Some("proto2".to_string()),
            ..Default::default()
        }],
    };

    // field 9 (undeclared): START_GROUP then END_GROUP.
    fixture_under("group-holder", &fds, "test.GroupHolder", &[0x4B, 0x4C])
}

/// `Outer { Wrap head = 1; Wrap tail = 2; }`, with `Wrap { Leaf leaf =
/// 1; }`, `Leaf { int32 v = 1; }`, and a `Blob { string leaf = 1; }`
/// that reads the very same bytes as one line instead of three.
///
/// The shape a splice's line-count delta gets lost in, and the one no
/// other fixture here has: a node that can be retyped into a *shorter*
/// rendering, followed by a sibling whose own rendering cannot change —
/// so the override walk prunes it — but which still has an interior
/// that has to move. Returns `(app, head, tail, tail_leaf, tail_v)`.
pub(super) fn pruned_tail_fixture() -> (App, usize, usize, usize, usize) {
    let wrap_of = |name: &str, number: i32| {
        field_of(name, number, Label::Optional, Type::Message, ".test.Wrap")
    };
    let fds = proto3_fds(
        "test_pruned_tail.proto",
        vec![
            message("Outer", vec![wrap_of("head", 1), wrap_of("tail", 2)]),
            message(
                "Wrap",
                vec![field_of(
                    "leaf",
                    1,
                    Label::Optional,
                    Type::Message,
                    ".test.Leaf",
                )],
            ),
            message(
                "Blob",
                vec![field("leaf", 1, Label::Optional, Type::String)],
            ),
            message("Leaf", vec![field("v", 1, Label::Optional, Type::Int32)]),
        ],
    );

    // head (field 1, LEN) then tail (field 2, LEN), each holding
    // `Wrap { leaf: Leaf { v: 5 } }` == `0A 02 08 05`.
    let app = fixture_under(
        "pruned-tail",
        &fds,
        "test.Outer",
        &[
            0x0Au8, 0x04, 0x0A, 0x02, 0x08, 0x05, //
            0x12, 0x04, 0x0A, 0x02, 0x08, 0x05,
        ],
    );

    let root = app.first_node;
    let head = app.first_child(root).expect("Outer has children");
    let tail = app.next_sibling(head).expect("Outer has two children");
    let tail_leaf = app.first_child(tail).expect("tail wraps a Leaf");
    let tail_v = app.first_child(tail_leaf).expect("Leaf holds v");
    (app, head, tail, tail_leaf, tail_v)
}
