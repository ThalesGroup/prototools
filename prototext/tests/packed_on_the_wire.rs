// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0370 — a packed record renders as packed, whatever the field
//! declares, and every such record round-trips byte-exactly.
//!
//! The schemas are built inline rather than as `fixtures/schemas/*.proto`,
//! so that each test states the one declaration it is about (proto2
//! default, `[packed=false]`, `[packed=true]`, present-but-empty
//! `FieldOptions`) next to the bytes it feeds.

use prototext_core::schema::descriptor::Encode;
use prototext_core::schema::descriptor::{
    descriptor_proto::ExtensionRange,
    field_descriptor_proto::{Label, Type},
    DescriptorProto, EnumDescriptorProto, EnumValueDescriptorProto, FieldDescriptorProto,
    FieldOptions, FileDescriptorProto, FileDescriptorSet,
};
use prototext_core::{parse_schema, render_as_bytes, render_as_text, ParsedSchema, RenderOpts};

// ── Schema ────────────────────────────────────────────────────────────────────

/// How a field's `packed` option appears in the descriptor.
#[derive(Clone, Copy)]
enum Packing {
    /// No `FieldOptions` at all: the syntax's default applies.
    Default,
    /// `[packed = <bool>]`.
    Explicit(bool),
    /// `FieldOptions` present but carrying no `packed` — what `protoc`
    /// emits when some other option is set. prost-reflect then answers
    /// `is_packed() == false` even for proto3 (PROST-ISSUES.md §1).
    EmptyOptions,
}

fn field(
    name: &str,
    number: i32,
    label: Label,
    ty: Type,
    packing: Packing,
) -> FieldDescriptorProto {
    let options = match packing {
        Packing::Default => None,
        Packing::Explicit(p) => Some(FieldOptions {
            packed: Some(p),
            ..Default::default()
        }),
        Packing::EmptyOptions => Some(FieldOptions::default()),
    };
    FieldDescriptorProto {
        name: Some(name.into()),
        number: Some(number),
        label: Some(label as i32),
        r#type: Some(ty as i32),
        type_name: (ty == Type::Enum).then(|| ".p2.C".into()),
        options,
        ..Default::default()
    }
}

fn repeated(name: &str, number: i32, ty: Type, packing: Packing) -> FieldDescriptorProto {
    field(name, number, Label::Repeated, ty, packing)
}

/// The proto2 message body shared by `Unp` (every field declared
/// expanded) and `Pk` (every packable field declared `[packed=true]`).
/// Field numbers are the tags the tests below write by hand.
fn p2_message(name: &str, packing: Packing) -> DescriptorProto {
    DescriptorProto {
        name: Some(name.into()),
        field: vec![
            repeated("lane", 1, Type::Int32, packing),
            repeated("fx", 2, Type::Fixed32, packing),
            repeated("b", 3, Type::Bool, packing),
            repeated("u", 4, Type::Uint32, packing),
            repeated("c", 5, Type::Enum, packing),
            repeated("s", 6, Type::Sint32, packing),
            repeated("f", 7, Type::Float, packing),
            repeated("l", 8, Type::Int64, packing),
            repeated("names", 9, Type::String, Packing::Default),
        ],
        extension_range: vec![ExtensionRange {
            start: Some(100),
            end: Some(201),
        }],
        ..Default::default()
    }
}

fn extension(name: &str, extendee: &str, packing: Packing) -> FieldDescriptorProto {
    FieldDescriptorProto {
        extendee: Some(extendee.into()),
        ..repeated(name, 100, Type::Int32, packing)
    }
}

