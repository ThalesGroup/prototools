// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Regression tests for the scoring walk (spec 0042).
//!
//! The test schema (proto2):
//!
//! ```text
//! enum Status { OK = 0; WARN = 1; ERR = 2; }
//!
//! message Inner {
//!   optional uint32 value = 1;   // VARINT optional
//! }
//!
//! message Outer {
//!   required uint32 id     = 1;  // VARINT required
//!   optional string name   = 2;  // LEN_STRING optional
//!   repeated uint32 tags   = 3;  // VARINT repeated
//!   optional Inner  child  = 4;  // LEN_MSG → Inner optional
//!   optional Status status = 5;  // ENUM [0..2] optional
//! }
//! ```
//!
//! The graph is built programmatically through the same `graph::build` /
//! `graph::compile` / `serial::write` / `score::load` pipeline used in
//! production, so the tests exercise the full round-trip.

use crate::build_scoring_graph::{
    graph, hopcroft,
    load::{FieldLabel, Merged, ScoringField, ScoringKind},
    serial,
};
use crate::score::{load as score_load, walk};

// ── Schema fixture ────────────────────────────────────────────────────────────

fn make_merged() -> Merged {
    let inner_fields = vec![ScoringField {
        number: 1,
        kind: ScoringKind::Varint,
        child: None,
        enum_range: None,
        label: FieldLabel::Optional,
    }];

    let outer_fields = vec![
        ScoringField {
            number: 1,
            kind: ScoringKind::Varint,
            child: None,
            enum_range: None,
            label: FieldLabel::Required,
        },
        ScoringField {
            number: 2,
            kind: ScoringKind::LenString,
            child: None,
            enum_range: None,
            label: FieldLabel::Optional,
        },
        ScoringField {
            number: 3,
            kind: ScoringKind::Varint,
            child: None,
            enum_range: None,
            label: FieldLabel::Repeated,
        },
        ScoringField {
            number: 4,
            kind: ScoringKind::LenMsg,
            child: Some("Inner".to_string()),
            enum_range: None,
            label: FieldLabel::Optional,
        },
        ScoringField {
            number: 5,
            kind: ScoringKind::Enum,
            child: None,
            enum_range: Some((0, 2)),
            label: FieldLabel::Optional,
        },
    ];

    let mut states = std::collections::HashMap::new();
    states.insert("Inner".to_string(), inner_fields);
    states.insert("Outer".to_string(), outer_fields);

    Merged {
        states,
        roots: vec!["Outer".to_string()],
    }
}

/// Build a graph binary in a tempdir and load it back.
fn build_graph() -> score_load::LoadedGraph {
    let merged = make_merged();
    let (raw, reg) = graph::build(&merged);
    let partition = hopcroft::minimize(&raw, &reg);
    let compiled = graph::compile(&raw, &reg, &partition, &merged);

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("test.bin");
    serial::write(&compiled, &path).expect("write graph");

    // Keep tempdir alive by leaking it — fine for tests.
    let _ = std::mem::ManuallyDrop::new(dir);
    score_load::load_graph(&path).expect("load graph")
}

fn root_state(graph: &score_load::LoadedGraph) -> u32 {
    graph
        .roots
        .iter()
        .find(|r| r.fqdn.as_str() == "Outer")
        .expect("Outer root")
        .state_id
        .to_native()
}

// ── Wire encoding helpers ─────────────────────────────────────────────────────

fn varint(v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    let mut v = v;
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            break;
        }
        out.push(b | 0x80);
    }
    out
}

fn tag(field: u32, wire_type: u8) -> Vec<u8> {
    varint(((field as u64) << 3) | wire_type as u64)
}

fn field_varint(field: u32, v: u64) -> Vec<u8> {
    let mut b = tag(field, 0);
    b.extend(varint(v));
    b
}

fn field_len(field: u32, payload: &[u8]) -> Vec<u8> {
    let mut b = tag(field, 2);
    b.extend(varint(payload.len() as u64));
    b.extend_from_slice(payload);
    b
}

fn field_fixed32(field: u32, v: u32) -> Vec<u8> {
    let mut b = tag(field, 5);
    b.extend_from_slice(&v.to_le_bytes());
    b
}

