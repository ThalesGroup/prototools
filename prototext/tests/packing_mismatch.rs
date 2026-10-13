// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0371 — a record whose packing contradicts its declaration.
//!
//! The renderer marks it `packing_mismatch`, the scorer charges it `packing`,
//! and the two must agree record for record (G3). Test plan items 2, 5 and 6.

use prototext_core::schema::descriptor::Encode;
use prototext_core::schema::descriptor::{
    descriptor_proto::ExtensionRange,
    field_descriptor_proto::{Label, Type},
    DescriptorProto, FieldDescriptorProto, FieldOptions, FileDescriptorProto, FileDescriptorSet,
};
use prototext_core::{parse_schema, render_as_bytes, render_as_text, ParsedSchema, RenderOpts};
use prototext_graph::build_scoring_graph::build_from_strings;
use prototext_graph::score::load::LoadedGraph;
use prototext_graph::score::{score_one, ScoringOpts};

// ── Schema ────────────────────────────────────────────────────────────────────

/// How a field's `packed` option appears in its descriptor.
#[derive(Clone, Copy)]
enum Packing {
    /// No `FieldOptions`: the syntax's default applies.
    Default,
    /// `[packed = <bool>]`.
    Explicit(bool),
    /// `FieldOptions` present but without `packed` — prost-reflect's
    /// `is_packed()` then answers false even in proto3 (PROST-ISSUES §1).
    OtherOption,
}

fn field(name: &str, number: i32, ty: Type, packing: Packing) -> FieldDescriptorProto {
    let options = match packing {
        Packing::Default => None,
        Packing::Explicit(p) => Some(FieldOptions {
            packed: Some(p),
            ..Default::default()
        }),
        Packing::OtherOption => Some(FieldOptions {
            deprecated: Some(false),
            ..Default::default()
        }),
    };
    FieldDescriptorProto {
        name: Some(name.into()),
        number: Some(number),
        label: Some(Label::Repeated as i32),
        r#type: Some(ty as i32),
        options,
        ..Default::default()
    }
}

