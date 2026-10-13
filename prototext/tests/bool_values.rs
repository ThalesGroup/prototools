// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0372 — a bool is any varint: it renders as its field, `true` for any
//! non-zero value, and a value other than 0 or 1 keeps its raw number in
//! `bool_val` so that the record round-trips byte-exactly.
//!
//! Also the renderer/scorer parity the spec establishes (G3): no value the
//! scorer accepts renders as `INVALID_PACKED_RECORDS` or `TYPE_MISMATCH`.

use prototext_core::schema::descriptor::Encode;
use prototext_core::schema::descriptor::{
    field_descriptor_proto::{Label, Type},
    DescriptorProto, EnumDescriptorProto, EnumValueDescriptorProto, FieldDescriptorProto,
    FileDescriptorProto, FileDescriptorSet,
};
use prototext_core::{parse_schema, render_as_bytes, render_as_text, ParsedSchema, RenderOpts};
use prototext_graph::build_scoring_graph::build_from_strings;
use prototext_graph::score::load::LoadedGraph;
use prototext_graph::score::{score_one, ScoringOpts};

// ── Schema ────────────────────────────────────────────────────────────────────

/// One field of the test message, as the descriptor and the scoring graph
/// each declare it.
struct Field {
    name: &'static str,
    number: i32,
    ty: Type,
    /// The scoring-graph type `reproto`'s `_scoring_kind` maps `ty` to.
    scoring: &'static str,
    /// `[min, max]` for a `Range` leaf (bool, closed enum).
    range: Option<(i32, i32)>,
}

const fn field(
    name: &'static str,
    number: i32,
    ty: Type,
    scoring: &'static str,
    range: Option<(i32, i32)>,
) -> Field {
    Field {
        name,
        number,
        ty,
        scoring,
        range,
    }
}

/// Every varint-encoded scalar kind, plus `float` for a fixed-width one.
/// `C` is a closed (proto2) enum with values 0 and 1; `one` is the only
/// singular field.
const FIELDS: [Field; 8] = [
    field("b", 1, Type::Bool, "bool", Some((0, 1))),
    field("i", 2, Type::Int32, "int32", None),
    field("u", 3, Type::Uint32, "uint32", None),
    field("c", 4, Type::Enum, "enum", Some((0, 1))),
    field("s", 5, Type::Sint32, "uint32", None),
    field("l", 6, Type::Int64, "uint64", None),
    field("f", 7, Type::Float, "float", None),
    field("one", 8, Type::Bool, "bool", Some((0, 1))),
];