fn field_fixed64(field: u32, v: u64) -> Vec<u8> {
    let mut b = tag(field, 1);
    b.extend_from_slice(&v.to_le_bytes());
    b
}

/// A varint with `ohb` non-canonical overhang bytes appended.
fn varint_ohb(v: u64, ohb: u8) -> Vec<u8> {
    let mut out = varint(v);
    if ohb > 0 {
        // Remove final terminator, re-add as continuation, then pad, then 0x00.
        let last = out.pop().unwrap();
        out.push(last | 0x80);
        for _ in 1..ohb {
            out.push(0x80);
        }
        out.push(0x00);
    }
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// TC-01: Perfect match — all fields present, canonical.
#[test]
fn tc01_perfect_match() {
    let g = build_graph();
    let root = root_state(&g);

    // Outer { id=1, name="hi", tags=7, child=Inner{value=42}, status=1 }
    let inner = field_varint(1, 42);
    let mut pb = Vec::new();
    pb.extend(field_varint(1, 1)); // id (required)
    pb.extend(field_len(2, b"hi")); // name (optional string)
    pb.extend(field_varint(3, 7)); // tags (repeated)
    pb.extend(field_len(4, &inner)); // child (optional message)
    pb.extend(field_varint(5, 1)); // status = WARN (enum 0..2)

    let s = walk::score(&pb, root, &g);
    // matches: id + name + tags + child-field + inner.value + status = 6
    // (child-field counts as 1 match at the Outer level; inner.value counts
    // as 1 more match inside the Inner recursion)
    assert!(!s.vetoed, "should not veto");
    assert_eq!(s.unknowns, 0);
    assert_eq!(s.matches, 6);
    assert_eq!(s.non_canonical, 0);
}

/// TC-02: All-unknown fields (field numbers not in schema).
#[test]
fn tc02_all_unknown() {
    let g = build_graph();
    let root = root_state(&g);

    let mut pb = Vec::new();
    pb.extend(field_varint(10, 1)); // unknown VARINT
    pb.extend(field_varint(11, 2)); // unknown VARINT
    pb.extend(field_len(12, b"x")); // unknown LEN

    let s = walk::score(&pb, root, &g);
    assert!(!s.vetoed);
    assert_eq!(s.unknowns, 3);
    assert_eq!(s.matches, 0);
}

/// TC-03: Wrong wire type on a known field number → veto.
///
/// The walk resolves by field number first: if the field number is declared
/// in the schema but the wire type does not match any of its transitions,
/// that is a type mismatch and must veto.
#[test]
fn tc03_wrong_wire_type_veto() {
    let g = build_graph();
    let root = root_state(&g);

    // field 1 (id, VARINT) sent as LEN — wire-type mismatch on known field
    let pb = field_len(1, b"oops");

    let s = walk::score(&pb, root, &g);
    assert!(s.vetoed, "wrong wire type on known field should veto");
}

/// TC-04: Invalid UTF-8 on a string field → veto.
#[test]
fn tc04_invalid_utf8_veto() {
    let g = build_graph();
    let root = root_state(&g);

    let mut pb = Vec::new();
    pb.extend(field_varint(1, 1)); // id ok
    pb.extend(field_len(2, b"\xff\xfe invalid")); // name: invalid UTF-8

    let s = walk::score(&pb, root, &g);
    assert!(s.vetoed, "invalid UTF-8 on string field should veto");
}

/// TC-05: Enum value out of range [0..2] → veto.
#[test]
fn tc05_enum_out_of_range_veto() {
    let g = build_graph();
    let root = root_state(&g);

    let mut pb = Vec::new();
    pb.extend(field_varint(1, 1)); // id
    pb.extend(field_varint(5, 99)); // status=99, outside [0..2]

    let s = walk::score(&pb, root, &g);
    assert!(s.vetoed, "enum value 99 outside [0..2] should veto");
}

/// TC-06: Truncated FIXED32 → veto.
#[test]
fn tc06_truncated_fixed32_veto() {
    let g = build_graph();
    let root = root_state(&g);

    // Send a raw field tag for wire type 5 (I32) on an unknown field,
    // then only 2 bytes instead of 4.
    let mut pb = tag(20, 5); // unknown I32 field
    pb.extend_from_slice(&[0x01, 0x02]); // only 2 bytes

    let s = walk::score(&pb, root, &g);
    assert!(s.vetoed, "truncated fixed32 should veto");
}

/// TC-07: Truncated FIXED64 → veto.
#[test]
fn tc07_truncated_fixed64_veto() {
    let g = build_graph();
    let root = root_state(&g);

    let mut pb = tag(20, 1); // unknown I64 field
    pb.extend_from_slice(&[0x01, 0x02, 0x03]); // only 3 bytes

    let s = walk::score(&pb, root, &g);
    assert!(s.vetoed, "truncated fixed64 should veto");
}

/// TC-08: Truncated LEN payload → veto.
#[test]
fn tc08_truncated_len_payload_veto() {
    let g = build_graph();
    let root = root_state(&g);

    let mut pb = tag(2, 2); // name field, LEN
    pb.extend(varint(100)); // claims 100-byte payload
    pb.extend_from_slice(b"short"); // only 5 bytes

    let s = walk::score(&pb, root, &g);
    assert!(s.vetoed, "truncated LEN payload should veto");
}

/// TC-09: Invalid wire type (6) → veto.
#[test]
fn tc09_invalid_wire_type_veto() {
    let g = build_graph();
    let root = root_state(&g);

    let pb = vec![0x06]; // field=0, wire_type=6 — invalid

    let s = walk::score(&pb, root, &g);
    assert!(s.vetoed, "wire type 6 should veto");
}

/// TC-10: Non-canonical varint overhang on tag → non_canonical incremented.
#[test]
fn tc10_tag_overhang_non_canonical() {
    let g = build_graph();
    let root = root_state(&g);

    // Tag for field 1 (id), wire type 0, with 1 overhang byte on the tag.
    let mut pb = varint_ohb(1 << 3 | 0, 1); // tag with ohb=1
    pb.extend(varint(42)); // id value

    let s = walk::score(&pb, root, &g);
    assert!(!s.vetoed);
    assert_eq!(s.non_canonical, 1);
    assert_eq!(s.matches, 1);
}

/// TC-11: Non-canonical varint overhang on value → non_canonical incremented.
#[test]
fn tc11_value_overhang_non_canonical() {
    let g = build_graph();
    let root = root_state(&g);

    // field 1 (id), canonical tag, non-canonical value
    let mut pb = tag(1, 0);
    pb.extend(varint_ohb(1, 2)); // value 1 with 2 overhang bytes

    let s = walk::score(&pb, root, &g);
    assert!(!s.vetoed);
    assert_eq!(s.non_canonical, 1);
    assert_eq!(s.matches, 1);
}

/// TC-12: Non-canonical LEN length-prefix overhang → non_canonical incremented.
#[test]
fn tc12_len_prefix_overhang_non_canonical() {
    let g = build_graph();
    let root = root_state(&g);

    // field 2 (name), canonical tag, non-canonical length prefix
    let mut pb = tag(2, 2);
    pb.extend(varint_ohb(2, 1)); // length=2 with 1 overhang byte
    pb.extend_from_slice(b"hi");

    let s = walk::score(&pb, root, &g);
    assert!(!s.vetoed);
    assert_eq!(s.non_canonical, 1);
    assert_eq!(s.matches, 1);
}

/// TC-13: Recursion into sub-message — matches accumulate across both levels.
#[test]
fn tc13_submessage_recursion() {
    let g = build_graph();
    let root = root_state(&g);

    // Outer { id=1, child=Inner{value=99} }
    let inner = field_varint(1, 99);
    let mut pb = Vec::new();
    pb.extend(field_varint(1, 1)); // id (required) → match
    pb.extend(field_len(4, &inner)); // child → match + recurse: value → match

    let s = walk::score(&pb, root, &g);
    assert!(!s.vetoed);
    assert_eq!(s.matches, 3); // id + child-field + inner.value
    assert_eq!(s.unknowns, 0);
}

/// TC-14: Mix of known and unknown fields.
#[test]
fn tc14_mixed_known_unknown() {
    let g = build_graph();
    let root = root_state(&g);

    let mut pb = Vec::new();
    pb.extend(field_varint(1, 5)); // id → match
    pb.extend(field_varint(99, 42)); // unknown → unknown
    pb.extend(field_len(2, b"hello")); // name → match
    pb.extend(field_fixed32(88, 0)); // unknown I32 → unknown

    let s = walk::score(&pb, root, &g);
    assert!(!s.vetoed);
    assert_eq!(s.matches, 2);
    assert_eq!(s.unknowns, 2);
}

/// TC-15: Unknown field with invalid body still vetoes.
/// A truncated FIXED64 in an unknown field must still veto —
/// the walk consumes bodies of unknown fields and validates them.
#[test]
fn tc15_unknown_field_truncated_body_veto() {
    let g = build_graph();
    let root = root_state(&g);

    let mut pb = field_varint(1, 1); // id → match
    pb.extend(tag(99, 1)); // unknown I64 field
    pb.extend_from_slice(&[0x00, 0x01]); // only 2 bytes of the required 8

    let s = walk::score(&pb, root, &g);
    assert!(s.vetoed, "truncated body on unknown I64 field should veto");
}

/// TC-16: FIXED32 and FIXED64 on known fields — match, no veto.
#[test]
fn tc16_fixed_fields_known() {
    // Build a schema with an I32 and I64 field.
    let merged = Merged {
        states: {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "M".to_string(),
                vec![
                    ScoringField {
                        number: 1,
                        kind: ScoringKind::I32,
                        child: None,
                        enum_range: None,
                        label: FieldLabel::Optional,
                    },
                    ScoringField {
                        number: 2,
                        kind: ScoringKind::I64,
                        child: None,
                        enum_range: None,
                        label: FieldLabel::Optional,
                    },
                ],
            );
            m
        },
        roots: vec!["M".to_string()],
    };
    let (raw, reg) = graph::build(&merged);
    let partition = hopcroft::minimize(&raw, &reg);
    let compiled = graph::compile(&raw, &reg, &partition, &merged);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.bin");
    serial::write(&compiled, &path).unwrap();
    let g = score_load::load_graph(&path).unwrap();
    let root = g
        .roots
        .iter()
        .find(|r| r.fqdn.as_str() == "M")
        .unwrap()
        .state_id
        .to_native();

    let mut pb = field_fixed32(1, 0xDEAD_BEEF);
    pb.extend(field_fixed64(2, 0x0102_0304_0506_0708));

    let s = walk::score(&pb, root, &g);
    assert!(!s.vetoed);
    assert_eq!(s.matches, 2);
    assert_eq!(s.unknowns, 0);
}