fn fds() -> Vec<u8> {
    let p2 = FileDescriptorProto {
        name: Some("p2.proto".into()),
        package: Some("p2".into()),
        syntax: Some("proto2".into()),
        enum_type: vec![EnumDescriptorProto {
            name: Some("C".into()),
            value: vec![
                EnumValueDescriptorProto {
                    name: Some("A".into()),
                    number: Some(0),
                },
                EnumValueDescriptorProto {
                    name: Some("B".into()),
                    number: Some(1),
                },
            ],
        }],
        message_type: vec![
            p2_message("Unp", Packing::Default),
            p2_message("Pk", Packing::Explicit(true)),
        ],
        extension: vec![
            extension("eunp", ".p2.Unp", Packing::Default),
            extension("epk", ".p2.Pk", Packing::Explicit(true)),
        ],
        ..Default::default()
    };
    let p3 = FileDescriptorProto {
        name: Some("p3.proto".into()),
        package: Some("p3".into()),
        syntax: Some("proto3".into()),
        message_type: vec![
            DescriptorProto {
                name: Some("Unp3".into()),
                field: vec![repeated("lane", 1, Type::Int32, Packing::Explicit(false))],
                ..Default::default()
            },
            DescriptorProto {
                name: Some("Opts3".into()),
                field: vec![repeated("lane", 1, Type::Int32, Packing::EmptyOptions)],
                ..Default::default()
            },
            DescriptorProto {
                name: Some("Str3".into()),
                field: vec![field(
                    "name",
                    1,
                    Label::Optional,
                    Type::String,
                    Packing::Default,
                )],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    FileDescriptorSet { file: vec![p2, p3] }.encode_to_vec()
}

fn schema(message: &str) -> ParsedSchema {
    parse_schema(&fds(), message).unwrap_or_else(|e| panic!("cannot parse {message}: {e}"))
}

// ── Round trip ────────────────────────────────────────────────────────────────

fn opts(assume_binary: bool) -> RenderOpts {
    RenderOpts {
        assume_binary,
        include_annotations: true,
        indent: 1,
        expand_any: true,
        hide_unknown_fields: false,
        expand_message_set: true,
    }
}

/// Render `wire` as `message`, assert it re-encodes to exactly `wire`, and
/// return the text for the caller's own assertions.
fn roundtrip(message: &str, wire: &[u8]) -> String {
    let schema = schema(message);
    let text = render_as_text(wire, schema.root_descriptor().as_ref(), opts(true)).unwrap();
    let back = render_as_bytes(&text, opts(false)).unwrap().into_owned();
    let text = String::from_utf8(text).unwrap();
    assert_eq!(
        back, wire,
        "{message}: {wire:02x?} must round-trip byte-exactly; rendered as:\n{text}"
    );
    text
}

/// The rendered lines after the `#@ prototext:` header.
fn body(text: &str) -> Vec<&str> {
    text.lines().skip(1).collect()
}

const DECLARATIONS: [&str; 2] = ["p2.Unp", "p2.Pk"];

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Tests 1 and 3: a packed record on a proto2 field declared expanded is a
/// packed run, not a wire-type mismatch.
#[test]
fn packed_record_on_proto2_unpacked_field_renders_packed() {
    let text = roundtrip("p2.Unp", &[0x0a, 0x03, 0x01, 0x02, 0x03]);
    assert_eq!(
        body(&text),
        [
            "lane: 1  #@ repeated int32 = 1; pack_size: 3; packing_mismatch",
            "lane: 2  #@ repeated int32 = 1",
            "lane: 3  #@ repeated int32 = 1",
        ],
    );
}

/// Tests 2 and 3: the same for proto3 with an explicit `[packed=false]`.
#[test]
fn packed_record_on_proto3_packed_false_field_renders_packed() {
    let text = roundtrip("p3.Unp3", &[0x0a, 0x03, 0x01, 0x02, 0x03]);
    assert!(!text.contains("TYPE_MISMATCH"), "{text}");
    assert_eq!(body(&text).len(), 3, "{text}");
    assert!(
        body(&text)[0].ends_with("pack_size: 3; packing_mismatch"),
        "the first element carries the record's pack_size: {text}"
    );
}

/// Test 4: prost-reflect reports `is_packed() == false` for this proto3
/// field, so the declaration renders without `[packed=true]`. Before spec
/// 0370 S2 the encoder keyed on that and wrote `08 01 08 02 08 03`.
#[test]
fn packed_record_with_prost_empty_options_roundtrips() {
    let text = roundtrip("p3.Opts3", &[0x0a, 0x03, 0x01, 0x02, 0x03]);
    assert!(text.contains("pack_size: 3"), "{text}");
}

/// Test 5: expanded, packed, expanded — in both declarations.
#[test]
fn mixed_encodings_of_one_field_roundtrip() {
    let wire = [0x08, 0x01, 0x0a, 0x02, 0x02, 0x03, 0x08, 0x04];
    for message in DECLARATIONS {
        let text = roundtrip(message, &wire);
        let values: Vec<&str> = body(&text)
            .iter()
            .map(|l| l.split("  #@").next().unwrap())
            .collect();
        assert_eq!(
            values,
            ["lane: 1", "lane: 2", "lane: 3", "lane: 4"],
            "{message}: {text}"
        );
    }
}

/// Test 6: a payload that is not a packed run is `INVALID_PACKED_RECORDS`
/// on a field declared expanded, as on one declared packed.
#[test]
fn invalid_packed_run_on_unpacked_field() {
    // A varint running past the payload; a fixed32 run of 3 bytes.
    for wire in [&[0x0a, 0x01, 0x80][..], &[0x12, 0x03, 0x01, 0x02, 0x03]] {
        let text = roundtrip("p2.Unp", wire);
        assert!(text.contains("#@ INVALID_PACKED_RECORDS"), "{text}");
        assert!(!text.contains("TYPE_MISMATCH"), "{text}");
    }
}

/// Test 7: strings cannot be packed; a LEN record on one is a string.
#[test]
fn len_on_repeated_string_unchanged() {
    let text = roundtrip("p2.Unp", b"\x4a\x03abc");
    assert_eq!(body(&text), [r#"names: "abc"  #@ repeated string = 9"#]);
}

/// Test 8: spec 0370 S5. The length prefix is overlong by one byte; the
/// payload is not a packed run. Before S5 `render_invalid` dropped
/// `len_ohb` and the prefix came back canonical.
#[test]
fn invalid_packed_run_keeps_overlong_length() {
    let wires: [&[u8]; 2] = [
        &[0x0a, 0x81, 0x00, 0x80],
        &[0x12, 0x83, 0x00, 0x01, 0x02, 0x03],
    ];
    for message in DECLARATIONS {
        for wire in wires {
            let text = roundtrip(message, wire);
            assert!(
                text.contains("INVALID_PACKED_RECORDS; len_ohb: 1"),
                "{message}: {text}"
            );
        }
    }
}

/// Test 9: spec 0370 S5, `INVALID_STRING`'s side of the same parameter.
#[test]
fn invalid_string_keeps_overlong_length() {
    let text = roundtrip("p3.Str3", &[0x0a, 0x81, 0x00, 0xff]);
    assert!(text.contains("INVALID_STRING; len_ohb: 1"), "{text}");
}

/// Test 10: `FieldOrExt::Ext::is_packed()` is always false, so before
/// spec 0370 no extension record took the packed path.
#[test]
fn packed_record_on_repeated_extension_roundtrips() {
    for message in DECLARATIONS {
        let text = roundtrip(message, &[0xa2, 0x06, 0x02, 0x01, 0x02]);
        assert!(!text.contains("TYPE_MISMATCH"), "{message}: {text}");
        let lines = body(&text);
        assert_eq!(lines.len(), 2, "{message}: {text}");
        assert!(lines[0].contains("pack_size: 2"), "{message}: {text}");
    }
}

/// Test 11 (G4): every per-element edge case round-trips on a field
/// declared expanded, rendering exactly as it does on one declared packed.
/// `expected` is a substring of the one rendered line.
#[test]
fn packed_element_edge_cases_roundtrip_on_unpacked_fields() {
    let cases: &[(&str, &[u8], &str)] = &[
        // Spec 0372: a bool above 1 is `true`, with its raw value kept.
        (
            "bool 2",
            &[0x1a, 0x01, 0x02],
            "b: true  #@ repeated bool = 3; pack_size: 1; packing_mismatch; bool_val: 2",
        ),
        (
            "bool ohb",
            &[0x1a, 0x02, 0x81, 0x00],
            "b: true  #@ repeated bool = 3; pack_size: 1; packing_mismatch; ohb: 1",
        ),
        (
            "int32 in the 32-bit gap",
            &[0x0a, 0x05, 0x80, 0x80, 0x80, 0x80, 0x20],
            "INVALID_PACKED_RECORDS",
        ),
        (
            "int32 5-byte negative",
            &[0x0a, 0x05, 0xff, 0xff, 0xff, 0xff, 0x0f],
            "lane: -1  #@ repeated int32 = 1; pack_size: 1; packing_mismatch; neg",
        ),
        (
            "int32 10-byte negative",
            &[
                0x0a, 0x0a, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01,
            ],
            "lane: -1  #@ repeated int32 = 1; pack_size: 1; packing_mismatch",
        ),
        (
            "uint32 over 32 bits",
            &[0x22, 0x05, 0x80, 0x80, 0x80, 0x80, 0x10],
            "INVALID_PACKED_RECORDS",
        ),
        (
            "sint32 over 32 bits",
            &[0x32, 0x05, 0x80, 0x80, 0x80, 0x80, 0x10],
            "INVALID_PACKED_RECORDS",
        ),
        (
            "enum undeclared",
            &[0x2a, 0x01, 0x05],
            "pack_size: 1; packing_mismatch; ENUM_UNKNOWN",
        ),
        (
            "enum 5-byte negative",
            &[0x2a, 0x05, 0xff, 0xff, 0xff, 0xff, 0x0f],
            "pack_size: 1; packing_mismatch; neg; ENUM_UNKNOWN",
        ),
        (
            "float NaN payload",
            &[0x3a, 0x04, 0x01, 0x00, 0xc0, 0x7f],
            "f: nan  #@ repeated float = 7; pack_size: 1; packing_mismatch; nan_bits: 0x7fc00001",
        ),
        (
            "int64 10-byte zero",
            &[
                0x42, 0x0b, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x00,
            ],
            "l: 0  #@ repeated int64 = 8; pack_size: 1; packing_mismatch; ohb: 10",
        ),
        (
            "int64 11-byte varint",
            &[
                0x42, 0x0b, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01,
            ],
            "INVALID_PACKED_RECORDS",
        ),
    ];
    for &(what, wire, expected) in cases {
        let text = roundtrip("p2.Unp", wire);
        let lines = body(&text);
        assert_eq!(lines.len(), 1, "{what}: {text}");
        assert!(
            lines[0].contains(expected),
            "{what}: expected {expected:?} in {text}"
        );
        assert!(!text.contains("TYPE_MISMATCH"), "{what}: {text}");
    }
}