fn schema() -> ParsedSchema {
    let descriptor = |f: Field| FieldDescriptorProto {
        name: Some(f.name.into()),
        number: Some(f.number),
        label: Some(if f.name == "one" {
            Label::Optional
        } else {
            Label::Repeated
        } as i32),
        r#type: Some(f.ty as i32),
        type_name: (f.ty == Type::Enum).then(|| ".p.C".into()),
        ..Default::default()
    };
    let value = |name: &str, number| EnumValueDescriptorProto {
        name: Some(name.into()),
        number: Some(number),
    };
    let file = FileDescriptorProto {
        name: Some("p.proto".into()),
        package: Some("p".into()),
        syntax: Some("proto2".into()),
        enum_type: vec![EnumDescriptorProto {
            name: Some("C".into()),
            value: vec![value("A", 0), value("B", 1)],
        }],
        message_type: vec![DescriptorProto {
            name: Some("P".into()),
            field: FIELDS.map(descriptor).to_vec(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] }.encode_to_vec();
    parse_schema(&fds, "p.P").expect("schema")
}

/// The same message as a scoring graph, built in memory.
fn graph() -> LoadedGraph {
    let mut yaml = String::from("entries:\n- p.P\nmessages:\n  p.P:\n    fields:\n");
    for f in FIELDS {
        yaml.push_str(&format!(
            "    - number: {}\n      type: {}\n",
            f.number, f.scoring
        ));
        if let Some((min, max)) = f.range {
            yaml.push_str(&format!("      range: [{min}, {max}]\n"));
        }
        let label = if f.name == "one" {
            "optional"
        } else {
            "repeated"
        };
        yaml.push_str(&format!("      label: {label}\n"));
    }
    let (bytes, _, _) = build_from_strings(&[yaml], false, false, |_, _| {}).expect("graph");
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

/// One expanded varint occurrence of field `number`.
fn expanded(number: u32, value: &[u8]) -> Vec<u8> {
    let mut out = varint(u64::from(number) << 3);
    out.extend_from_slice(value);
    out
}

/// One packed record of field `number` holding `payload`.
fn packed(number: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = varint(u64::from(number) << 3 | 2);
    out.extend(varint(payload.len() as u64));
    out.extend_from_slice(payload);
    out
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
    assert_eq!(
        back, wire,
        "{wire:02x?} must round-trip byte-exactly:\n{text}"
    );
    text.lines().skip(1).map(str::to_owned).collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Test 1: a bool above 1 renders as its field, `true`, with `bool_val` —
/// on a repeated and on a singular field. Before spec 0372 both were
/// `N: 2  #@ varint; TYPE_MISMATCH`.
#[test]
fn bool_above_one_renders_true_with_bool_val() {
    let s = schema();
    assert_eq!(
        roundtrip(&s, &[0x08, 0x02]),
        ["b: true  #@ repeated bool = 1; bool_val: 2"]
    );
    assert_eq!(
        roundtrip(&s, &[0x40, 0x02]),
        ["one: true  #@ bool = 8; bool_val: 2"]
    );
}

/// Test 2: the packed path. Before spec 0372 the whole record was
/// `INVALID_PACKED_RECORDS`.
#[test]
fn packed_bool_above_one_renders_true_with_bool_val() {
    assert_eq!(
        roundtrip(&schema(), &[0x0a, 0x03, 0x00, 0x01, 0x02]),
        [
            "b: false  #@ repeated bool = 1; pack_size: 3; packing_mismatch",
            "b: true  #@ repeated bool = 1",
            "b: true  #@ repeated bool = 1; bool_val: 2",
        ]
    );
}

/// Test 3: overhang applies to the raw value, not to the canonical 1.
#[test]
fn bool_val_with_overhang_roundtrips() {
    let s = schema();
    assert_eq!(
        roundtrip(&s, &[0x08, 0x82, 0x00]),
        ["b: true  #@ repeated bool = 1; val_ohb: 1; bool_val: 2"]
    );
    assert_eq!(
        roundtrip(&s, &[0x0a, 0x02, 0x82, 0x00]),
        ["b: true  #@ repeated bool = 1; pack_size: 1; packing_mismatch; ohb: 1; bool_val: 2"]
    );
}

/// Test 4: the extremes, including a value in the 32-bit gap, which the
/// scorer vetoes (spec 0372 N1) but a parser reads as `true`.
#[test]
fn bool_large_values_roundtrip() {
    let s = schema();
    for v in [0xFFFF_FFFF, 1 << 33, u64::MAX] {
        let want = format!("bool_val: {v}");
        let lines = roundtrip(&s, &expanded(1, &varint(v)));
        assert!(
            lines.len() == 1 && lines[0].starts_with("b: true") && lines[0].ends_with(&want),
            "{v}: {lines:?}"
        );
        let lines = roundtrip(&s, &packed(1, &varint(v)));
        assert!(
            lines.len() == 1 && lines[0].starts_with("b: true") && lines[0].ends_with(&want),
            "{v} packed: {lines:?}"
        );
    }
}

/// Test 5: no-change guard. Canonical bools carry no `bool_val`, before and
/// after spec 0372.
#[test]
fn canonical_bools_unchanged() {
    let s = schema();
    assert_eq!(
        roundtrip(&s, &[0x08, 0x00, 0x08, 0x01]),
        [
            "b: false  #@ repeated bool = 1",
            "b: true  #@ repeated bool = 1"
        ]
    );
    assert_eq!(
        roundtrip(&s, &[0x0a, 0x02, 0x00, 0x01]),
        [
            "b: false  #@ repeated bool = 1; pack_size: 2; packing_mismatch",
            "b: true  #@ repeated bool = 1"
        ]
    );
}

/// Test 6 (G3): whenever the scorer accepts a value, the renderer shows it
/// as a value of its field — never `INVALID_PACKED_RECORDS`, never
/// `TYPE_MISMATCH` — on the expanded and the packed path alike. Every
/// case must also round-trip, whatever it renders as.
#[test]
fn renderer_accepts_whatever_the_scorer_accepts() {
    let s = schema();
    let loaded = graph();
    let values: [u64; 11] = [
        0,
        1,
        2,
        5,
        0x7FFF_FFFF,
        0x8000_0000,
        0xFFFF_FFFF,
        1 << 32,
        0xFFFF_FFFF_7FFF_FFFF,
        0xFFFF_FFFF_8000_0000,
        u64::MAX,
    ];
    let mut cases: Vec<(String, Vec<u8>)> = Vec::new();
    for f in FIELDS {
        let (name, number) = (f.name, f.number as u32);
        if f.ty == Type::Float {
            // Fixed-width: a packed run of 3, 4 or 8 bytes.
            for len in [3, 4, 8] {
                cases.push((
                    format!("{name} packed {len} bytes"),
                    packed(number, &vec![0; len]),
                ));
            }
            continue;
        }
        for v in values {
            cases.push((format!("{name} = {v:#x}"), expanded(number, &varint(v))));
            if name != "one" {
                cases.push((
                    format!("{name} = {v:#x} packed"),
                    packed(number, &varint(v)),
                ));
            }
        }
    }

    let mut accepted = 0;
    for (what, wire) in cases {
        let lines = roundtrip(&s, &wire);
        let scored = score_one(&wire, "p.P", loaded.graph(), &ScoringOpts::default())
            .expect("p.P is a root");
        if scored.vetoed {
            continue;
        }
        accepted += 1;
        for line in &lines {
            assert!(
                !line.contains("INVALID_PACKED_RECORDS") && !line.contains("TYPE_MISMATCH"),
                "{what}: the scorer accepts it, the renderer does not: {line}"
            );
        }
    }
    assert!(
        accepted > 50,
        "only {accepted} cases were accepted by the scorer"
    );
}