/// TC-17: Empty message — no fields → no matches, no unknowns, not vetoed.
#[test]
fn tc17_empty_message() {
    let g = build_graph();
    let root = root_state(&g);
    let s = walk::score(&[], root, &g);
    assert!(!s.vetoed);
    assert_eq!(s.matches, 0);
    assert_eq!(s.unknowns, 0);
}

/// TC-18: END_GROUP outside a group → veto.
#[test]
fn tc18_end_group_outside_group_veto() {
    let g = build_graph();
    let root = root_state(&g);

    let pb = tag(1, 4); // wire type 4 = END_GROUP, not inside a group

    let s = walk::score(&pb, root, &g);
    assert!(s.vetoed, "END_GROUP outside group should veto");
}

/// TC-19: Multiple occurrences of a repeated field — all count as matches.
#[test]
fn tc19_repeated_field_multiple_occurrences() {
    let g = build_graph();
    let root = root_state(&g);

    let mut pb = field_varint(1, 1); // id
    pb.extend(field_varint(3, 10)); // tags[0]
    pb.extend(field_varint(3, 20)); // tags[1]
    pb.extend(field_varint(3, 30)); // tags[2]

    let s = walk::score(&pb, root, &g);
    assert!(!s.vetoed);
    assert_eq!(s.matches, 4); // id + 3× tags
    assert_eq!(s.unknowns, 0);
}