/// proto2 `p2.M` and proto3 `p3.M`, whose fields cover the declaration
/// matrix, and two proto2 extensions of `p2.M`.
fn schema(message: &str) -> ParsedSchema {
    let p2 = FileDescriptorProto {
        name: Some("p2.proto".into()),
        package: Some("p2".into()),
        syntax: Some("proto2".into()),
        message_type: vec![DescriptorProto {
            name: Some("M".into()),
            field: vec![
                field("u", 1, Type::Uint64, Packing::Default),
                field("k", 2, Type::Uint64, Packing::Explicit(true)),
                field("b", 3, Type::Bool, Packing::Default),
                field("f", 4, Type::Fixed32, Packing::Explicit(true)),
            ],
            extension_range: vec![ExtensionRange {
                start: Some(100),
                end: Some(201),
            }],
            ..Default::default()
        }],
        extension: vec![
            FieldDescriptorProto {
                extendee: Some(".p2.M".into()),
                ..field("eu", 100, Type::Int32, Packing::Default)
            },
            FieldDescriptorProto {
                extendee: Some(".p2.M".into()),
                ..field("ek", 101, Type::Int32, Packing::Explicit(true))
            },
        ],
        ..Default::default()
    };
    let p3 = FileDescriptorProto {
        name: Some("p3.proto".into()),
        package: Some("p3".into()),
        syntax: Some("proto3".into()),
        message_type: vec![DescriptorProto {
            name: Some("M".into()),
            field: vec![
                field("d", 1, Type::Int32, Packing::Default),
                field("x", 2, Type::Int32, Packing::Explicit(false)),
                field("o", 3, Type::Int32, Packing::OtherOption),
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![p2, p3] }.encode_to_vec();
    parse_schema(&fds, message).unwrap_or_else(|e| panic!("{message}: {e}"))
}

/// `p2.M` as a scoring graph declaring the same packing, as `reproto`
/// would emit it (spec 0371 S2).
fn graph() -> LoadedGraph {
    let yaml = "entries:\n- p2.M\nmessages:\n  p2.M:\n    fields:\n\
        \x20   - number: 1\n      type: uint64\n      label: repeated\n\
        \x20   - number: 2\n      type: uint64\n      label: repeated\n      packed: true\n\
        \x20   - number: 3\n      type: bool\n      range: [0, 1]\n      label: repeated\n\
        \x20   - number: 4\n      type: float\n      label: repeated\n      packed: true\n";
    let (bytes, _, _) =
        build_from_strings(&[yaml.to_string()], false, false, |_, _| {}).expect("graph");
    LoadedGraph::from_static_bytes(Box::leak(bytes.into_boxed_slice())).expect("load")
}

// ── Wire and round trip ───────────────────────────────────────────────────────

fn varint(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
    out
}

fn expanded(number: u32, values: &[u64]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|&v| [varint(u64::from(number) << 3), varint(v)].concat())
        .collect()
}

fn packed(number: u32, values: &[u64]) -> Vec<u8> {
    let payload: Vec<u8> = values.iter().flat_map(|&v| varint(v)).collect();
    [
        varint(u64::from(number) << 3 | 2),
        varint(payload.len() as u64),
        payload,
    ]
    .concat()
}

fn fixed32(number: u32, v: u32) -> Vec<u8> {
    [varint(u64::from(number) << 3 | 5), v.to_le_bytes().to_vec()].concat()
}

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

/// Render `wire`, assert it re-encodes to exactly `wire`, and return the
/// lines after the header.
fn roundtrip(schema: &ParsedSchema, wire: &[u8]) -> Vec<String> {
    let text = render_as_text(wire, schema.root_descriptor().as_ref(), opts(true)).unwrap();
    let back = render_as_bytes(&text, opts(false)).unwrap().into_owned();
    let text = String::from_utf8(text).unwrap();
    assert_eq!(back, wire, "{wire:02x?} must round-trip:\n{text}");
    text.lines().skip(1).map(str::to_owned).collect()
}

fn marks(lines: &[String]) -> usize {
    lines
        .iter()
        .filter(|l| l.contains("packing_mismatch"))
        .count()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Test 2: the renderer's declaration rule, read through the one place it
/// shows — `[packed=true]` in the field declaration — over the matrix,
/// extensions and prost-reflect's §1 case included.
#[test]
fn declared_packing_follows_the_syntax_rule() {
    let declared = |message: &str, wire: Vec<u8>| -> bool {
        let lines = roundtrip(&schema(message), &wire);
        lines[0].contains("[packed=true]")
    };
    // proto2: packed only when it says so.
    assert!(!declared("p2.M", expanded(1, &[1])), "proto2 default");
    assert!(declared("p2.M", expanded(2, &[1])), "proto2 [packed=true]");
    // proto3: packed unless it says otherwise — whatever else it carries.
    assert!(declared("p3.M", expanded(1, &[1])), "proto3 default");
    assert!(
        !declared("p3.M", expanded(2, &[1])),
        "proto3 [packed=false]"
    );
    assert!(
        declared("p3.M", expanded(3, &[1])),
        "proto3 with another option"
    );
    // Extensions, which the wrapper used to call unpacked unconditionally.
    assert!(!declared("p2.M", expanded(100, &[1])), "extension, default");
    assert!(
        declared("p2.M", expanded(101, &[1])),
        "extension, [packed=true]"
    );
}

/// Test 5: where the mark goes, and that every marked record round-trips.
#[test]
fn packing_mismatch_marks_the_contradicting_records() {
    let s = schema("p2.M");
    // Packed on an expanded declaration: the record's first line only.
    let lines = roundtrip(&s, &packed(1, &[1, 2, 3]));
    assert_eq!(
        lines[0],
        "u: 1  #@ repeated uint64 = 1; pack_size: 3; packing_mismatch"
    );
    assert_eq!(marks(&lines), 1);
    // Expanded on a packed declaration: every occurrence.
    let lines = roundtrip(&s, &expanded(2, &[1, 2, 3]));
    assert_eq!(marks(&lines), 3, "{lines:?}");
    assert_eq!(
        lines[0],
        "k: 1  #@ repeated uint64 [packed=true] = 2; packing_mismatch"
    );
    // Fixed-width, expanded on a packed declaration.
    let lines = roundtrip(&s, &fixed32(4, 7));
    assert_eq!(
        lines,
        ["f: 7  #@ repeated fixed32 [packed=true] = 4; packing_mismatch"]
    );
    // An empty packed record on an expanded declaration.
    let lines = roundtrip(&s, &packed(1, &[]));
    assert_eq!(
        lines,
        ["#@ repeated uint64 = 1; pack_size: 0; packing_mismatch"]
    );
    // Extensions, both directions.
    assert_eq!(marks(&roundtrip(&s, &packed(100, &[1, 2]))), 1);
    assert_eq!(marks(&roundtrip(&s, &expanded(101, &[1, 2]))), 2);
    // Agreement is never marked.
    assert_eq!(marks(&roundtrip(&s, &expanded(1, &[1, 2]))), 0);
    assert_eq!(marks(&roundtrip(&s, &packed(2, &[1, 2]))), 0);
    assert_eq!(marks(&roundtrip(&s, &packed(101, &[1, 2]))), 0);
}

/// Test 6 (G3): the rows and the score agree record for record — every case
/// the scorer does not veto has exactly as many `packing_mismatch` rows as
/// `EntryScore.packing`.
#[test]
fn rows_and_score_agree_on_packing() {
    let s = schema("p2.M");
    let g = graph();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("packed on expanded", packed(1, &[1, 2, 3])),
        ("expanded on expanded", expanded(1, &[1, 2, 3])),
        ("packed on packed", packed(2, &[1, 2, 3])),
        ("expanded on packed", expanded(2, &[1, 2, 3])),
        ("empty packed on expanded", packed(1, &[])),
        ("packed bool 2 on expanded", packed(3, &[0, 1, 2])),
        ("fixed32 expanded on packed", fixed32(4, 7)),
        (
            "both directions at once",
            [packed(1, &[5]), expanded(2, &[5, 6])].concat(),
        ),
    ];
    for (what, wire) in cases {
        let lines = roundtrip(&s, &wire);
        let scored =
            score_one(&wire, "p2.M", g.graph(), &ScoringOpts::default()).expect("p2.M is a root");
        assert!(!scored.vetoed, "{what}: the scorer must accept it");
        assert_eq!(
            marks(&lines) as u64,
            scored.packing,
            "{what}: rows {lines:?} against packing {}",
            scored.packing
        );
    }
}