// ── Multi-entry tests (spec 0048 §7) ──────────────────────────────────────────

/// Build a two-entry graph: Outer (full schema) + Inner (one field).
/// Used by MT-01..MT-04.
fn build_two_entry_graph() -> score_load::LoadedGraph {
    let inner_fields = vec![ScoringField {
        number: 1,
        kind: ScoringKind::Varint,
        child: None,
        enum_range: None,
        label: FieldLabel::Optional,
    }];

    let outer_fields = vec![
        ScoringField {
            number: 1,
            kind: ScoringKind::Varint,
            child: None,
            enum_range: None,
            label: FieldLabel::Required,
        },
        ScoringField {
            number: 2,
            kind: ScoringKind::LenString,
            child: None,
            enum_range: None,
            label: FieldLabel::Optional,
        },
        ScoringField {
            number: 4,
            kind: ScoringKind::LenMsg,
            child: Some("Inner".to_string()),
            enum_range: None,
            label: FieldLabel::Optional,
        },
        ScoringField {
            number: 5,
            kind: ScoringKind::Enum,
            child: None,
            enum_range: Some((0, 2)),
            label: FieldLabel::Optional,
        },
    ];

    let mut states = std::collections::HashMap::new();
    states.insert("Inner".to_string(), inner_fields);
    states.insert("Outer".to_string(), outer_fields);

    let merged = crate::build_scoring_graph::load::Merged {
        states,
        roots: vec!["Outer".to_string(), "Inner".to_string()],
    };

    let (raw, reg) = graph::build(&merged);
    let partition = hopcroft::minimize(&raw, &reg);
    let compiled = graph::compile(&raw, &reg, &partition, &merged);
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("two.bin");
    serial::write(&compiled, &path).expect("write");
    let _ = std::mem::ManuallyDrop::new(dir);
    score_load::load_graph(&path).expect("load")
}

fn entry_score<'a>(results: &'a [walk::EntryScore], fqdn: &str) -> &'a walk::EntryScore {
    results
        .iter()
        .find(|r| r.fqdn == fqdn)
        .unwrap_or_else(|| panic!("entry '{fqdn}' not found in results"))
}

/// MT-01: Two entries with the same root state (after Hopcroft deduplication)
/// both receive the same match/unknown counts from one walk.
///
/// Inner has one field (field 1, varint).  A wire message containing only
/// field 1 is valid for both Outer (field 1 = id) and Inner (field 1 = value).
/// After Hopcroft, if they deduplicate they share a state; if not, both still
/// walk correctly and agree on the result.
#[test]
fn mt01_shared_root_state_both_scored() {
    let g = build_two_entry_graph();
    // A message with only field 1 (varint).
    let pb = field_varint(1, 42);
    let results = walk::score_all(&pb, &g);

    let outer = entry_score(&results, "Outer");
    let inner = entry_score(&results, "Inner");

    // Both declare field 1 as VARINT → match for both.
    assert!(!outer.vetoed, "Outer should not be vetoed");
    assert!(!inner.vetoed, "Inner should not be vetoed");
    assert_eq!(outer.matches, 1, "Outer: field 1 is a match");
    assert_eq!(inner.matches, 1, "Inner: field 1 is a match");
    assert_eq!(outer.unknowns, 0);
    assert_eq!(inner.unknowns, 0);
}

/// MT-02: Two entries with different root states; a wire-type mismatch on a
/// field declared only by Outer vetoes Outer but leaves Inner unaffected.
///
/// Outer declares field 2 as LEN_STRING.  Inner does not declare field 2.
/// Send field 2 as VARINT (wire type 0) — this is a mismatch for Outer
/// (expected WT_LEN) and an unknown for Inner.
#[test]
fn mt02_mismatch_vetoes_one_entry_only() {
    let g = build_two_entry_graph();
    // field 2 sent as VARINT — Outer expects LEN, Inner has no field 2.
    let pb = field_varint(2, 99);
    let results = walk::score_all(&pb, &g);

    let outer = entry_score(&results, "Outer");
    let inner = entry_score(&results, "Inner");

    assert!(
        outer.vetoed,
        "Outer: field 2 wire-type mismatch should veto"
    );
    assert!(!inner.vetoed, "Inner: field 2 is unknown, should not veto");
    assert_eq!(inner.unknowns, 1, "Inner: field 2 counts as unknown");
}

/// MT-03: Veto inside a sub-message propagates upward to the parent entry.
///
/// Outer declares field 4 as LEN_MSG → Inner.  Inner expects field 1 as VARINT.
/// We send field 4 as LEN containing a sub-message where field 1 is sent as
/// LEN (wire-type mismatch inside Inner) — this should veto Outer (which
/// recurses) but not Inner-as-root (which sees field 4 as unknown).
#[test]
fn mt03_veto_in_submessage_propagates_upward() {
    let g = build_two_entry_graph();

    // Sub-message payload: field 1 sent as LEN instead of VARINT → mismatch.
    let bad_inner = field_len(1, b"oops");
    let pb = field_len(4, &bad_inner);

    let results = walk::score_all(&pb, &g);

    let outer = entry_score(&results, "Outer");
    let inner = entry_score(&results, "Inner");

    assert!(
        outer.vetoed,
        "Outer: veto in sub-message should propagate up"
    );
    // Inner-as-root sees field 4 as unknown (not declared) — no veto.
    assert!(!inner.vetoed, "Inner-as-root: field 4 is unknown, no veto");
    assert_eq!(inner.unknowns, 1);
}

/// MT-04: Non-canonical tag overhang increments non_canonical for all active
/// entries at that depth.
#[test]
fn mt04_tag_overhang_increments_all_active_entries() {
    let g = build_two_entry_graph();

    // Field 1 with a non-canonical tag varint (1 overhang byte).
    let mut pb = varint_ohb((1u64 << 3) | 0, 1); // tag: field=1, wt=VARINT, ohb=1
    pb.extend(varint(7)); // value

    let results = walk::score_all(&pb, &g);

    let outer = entry_score(&results, "Outer");
    let inner = entry_score(&results, "Inner");

    assert!(!outer.vetoed);
    assert!(!inner.vetoed);
    assert_eq!(outer.non_canonical, 1, "Outer: tag overhang counted");
    assert_eq!(inner.non_canonical, 1, "Inner: tag overhang counted");
}

/// MT-05: Length-prefix overhang increments non_canonical for all active
/// entries at that depth, both recursing and non-recursing.
///
/// Field 4 sent as LEN with overhang on the length prefix.
/// Outer recurses into it (LEN_MSG); Inner-as-root sees it as unknown.
/// Both should get non_canonical += 1 for the overhang.
#[test]
fn mt05_len_prefix_overhang_increments_all_active_entries() {
    let g = build_two_entry_graph();

    // Build field 4 LEN with non-canonical length prefix (overhang=1).
    // Payload: field 1 varint 5 (valid Inner content).
    let inner_payload = field_varint(1, 5);
    let mut pb = tag(4, 2);
    pb.extend(varint_ohb(inner_payload.len() as u64, 1)); // length with ohb=1
    pb.extend(&inner_payload);

    let results = walk::score_all(&pb, &g);

    let outer = entry_score(&results, "Outer");
    let inner = entry_score(&results, "Inner");

    assert!(!outer.vetoed);
    assert!(!inner.vetoed);
    assert_eq!(outer.non_canonical, 1, "Outer: len-prefix overhang counted");
    assert_eq!(inner.non_canonical, 1, "Inner: len-prefix overhang counted");
}

/// MT-06: Enum out-of-range vetoes only the entry with the enum leaf.
///
/// Outer declares field 5 as ENUM [0..2].  Inner has no field 5.
/// Send field 5 = 99 (out of range) → Outer vetoed, Inner gets unknown.
#[test]
fn mt06_enum_oor_vetoes_only_enum_entry() {
    let g = build_two_entry_graph();
    let pb = field_varint(5, 99);
    let results = walk::score_all(&pb, &g);

    let outer = entry_score(&results, "Outer");
    let inner = entry_score(&results, "Inner");

    assert!(outer.vetoed, "Outer: enum 99 outside [0..2] should veto");
    assert!(!inner.vetoed, "Inner: field 5 is unknown, no veto");
    assert_eq!(inner.unknowns, 1);
}

/// TC-20: Enum value exactly at boundary (0 and 2) — both valid.
#[test]
fn tc20_enum_boundary_values() {
    let g = build_graph();
    let root = root_state(&g);

    let mut pb = field_varint(1, 1); // id
    pb.extend(field_varint(5, 0)); // status = OK (min boundary)

    let s = walk::score(&pb, root, &g);
    assert!(!s.vetoed);
    assert_eq!(s.matches, 2);

    let mut pb = field_varint(1, 1); // id
    pb.extend(field_varint(5, 2)); // status = ERR (max boundary)

    let s = walk::score(&pb, root, &g);
    assert!(!s.vetoed);
    assert_eq!(s.matches, 2);
}
