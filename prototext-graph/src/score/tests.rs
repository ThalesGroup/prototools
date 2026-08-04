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
//!   optional uint32 value = 1;   // UINT32 optional
//! }
//!
//! message Outer {
//!   required uint32 id     = 1;  // UINT32 required
//!   optional string name   = 2;  // LEN_STRING optional
//!   repeated uint32 tags   = 3;  // UINT32 repeated
//!   optional Inner  child  = 4;  // LEN_MSG → Inner optional
//!   optional Status status = 5;  // RANGE [0..2] optional
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
use prototext_core::helpers::MAX_WIRE_DEPTH;

/// Score `pb` against the graph and return the result for entry `fqdn`.
fn score_entry<'g>(
    pb: &[u8],
    graph: &'g score_load::LoadedGraph,
    fqdn: &str,
) -> walk::EntryScore<'g> {
    score_entry_opts(pb, graph, fqdn, &walk::ScoringOpts::default())
}

fn score_entry_opts<'g>(
    pb: &[u8],
    graph: &'g score_load::LoadedGraph,
    fqdn: &str,
    opts: &walk::ScoringOpts,
) -> walk::EntryScore<'g> {
    let mut results = walk::score_all(pb, graph, opts);
    let pos = results
        .iter()
        .position(|r| r.fqdn == fqdn)
        .unwrap_or_else(|| panic!("entry '{fqdn}' not found in results"));
    results.swap_remove(pos)
}

// ── Schema fixture ────────────────────────────────────────────────────────────

fn make_merged() -> Merged {
    let inner_fields = vec![ScoringField {
        number: 1,
        kind: ScoringKind::Uint32,
        child: None,
        range: None,
        label: FieldLabel::Optional,
    }];

    let outer_fields = vec![
        ScoringField {
            number: 1,
            kind: ScoringKind::Uint32,
            child: None,
            range: None,
            label: FieldLabel::Required,
        },
        ScoringField {
            number: 2,
            kind: ScoringKind::LenString,
            child: None,
            range: None,
            label: FieldLabel::Optional,
        },
        ScoringField {
            number: 3,
            kind: ScoringKind::Uint32,
            child: None,
            range: None,
            label: FieldLabel::Repeated,
        },
        ScoringField {
            number: 4,
            kind: ScoringKind::Node,
            child: Some("Inner".to_string()),
            range: None,
            label: FieldLabel::Optional,
        },
        ScoringField {
            number: 5,
            kind: ScoringKind::Range,
            child: None,
            range: Some((0, 2)),
            label: FieldLabel::Optional,
        },
    ];

    let mut states = std::collections::HashMap::new();
    states.insert("Inner".to_string(), inner_fields);
    states.insert("Outer".to_string(), outer_fields);

    Merged {
        states,
        node_kinds: std::collections::HashMap::new(),
        roots: vec!["Outer".to_string()],
        ..Default::default()
    }
}

/// Build a graph binary in a tempdir and load it back.
fn build_graph() -> score_load::LoadedGraph {
    let merged = make_merged();
    let (raw, reg) = graph::build(&merged);
    let partition = hopcroft::minimize(&raw, &reg, &raw.node_wire_types, |_, _| {});
    let compiled = graph::compile(&raw, &reg, &partition, &merged.roots);

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("test.bin");
    serial::write(&compiled, &path).expect("write graph");

    // Keep tempdir alive by leaking it — fine for tests.
    let _ = std::mem::ManuallyDrop::new(dir);
    score_load::load_graph(&path).expect("load graph")
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
        out.extend(std::iter::repeat_n(0x80u8, (ohb - 1) as usize));
        out.push(0x00);
    }
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

// ── The score coefficients (spec 0178 S1) ────────────────────────────────────

/// Each counter's weight in isolation, and — the part that matters — their
/// *order*.
///
/// This is the only guard on the coefficients: before spec 0178 no test
/// asserted a numeric score with `mismatches > 0` at all, which is how
/// `mismatches` sat at `-10` while `non_canonical` sat at `-20` even though a
/// missing `required` field is the more damning signal. The ordering assertion
/// is what stops that contradiction from reappearing.
#[test]
fn score_coefficients_rank_by_suspicion() {
    let one = |f: fn(&mut walk::EntryScore<'static>)| -> i64 {
        let mut s = walk::EntryScore {
            fqdn: "M",
            matches: 0,
            unknowns: 0,
            out_of_range: 0,
            non_canonical: 0,
            mismatches: 0,
            vetoed: false,
        };
        f(&mut s);
        s.score()
    };

    let matched = one(|s| s.matches = 1);
    let unknown = one(|s| s.unknowns = 1);
    let oor = one(|s| s.out_of_range = 1);
    let non_canon = one(|s| s.non_canonical = 1);
    let missing_required = one(|s| s.mismatches = 1);

    assert_eq!(matched, 1);
    assert_eq!(unknown, -10);
    assert_eq!(oor, -15);
    assert_eq!(non_canon, -20);
    assert_eq!(missing_required, -30);

    // Increasing suspicion, which is also the order the reports print.
    assert!(
        matched > unknown && unknown > oor && oor > non_canon && non_canon > missing_required,
        "coefficients must rank by suspicion: \
         matched {matched} > unknown {unknown} > out_of_range {oor} > \
         non_canonical {non_canon} > mismatches {missing_required}"
    );
}

/// TC-01: Perfect match — all fields present, canonical.
#[test]
fn tc01_perfect_match() {
    let g = build_graph();

    // Outer { id=1, name="hi", tags=7, child=Inner{value=42}, status=1 }
    let inner = field_varint(1, 42);
    let mut pb = Vec::new();
    pb.extend(field_varint(1, 1)); // id (required)
    pb.extend(field_len(2, b"hi")); // name (optional string)
    pb.extend(field_varint(3, 7)); // tags (repeated)
    pb.extend(field_len(4, &inner)); // child (optional message)
    pb.extend(field_varint(5, 1)); // status = WARN (enum 0..2)

    let s = score_entry(&pb, &g, "Outer");
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

    let mut pb = Vec::new();
    pb.extend(field_varint(10, 1)); // unknown VARINT
    pb.extend(field_varint(11, 2)); // unknown VARINT
    pb.extend(field_len(12, b"x")); // unknown LEN

    let s = score_entry(&pb, &g, "Outer");
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

    // field 1 (id, VARINT) sent as LEN — wire-type mismatch on known field
    let pb = field_len(1, b"oops");

    let s = score_entry(&pb, &g, "Outer");
    assert!(s.vetoed, "wrong wire type on known field should veto");
}

/// TC-04: Invalid UTF-8 on a string field → veto.
#[test]
fn tc04_invalid_utf8_veto() {
    let g = build_graph();

    let mut pb = Vec::new();
    pb.extend(field_varint(1, 1)); // id ok
    pb.extend(field_len(2, b"\xff\xfe invalid")); // name: invalid UTF-8

    let s = score_entry(&pb, &g, "Outer");
    assert!(s.vetoed, "invalid UTF-8 on string field should veto");
}

/// TC-05: Enum value out of range [0..2] → penalized, not vetoed.
///
/// Retargeted by spec 0172 S3 (this is its C6 proving test): the compiled
/// graph carries no syntax bit, and under proto3 every enum is open, so an
/// unknown value is forward-compatible rather than impossible. Vetoing
/// eliminated the blob's own correct FQDN. Spec 0178 then made the penalty
/// unconditional and moved it to its own counter.
#[test]
fn tc05_enum_out_of_range_penalized_by_default() {
    let g = build_graph();

    let mut pb = Vec::new();
    pb.extend(field_varint(1, 1)); // id
    pb.extend(field_varint(5, 99)); // status=99, outside [0..2]

    let s = score_entry(&pb, &g, "Outer");
    assert!(
        !s.vetoed,
        "enum value 99 outside [0..2] should survive by default"
    );
    assert_eq!(s.out_of_range, 1, "out-of-range enum should be penalized");
    assert_eq!(s.non_canonical, 0, "the encoding itself is canonical");
    assert_eq!(s.matches, 2, "both fields still resolve");
}

/// TC-06: Truncated FIXED32 → veto.
#[test]
fn tc06_truncated_fixed32_veto() {
    let g = build_graph();

    // Send a raw field tag for wire type 5 (I32) on an unknown field,
    // then only 2 bytes instead of 4.
    let mut pb = tag(20, 5); // unknown I32 field
    pb.extend_from_slice(&[0x01, 0x02]); // only 2 bytes

    let s = score_entry(&pb, &g, "Outer");
    assert!(s.vetoed, "truncated fixed32 should veto");
}

/// TC-07: Truncated FIXED64 → veto.
#[test]
fn tc07_truncated_fixed64_veto() {
    let g = build_graph();

    let mut pb = tag(20, 1); // unknown I64 field
    pb.extend_from_slice(&[0x01, 0x02, 0x03]); // only 3 bytes

    let s = score_entry(&pb, &g, "Outer");
    assert!(s.vetoed, "truncated fixed64 should veto");
}

/// TC-08: Truncated LEN payload → veto.
#[test]
fn tc08_truncated_len_payload_veto() {
    let g = build_graph();

    let mut pb = tag(2, 2); // name field, LEN
    pb.extend(varint(100)); // claims 100-byte payload
    pb.extend_from_slice(b"short"); // only 5 bytes

    let s = score_entry(&pb, &g, "Outer");
    assert!(s.vetoed, "truncated LEN payload should veto");
}

/// TC-09: Invalid wire type (6) → veto.
#[test]
fn tc09_invalid_wire_type_veto() {
    let g = build_graph();

    let pb = vec![0x06]; // field=0, wire_type=6 — invalid

    let s = score_entry(&pb, &g, "Outer");
    assert!(s.vetoed, "wire type 6 should veto");
}

/// TC-10: Non-canonical varint overhang on tag → non_canonical incremented.
#[test]
fn tc10_tag_overhang_non_canonical() {
    let g = build_graph();

    // Tag for field 1 (id), wire type 0, with 1 overhang byte on the tag.
    let mut pb = varint_ohb(1 << 3, 1); // tag: field=1, wt=VARINT (0), ohb=1
    pb.extend(varint(42)); // id value

    let s = score_entry(&pb, &g, "Outer");
    assert!(!s.vetoed);
    assert_eq!(s.non_canonical, 1);
    assert_eq!(s.matches, 1);
}

/// TC-11: Non-canonical varint overhang on value → non_canonical incremented.
#[test]
fn tc11_value_overhang_non_canonical() {
    let g = build_graph();

    // field 1 (id), canonical tag, non-canonical value
    let mut pb = tag(1, 0);
    pb.extend(varint_ohb(1, 2)); // value 1 with 2 overhang bytes

    let s = score_entry(&pb, &g, "Outer");
    assert!(!s.vetoed);
    assert_eq!(s.non_canonical, 1);
    assert_eq!(s.matches, 1);
}

/// TC-12: Non-canonical LEN length-prefix overhang → non_canonical incremented.
#[test]
fn tc12_len_prefix_overhang_non_canonical() {
    let g = build_graph();

    // field 2 (name), canonical tag, non-canonical length prefix
    let mut pb = tag(2, 2);
    pb.extend(varint_ohb(2, 1)); // length=2 with 1 overhang byte
    pb.extend_from_slice(b"hi");

    let s = score_entry(&pb, &g, "Outer");
    assert!(!s.vetoed);
    assert_eq!(s.non_canonical, 1);
    assert_eq!(s.matches, 1);
}

/// TC-13: Recursion into sub-message — matches accumulate across both levels.
#[test]
fn tc13_submessage_recursion() {
    let g = build_graph();

    // Outer { id=1, child=Inner{value=99} }
    let inner = field_varint(1, 99);
    let mut pb = Vec::new();
    pb.extend(field_varint(1, 1)); // id (required) → match
    pb.extend(field_len(4, &inner)); // child → match + recurse: value → match

    let s = score_entry(&pb, &g, "Outer");
    assert!(!s.vetoed);
    assert_eq!(s.matches, 3); // id + child-field + inner.value
    assert_eq!(s.unknowns, 0);
}

/// TC-14: Mix of known and unknown fields.
#[test]
fn tc14_mixed_known_unknown() {
    let g = build_graph();

    let mut pb = Vec::new();
    pb.extend(field_varint(1, 5)); // id → match
    pb.extend(field_varint(99, 42)); // unknown → unknown
    pb.extend(field_len(2, b"hello")); // name → match
    pb.extend(field_fixed32(88, 0)); // unknown I32 → unknown

    let s = score_entry(&pb, &g, "Outer");
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

    let mut pb = field_varint(1, 1); // id → match
    pb.extend(tag(99, 1)); // unknown I64 field
    pb.extend_from_slice(&[0x00, 0x01]); // only 2 bytes of the required 8

    let s = score_entry(&pb, &g, "Outer");
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
                        range: None,
                        label: FieldLabel::Optional,
                    },
                    ScoringField {
                        number: 2,
                        kind: ScoringKind::I64,
                        child: None,
                        range: None,
                        label: FieldLabel::Optional,
                    },
                ],
            );
            m
        },
        node_kinds: std::collections::HashMap::new(),
        roots: vec!["M".to_string()],
        ..Default::default()
    };
    let (raw, reg) = graph::build(&merged);
    let partition = hopcroft::minimize(&raw, &reg, &raw.node_wire_types, |_, _| {});
    let compiled = graph::compile(&raw, &reg, &partition, &merged.roots);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.bin");
    serial::write(&compiled, &path).unwrap();
    let g = score_load::load_graph(&path).unwrap();

    let mut pb = field_fixed32(1, 0xDEAD_BEEF);
    pb.extend(field_fixed64(2, 0x0102_0304_0506_0708));

    let s = score_entry(&pb, &g, "M");
    assert!(!s.vetoed);
    assert_eq!(s.matches, 2);
    assert_eq!(s.unknowns, 0);
}

/// TC-17: Empty message — no fields → no matches, no unknowns, not vetoed.
#[test]
fn tc17_empty_message() {
    let g = build_graph();
    let s = score_entry(&[], &g, "Outer");
    assert!(!s.vetoed);
    assert_eq!(s.matches, 0);
    assert_eq!(s.unknowns, 0);
}

/// TC-18: END_GROUP outside a group → veto.
#[test]
fn tc18_end_group_outside_group_veto() {
    let g = build_graph();

    let pb = tag(1, 4); // wire type 4 = END_GROUP, not inside a group

    let s = score_entry(&pb, &g, "Outer");
    assert!(s.vetoed, "END_GROUP outside group should veto");
}

/// TC-19: Multiple occurrences of a repeated field — all count as matches.
#[test]
fn tc19_repeated_field_multiple_occurrences() {
    let g = build_graph();

    let mut pb = field_varint(1, 1); // id
    pb.extend(field_varint(3, 10)); // tags[0]
    pb.extend(field_varint(3, 20)); // tags[1]
    pb.extend(field_varint(3, 30)); // tags[2]

    let s = score_entry(&pb, &g, "Outer");
    assert!(!s.vetoed);
    assert_eq!(s.matches, 4); // id + 3× tags
    assert_eq!(s.unknowns, 0);
}

/// Spec 0179 S2: cardinality is still right when `occurrences` spills out of
/// its inline buffer.
///
/// `ActiveEntry::occurrences` is a `SmallVec<[(u32, u32); 2]>`, so a frame
/// carrying three or more *distinct* field numbers moves the whole vec to the
/// heap mid-frame. That happens in 1.85% of real frames — rare enough that the
/// other fixtures here would never reach it by accident, and common enough that
/// it must not be left to chance. The vec is kept sorted and is
/// binary-searched by `apply_cardinality_multi`, so a spill that reordered or
/// truncated it would show up as wrong counts rather than as a crash.
#[test]
fn tc19b_occurrences_spilling_past_the_inline_buffer() {
    let g = build_graph();

    // Five distinct field numbers, one of them repeated: `occurrences` ends as
    // [(1,1), (2,3), (3,1), (4,1), (5,1)] — 5 pairs against an inline 2.
    let mut pb = field_varint(1, 1); // id (required) ×1
    pb.extend(field_len(2, b"aa")); // name (optional) ×3 → non-canonical
    pb.extend(field_len(2, b"bb"));
    pb.extend(field_len(2, b"cc"));
    pb.extend(field_varint(3, 10)); // tags (repeated) ×1
    pb.extend(field_len(4, &field_varint(1, 5))); // inner (optional) ×1
    pb.extend(field_varint(5, 1)); // enum in (0,2) ×1

    let s = score_entry(&pb, &g, "Outer");
    assert!(!s.vetoed);
    // Only the thrice-repeated optional field 2 is non-canonical, and it
    // contributes count - 1 = 2. Everything else appears exactly once, so a
    // spill that lost or duplicated a pair would move this number.
    assert_eq!(s.non_canonical, 2);
    assert_eq!(s.mismatches, 0, "the required field 1 is present");
    assert_eq!(s.unknowns, 0);
}

// ── Multi-entry tests (spec 0048 §7) ──────────────────────────────────────────

/// Build a two-entry graph: Outer (full schema) + Inner (one field).
/// Used by MT-01..MT-04.
fn build_two_entry_graph() -> score_load::LoadedGraph {
    let inner_fields = vec![ScoringField {
        number: 1,
        kind: ScoringKind::Uint32,
        child: None,
        range: None,
        label: FieldLabel::Optional,
    }];

    let outer_fields = vec![
        ScoringField {
            number: 1,
            kind: ScoringKind::Uint32,
            child: None,
            range: None,
            label: FieldLabel::Required,
        },
        ScoringField {
            number: 2,
            kind: ScoringKind::LenString,
            child: None,
            range: None,
            label: FieldLabel::Optional,
        },
        ScoringField {
            number: 4,
            kind: ScoringKind::Node,
            child: Some("Inner".to_string()),
            range: None,
            label: FieldLabel::Optional,
        },
        ScoringField {
            number: 5,
            kind: ScoringKind::Range,
            child: None,
            range: Some((0, 2)),
            label: FieldLabel::Optional,
        },
    ];

    let mut states = std::collections::HashMap::new();
    states.insert("Inner".to_string(), inner_fields);
    states.insert("Outer".to_string(), outer_fields);

    let merged = crate::build_scoring_graph::load::Merged {
        states,
        node_kinds: std::collections::HashMap::new(),
        roots: vec!["Outer".to_string(), "Inner".to_string()],
        ..Default::default()
    };

    let (raw, reg) = graph::build(&merged);
    let partition = hopcroft::minimize(&raw, &reg, &raw.node_wire_types, |_, _| {});
    let compiled = graph::compile(&raw, &reg, &partition, &merged.roots);
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("two.bin");
    serial::write(&compiled, &path).expect("write");
    let _ = std::mem::ManuallyDrop::new(dir);
    score_load::load_graph(&path).expect("load")
}

fn entry_score<'a, 'g>(
    results: &'a [walk::EntryScore<'g>],
    fqdn: &str,
) -> &'a walk::EntryScore<'g> {
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
    let results = walk::score_all(&pb, &g, &walk::ScoringOpts::default());

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
    let results = walk::score_all(&pb, &g, &walk::ScoringOpts::default());

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

    let results = walk::score_all(&pb, &g, &walk::ScoringOpts::default());

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
    let mut pb = varint_ohb(1u64 << 3, 1); // tag: field=1, wt=VARINT (0), ohb=1
    pb.extend(varint(7)); // value

    let results = walk::score_all(&pb, &g, &walk::ScoringOpts::default());

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

    let results = walk::score_all(&pb, &g, &walk::ScoringOpts::default());

    let outer = entry_score(&results, "Outer");
    let inner = entry_score(&results, "Inner");

    assert!(!outer.vetoed);
    assert!(!inner.vetoed);
    assert_eq!(outer.non_canonical, 1, "Outer: len-prefix overhang counted");
    assert_eq!(inner.non_canonical, 1, "Inner: len-prefix overhang counted");
}

/// MT-06: Enum out-of-range charges only the entry with the enum leaf.
///
/// Outer declares field 5 as ENUM [0..2].  Inner has no field 5.
/// Send field 5 = 99 (out of range) → Outer penalized, Inner gets unknown.
///
/// The subject is the *isolation* of the charge to the entry that owns the
/// offending leaf — a per-candidate signal must not leak across the shared
/// walk. Spec 0172 S3 kept that testable by opting into `strict_ranges: true`;
/// with the knob gone (spec 0178 S3) the same isolation is asserted on
/// `out_of_range`, which is the counter the charge now lands in.
#[test]
fn mt06_enum_oor_charges_only_enum_entry() {
    let g = build_two_entry_graph();
    let pb = field_varint(5, 99);
    let results = walk::score_all(&pb, &g, &walk::ScoringOpts::default());

    let outer = entry_score(&results, "Outer");
    let inner = entry_score(&results, "Inner");

    assert!(!outer.vetoed, "Outer: 99 still parses, so no veto");
    assert_eq!(outer.out_of_range, 1, "Outer owns the enum leaf");
    assert!(!inner.vetoed, "Inner: field 5 is unknown, no veto");
    assert_eq!(
        inner.out_of_range, 0,
        "Inner has no enum leaf; the charge must not leak to it"
    );
    assert_eq!(inner.unknowns, 1);
}

/// SO-01: `score_one` against one entry of a multi-root graph matches the
/// corresponding entry from `score_all` on the same blob.
#[test]
fn so01_matches_score_all_entry() {
    let g = build_two_entry_graph();
    let pb = field_varint(1, 42);

    let all_results = walk::score_all(&pb, &g, &walk::ScoringOpts::default());
    let expected = entry_score(&all_results, "Outer");

    let one = walk::score_one(&pb, "Outer", &g, &walk::ScoringOpts::default())
        .expect("Outer should be found");

    assert_eq!(one.fqdn, expected.fqdn);
    assert_eq!(one.matches, expected.matches);
    assert_eq!(one.unknowns, expected.unknowns);
    assert_eq!(one.out_of_range, expected.out_of_range);
    assert_eq!(one.mismatches, expected.mismatches);
    assert_eq!(one.non_canonical, expected.non_canonical);
    assert_eq!(one.vetoed, expected.vetoed);
}

/// SO-02: `score_one` does not walk or affect other entries — Inner sees the
/// same veto behavior it would in isolation, unaffected by Outer's presence.
#[test]
fn so02_only_requested_entry_is_scored() {
    let g = build_two_entry_graph();
    // field 2 sent as VARINT — Outer expects LEN and would veto; Inner has no
    // field 2 and would only see it as unknown.
    let pb = field_varint(2, 99);

    let inner = walk::score_one(&pb, "Inner", &g, &walk::ScoringOpts::default())
        .expect("Inner should be found");
    assert!(!inner.vetoed, "Inner: field 2 is unknown, should not veto");
    assert_eq!(inner.unknowns, 1);
}

/// SO-03: `score_one` accepts a leading-dot fully-qualified name.
#[test]
fn so03_accepts_leading_dot_fqdn() {
    let g = build_two_entry_graph();
    let pb = field_varint(1, 42);

    let bare = walk::score_one(&pb, "Outer", &g, &walk::ScoringOpts::default())
        .expect("bare name should resolve");
    let dotted = walk::score_one(&pb, ".Outer", &g, &walk::ScoringOpts::default())
        .expect("dotted name should resolve");

    assert_eq!(bare.matches, dotted.matches);
}

/// SO-04: `score_one` returns `None` for an fqdn absent from `graph.roots`.
#[test]
fn so04_unknown_fqdn_returns_none() {
    let g = build_two_entry_graph();
    let pb = field_varint(1, 42);

    assert!(walk::score_one(&pb, "NoSuchType", &g, &walk::ScoringOpts::default()).is_none());
}

/// TC-20: Enum value exactly at boundary (0 and 2) — both valid.
#[test]
fn tc20_enum_boundary_values() {
    let g = build_graph();

    let mut pb = field_varint(1, 1); // id
    pb.extend(field_varint(5, 0)); // status = OK (min boundary)

    let s = score_entry(&pb, &g, "Outer");
    assert!(!s.vetoed);
    assert_eq!(s.matches, 2);

    let mut pb = field_varint(1, 1); // id
    pb.extend(field_varint(5, 2)); // status = ERR (max boundary)

    let s = score_entry(&pb, &g, "Outer");
    assert!(!s.vetoed);
    assert_eq!(s.matches, 2);
}

// ── Reproduce AnnotatedMapEntry vs MultiOptionMapEntry merge ──────────────────
//
// MapWithOptions has two map fields:
//   annotated_map = 1  (map<string,int32>) → AnnotatedMapEntry: {f1=LEN_STRING, f2=VARINT}
//   multi_option_map=2 (map<int32,string>)  → MultiOptionMapEntry: {f1=VARINT,   f2=LEN_STRING}
//
// After minimisation, AnnotatedMapEntry and MultiOptionMapEntry MUST be in
// distinct states (they have different field→leaf assignments).
#[test]
fn map_entry_states_are_distinct() {
    let annotated_entry_fields = vec![
        ScoringField {
            number: 1,
            kind: ScoringKind::LenString,
            child: None,
            range: None,
            label: FieldLabel::Optional,
        },
        ScoringField {
            number: 2,
            kind: ScoringKind::Uint32,
            child: None,
            range: None,
            label: FieldLabel::Optional,
        },
    ];
    let multi_option_entry_fields = vec![
        ScoringField {
            number: 1,
            kind: ScoringKind::Uint32,
            child: None,
            range: None,
            label: FieldLabel::Optional,
        },
        ScoringField {
            number: 2,
            kind: ScoringKind::LenString,
            child: None,
            range: None,
            label: FieldLabel::Optional,
        },
    ];
    let map_with_options_fields = vec![
        ScoringField {
            number: 1,
            kind: ScoringKind::Node,
            child: Some("AnnotatedMapEntry".to_string()),
            range: None,
            label: FieldLabel::Repeated,
        },
        ScoringField {
            number: 2,
            kind: ScoringKind::Node,
            child: Some("MultiOptionMapEntry".to_string()),
            range: None,
            label: FieldLabel::Repeated,
        },
    ];

    let mut states = std::collections::HashMap::new();
    states.insert("AnnotatedMapEntry".to_string(), annotated_entry_fields);
    states.insert("MultiOptionMapEntry".to_string(), multi_option_entry_fields);
    states.insert("MapWithOptions".to_string(), map_with_options_fields);

    let merged = crate::build_scoring_graph::load::Merged {
        states,
        node_kinds: std::collections::HashMap::new(),
        roots: vec!["MapWithOptions".to_string()],
        ..Default::default()
    };

    let (raw, reg) = graph::build(&merged);
    let partition = hopcroft::minimize(&raw, &reg, &raw.node_wire_types, |_, _| {});

    let ame_id = raw.node_ids["AnnotatedMapEntry"];
    let moe_id = raw.node_ids["MultiOptionMapEntry"];
    let ame_block = partition.block_of(ame_id);
    let moe_block = partition.block_of(moe_id);

    assert_ne!(
        ame_block, moe_block,
        "AnnotatedMapEntry (block {ame_block}) and MultiOptionMapEntry (block {moe_block}) \
         must be in distinct states after minimisation"
    );
}

// ── Spec 0077: varint leaf refinement tests ───────────────────────────────────

fn build_single_field_graph(
    kind: ScoringKind,
    range: Option<(i32, i32)>,
) -> score_load::LoadedGraph {
    let merged = Merged {
        states: {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "M".to_string(),
                vec![ScoringField {
                    number: 1,
                    kind,
                    child: None,
                    range,
                    label: FieldLabel::Optional,
                }],
            );
            m
        },
        node_kinds: std::collections::HashMap::new(),
        roots: vec!["M".to_string()],
        ..Default::default()
    };
    let (raw, reg) = graph::build(&merged);
    let partition = hopcroft::minimize(&raw, &reg, &raw.node_wire_types, |_, _| {});
    let compiled = graph::compile(&raw, &reg, &partition, &merged.roots);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.bin");
    serial::write(&compiled, &path).unwrap();
    let _ = std::mem::ManuallyDrop::new(dir);
    score_load::load_graph(&path).unwrap()
}

/// TC-77-01: RANGE/bool — wire value 2 on a bool field → `out_of_range`, no veto.
///
/// The cleanest instance of what spec 0178 fixes: `bool` is decoded as
/// `value != 0` in every generated parser, so 2 reads as `true`. Vetoing it
/// eliminated a candidate over a value that decodes fine.
///
/// TC-77-01 and TC-77-02 used to opt into `strict_ranges: true` to assert the
/// veto; there is now one behavior, so TC-77-03 (which asserted the penalty
/// path under the old default) has been folded into them.
#[test]
fn tc77_01_bool_out_of_range_is_penalized() {
    let g = build_single_field_graph(ScoringKind::Range, Some((0, 1)));
    let pb = field_varint(1, 2);
    let s = score_entry(&pb, &g, "M");
    assert!(!s.vetoed, "bool value 2 parses as `true`; it must not veto");
    assert_eq!(s.out_of_range, 1, "outside [0,1] is penalized");
    assert_eq!(s.non_canonical, 0, "the encoding itself is canonical");
}

/// TC-77-02: RANGE/enum — wire value outside [0,2] → `out_of_range`, no veto.
///
/// A closed enum moves an undeclared number to the unknown-field set rather
/// than failing the parse, so the message still round-trips.
#[test]
fn tc77_02_enum_out_of_range_is_penalized() {
    let g = build_single_field_graph(ScoringKind::Range, Some((0, 2)));
    let pb = field_varint(1, 99);
    let s = score_entry(&pb, &g, "M");
    assert!(!s.vetoed, "99 still parses; it must not veto");
    assert_eq!(s.out_of_range, 1);
    assert_eq!(s.non_canonical, 0);
    // One match (+1) against one out-of-range value (-15).
    assert_eq!(s.score(), -14);
}

/// TC-77-04: RANGE, val in the impossible varint gap — still vetoed.
///
/// Spec 0172 S2 narrowed the veto condition from `val >= 2^32` to the gap
/// `0xFFFF_FFFF < val < 0xFFFF_FFFF_8000_0000` — values that are neither a
/// u32 nor a sign-extended i32. `1u64 << 32` sits in that gap.
///
/// This is spec 0178's load-bearing guard: it proves the demotion took only
/// the *legal* half of the arm. If this ever flips to a penalty, the veto of
/// the genuinely impossible has been lost and the arm is toothless.
#[test]
fn tc77_04_range_32bit_overflow_always_veto() {
    let g = build_single_field_graph(ScoringKind::Range, Some((0, 2)));
    let pb = field_varint(1, 1u64 << 32);
    let s = score_entry(&pb, &g, "M");
    assert!(s.vetoed, "a value in the impossible gap must still veto");
}

/// TC-77-05: UINT32 — wire value 2^32 → vetoed (always).
#[test]
fn tc77_05_uint32_overflow_veto() {
    let g = build_single_field_graph(ScoringKind::Uint32, None);
    let pb = field_varint(1, 1u64 << 32);
    let s = score_entry(&pb, &g, "M");
    assert!(s.vetoed, "uint32 value 2^32 should be vetoed");
}

/// TC-77-06: UINT32 — wire value 0xFFFF_FFFF → not vetoed.
#[test]
fn tc77_06_uint32_max_valid() {
    let g = build_single_field_graph(ScoringKind::Uint32, None);
    let pb = field_varint(1, 0xFFFF_FFFF);
    let s = score_entry(&pb, &g, "M");
    assert!(!s.vetoed, "uint32 max value should not be vetoed");
}

/// TC-77-07: INT32 — wire value 0x1_0000_0000 (in invalid gap) → vetoed (always).
#[test]
fn tc77_07_int32_gap_veto() {
    let g = build_single_field_graph(ScoringKind::Int32, None);
    let pb = field_varint(1, 0x1_0000_0000u64);
    let s = score_entry(&pb, &g, "M");
    assert!(s.vetoed, "int32 value in invalid gap should be vetoed");
}

/// TC-77-08: INT32 — wire value 0xFFFF_FFFF_8000_0000 (valid negative int32) →
/// not vetoed, non_canonical not incremented (canonical 10-byte encoding).
#[test]
fn tc77_08_int32_negative_canonical() {
    let g = build_single_field_graph(ScoringKind::Int32, None);
    let pb = field_varint(1, 0xFFFF_FFFF_8000_0000u64);
    let s = score_entry(&pb, &g, "M");
    assert!(!s.vetoed, "valid negative int32 should not be vetoed");
    assert_eq!(
        s.non_canonical, 0,
        "canonical 10-byte encoding should not be non-canonical"
    );
}

/// TC-77-09: INT32 — wire value in [0x8000_0000, 0xFFFF_FFFF] (truncated negative) →
/// not vetoed, non_canonical++.
#[test]
fn tc77_09_int32_truncated_negative() {
    let g = build_single_field_graph(ScoringKind::Int32, None);
    let pb = field_varint(1, 0x8000_0000u64);
    let s = score_entry(&pb, &g, "M");
    assert!(!s.vetoed, "truncated negative int32 should not be vetoed");
    assert_eq!(
        s.non_canonical, 1,
        "truncated negative should increment non_canonical"
    );
}

/// TC-77-10: INT32 — wire value 0x7FFF_FFFF (max positive int32) → not vetoed, no non_canonical.
#[test]
fn tc77_10_int32_max_positive() {
    let g = build_single_field_graph(ScoringKind::Int32, None);
    let pb = field_varint(1, 0x7FFF_FFFF);
    let s = score_entry(&pb, &g, "M");
    assert!(!s.vetoed);
    assert_eq!(s.non_canonical, 0);
}

/// TC-77-11: UINT64 (int64 field) — large value 2^63 → not vetoed.
#[test]
fn tc77_11_uint64_large_value() {
    let g = build_single_field_graph(ScoringKind::Uint64, None);
    let pb = field_varint(1, 1u64 << 63);
    let s = score_entry(&pb, &g, "M");
    assert!(
        !s.vetoed,
        "large value on uint64/int64 field should not be vetoed"
    );
}

/// TC-77-12: Discrimination benefit — bool vs int32 on the same field.
///
/// Two competing schemas:
///   Bool   { field 1: bool  (RANGE [0,1]) }
///   Int32  { field 1: int32 (INT32)       }
///
/// Value 802 on field 1:
///   - Bool  → penalized (802 outside [0,1])
///   - Int32 → clean     (802 is a valid positive int32)
///
/// This is the core discrimination benefit introduced by spec 0077: before
/// this change both schemas collapsed into a single VARINT leaf and 802
/// looked equally plausible under either. Spec 0172 S3 downgraded the
/// penalty from a veto to a counter, and spec 0178 S1 moved it to its own
/// `out_of_range` counter, so the assertion is on the resulting score gap
/// rather than on elimination — the discrimination is what spec 0077 bought,
/// and it survives; the veto was only ever how it happened to be expressed.
#[test]
fn tc77_12_bool_vs_int32_discrimination() {
    let merged = Merged {
        states: {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "Bool".to_string(),
                vec![ScoringField {
                    number: 1,
                    kind: ScoringKind::Range,
                    child: None,
                    range: Some((0, 1)),
                    label: FieldLabel::Optional,
                }],
            );
            m.insert(
                "Int32".to_string(),
                vec![ScoringField {
                    number: 1,
                    kind: ScoringKind::Int32,
                    child: None,
                    range: None,
                    label: FieldLabel::Optional,
                }],
            );
            m
        },
        node_kinds: std::collections::HashMap::new(),
        roots: vec!["Bool".to_string(), "Int32".to_string()],
        ..Default::default()
    };
    let (raw, reg) = graph::build(&merged);
    let partition = hopcroft::minimize(&raw, &reg, &raw.node_wire_types, |_, _| {});
    let compiled = graph::compile(&raw, &reg, &partition, &merged.roots);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.bin");
    serial::write(&compiled, &path).unwrap();
    let _ = std::mem::ManuallyDrop::new(dir);
    let g = score_load::load_graph(&path).unwrap();

    let pb = field_varint(1, 802);
    let bool_s = score_entry(&pb, &g, "Bool");
    let int32_s = score_entry(&pb, &g, "Int32");

    assert!(!bool_s.vetoed, "Bool: out-of-range no longer vetoes");
    assert!(!int32_s.vetoed, "Int32: 802 is a valid positive int32");
    assert_eq!(bool_s.out_of_range, 1, "Bool: 802 outside [0,1] penalized");
    assert_eq!(bool_s.non_canonical, 0, "Bool: the encoding itself is fine");
    assert_eq!(int32_s.out_of_range, 0, "Int32: nothing to penalize");
    assert!(
        int32_s.score() > bool_s.score(),
        "Int32 must outrank Bool on 802: {} vs {}",
        int32_s.score(),
        bool_s.score()
    );
}

// ── Any expansion (spec 0107) ──────────────────────────────────────────────

/// Schema fixture for the `Any`-expansion regression tests:
///
/// ```text
/// message Wrapper       { optional google.protobuf.Any field1 = 1; }
/// message TargetVarint  { optional uint32 field1 = 1; }   // real field 1 = VARINT
/// message TargetString  { optional string field1 = 1; }   // real field 1 = LEN
/// ```
///
/// `google.protobuf.Any` itself must be declared with its own two real
/// fields (`type_url` string=1, `value` bytes=2), matching how a real
/// schema corpus represents it (`any.proto` is an ordinary, if
/// well-known, message). Without this, the raw `ANY_NODE_ID` node has zero
/// outgoing edges, `is_message` evaluates false for it, and the
/// Any-expansion code path in `score_message_multi` never fires at all —
/// silently making any regression test here a no-op.
fn make_merged_any() -> Merged {
    let mut states = std::collections::HashMap::new();
    states.insert(
        "google.protobuf.Any".to_string(),
        vec![
            ScoringField {
                number: 1,
                kind: ScoringKind::LenString,
                child: None,
                range: None,
                label: FieldLabel::Optional,
            },
            ScoringField {
                number: 2,
                kind: ScoringKind::LenBytes,
                child: None,
                range: None,
                label: FieldLabel::Optional,
            },
        ],
    );
    states.insert(
        "Wrapper".to_string(),
        vec![ScoringField {
            number: 1,
            kind: ScoringKind::Node,
            child: Some("google.protobuf.Any".to_string()),
            range: None,
            label: FieldLabel::Optional,
        }],
    );
    states.insert(
        "TargetVarint".to_string(),
        vec![ScoringField {
            number: 1,
            kind: ScoringKind::Uint32,
            child: None,
            range: None,
            label: FieldLabel::Optional,
        }],
    );
    states.insert(
        "TargetString".to_string(),
        vec![ScoringField {
            number: 1,
            kind: ScoringKind::LenString,
            child: None,
            range: None,
            label: FieldLabel::Optional,
        }],
    );

    Merged {
        states,
        node_kinds: std::collections::HashMap::new(),
        roots: vec![
            "Wrapper".to_string(),
            "TargetVarint".to_string(),
            "TargetString".to_string(),
        ],
        ..Default::default()
    }
}

fn build_graph_any() -> score_load::LoadedGraph {
    let merged = make_merged_any();
    let (raw, reg) = graph::build(&merged);
    let partition = hopcroft::minimize(&raw, &reg, &raw.node_wire_types, |_, _| {});
    let compiled = graph::compile(&raw, &reg, &partition, &merged.roots);

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("test_any.bin");
    serial::write(&compiled, &path).expect("write graph");
    let _ = std::mem::ManuallyDrop::new(dir);
    score_load::load_graph(&path).expect("load graph")
}

/// Encode an `Any { type_url, value }` body.
fn any_bytes(type_url: &str, value: &[u8]) -> Vec<u8> {
    let mut b = field_len(1, type_url.as_bytes());
    b.extend(field_len(2, value));
    b
}

/// TC-S1: `Any` wrapping a type whose real field 1 is VARINT must not veto
/// (reproduces the hard-veto manifestation of the spec 0107 bug: pre-fix,
/// the misread `type_url` LEN tag collides with the expected VARINT wire
/// type and vetoes).
#[test]
fn any_expansion_recurses_into_value_only_varint() {
    let g = build_graph_any();

    let value = field_varint(1, 42); // TargetVarint { field1: 42 }
    let any = any_bytes("type.googleapis.com/TargetVarint", &value);
    let pb = field_len(1, &any); // Wrapper { field1: Any }

    let s = score_entry(&pb, &g, "Wrapper");
    assert!(!s.vetoed, "Any wrapping TargetVarint must not veto");
    assert_eq!(s.mismatches, 0);
}

/// TC-S2: `Any` wrapping a type whose real field 1 is LEN (string) must
/// score the wrapped field as a real match, not swallow it as an unknown
/// blob (reproduces the silent-corruption manifestation of the spec 0107
/// bug).
#[test]
fn any_expansion_recurses_into_value_only_string() {
    let g = build_graph_any();

    let value = field_len(1, b"hello"); // TargetString { field1: "hello" }
    let any = any_bytes("type.googleapis.com/TargetString", &value);
    let pb = field_len(1, &any); // Wrapper { field1: Any }

    let s = score_entry(&pb, &g, "Wrapper");
    // matches == 2: 1 for Wrapper.field1 itself (a valid Any-typed LEN
    // field), + 1 for the recursed TargetString.field1 real match.
    assert_eq!(
        s.matches, 2,
        "TargetString.field1 must score as a real recursed match"
    );
    assert_eq!(
        s.unknowns, 0,
        "the value payload must not be swallowed as an unknown field"
    );
}

/// TC-S3: `Any` with an empty `value` must not panic or veto when
/// recursing (regression guard for the `extract_any_value` → empty-slice
/// fallback).
#[test]
fn any_expansion_empty_value() {
    let g = build_graph_any();

    let any = any_bytes("type.googleapis.com/TargetVarint", &[]);
    let pb = field_len(1, &any); // Wrapper { field1: Any }

    let s = score_entry(&pb, &g, "Wrapper");
    assert!(!s.vetoed, "Any with empty value must not veto");
}

// ── Entry-count ceiling (spec 0140 G6, 0172 S5, 0179 S1) ─────────────────────

/// TC-OF1: a corpus past the old `u16` ceiling loads and scores.
///
/// This test has been inverted twice, and both turns are the point of
/// keeping it. Spec 0140 G6 added it as a `#[should_panic]` guard on the
/// `as u16` cast that packed entry indices. Spec 0172 S5 made it assert an
/// `Err` from `load::check_root_count` instead — such a corpus is input,
/// not a programming error, and aborting from a background scoring thread
/// is the wrong response. Spec 0179 S1 widened the index to `u32`, so the
/// corpus this test builds is now simply *valid*, and what needs guarding
/// is that it stays that way.
///
/// The 65 536 roots are structurally identical, so Hopcroft merges them
/// onto a single state and the blob is scored against **one `ActiveEntry`
/// holding 65 536 entries** — which is what actually exercises the width
/// of `ActiveEntry::entries` rather than merely the load-time check.
#[test]
fn tc_of1_a_corpus_past_the_old_u16_ceiling_loads_and_scores() {
    let n = usize::from(u16::MAX) + 1;
    let field = vec![ScoringField {
        number: 1,
        kind: ScoringKind::Uint32,
        child: None,
        range: None,
        label: FieldLabel::Optional,
    }];

    let mut states = std::collections::HashMap::new();
    let mut roots = Vec::with_capacity(n);
    for i in 0..n {
        let name = format!("M{i}");
        states.insert(name.clone(), field.clone());
        roots.push(name);
    }

    let merged = Merged {
        states,
        node_kinds: std::collections::HashMap::new(),
        roots,
        ..Default::default()
    };

    let (raw, reg) = graph::build(&merged);
    let partition = hopcroft::minimize(&raw, &reg, &raw.node_wire_types, |_, _| {});
    let compiled = graph::compile(&raw, &reg, &partition, &merged.roots);
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("huge.bin");
    serial::write(&compiled, &path).expect("write graph");
    let _ = std::mem::ManuallyDrop::new(dir);

    let g = score_load::load_graph(&path).expect("a 65 536-root graph must load");
    assert_eq!(g.graph().roots.len(), n);

    // Field 1, varint 7 — a match for every root in the corpus.
    let pb = field_varint(1, 7);
    let scores = walk::score_all(&pb, g.graph(), &walk::ScoringOpts::default());

    assert_eq!(scores.len(), n, "one score per root");
    assert!(
        scores.iter().all(|s| !s.vetoed && s.matches == 1),
        "every root declares field 1 as uint32, so every root must match it"
    );

    // The last root is the one an index that wrapped would have written over,
    // so it is named explicitly rather than left to the `all` above.
    let last = scores.last().expect("n > 0");
    assert_eq!(last.fqdn, format!("M{}", n - 1));
    assert_eq!(last.matches, 1);
}

// ── Bounds arithmetic and depth caps (spec 0171) ──────────────────────────────

#[test]
fn len_prefix_near_u64_max_vetoes_rather_than_panicking() {
    // Field 2 (LEN, a declared string on `Outer`) with a length prefix of
    // `u64::MAX`. The old `pos + length > buflen` check wrapped in release
    // mode, so the guard passed and `&buf[pos..pos + length]` panicked.
    let g = build_graph();
    let mut pb = vec![0x12]; // field 2, wire type LEN
    pb.extend_from_slice(&varint(u64::MAX));
    let r = score_entry(&pb, &g, "Outer");
    assert!(r.vetoed, "expected a veto, got score {}", r.score());
}

#[test]
fn blind_group_walk_does_not_overflow_the_stack() {
    // 200 000 START_GROUP tags for field 9 — a field `Outer` does not
    // declare, so every entry's verdict is Unknown and the walk goes
    // through `parse_group_blind`. A START_GROUP tag costs one byte, so
    // the recursive form this replaced demanded 200 000 stack frames.
    let g = build_graph();
    let pb = vec![0x4Bu8; 200_000]; // (9 << 3) | 3
    let r = score_entry(&pb, &g, "Outer");
    // The groups are all open-ended, so the walk vetoes; the point of the
    // test is that it returns at all.
    assert!(r.vetoed);
}

/// `message Node { optional Node child = 1; }` — self-recursive, so a nest of
/// LEN fields on field 1 keeps matching all the way down and the walk really
/// recurses instead of vetoing at the first tag.
fn build_recursive_graph() -> score_load::LoadedGraph {
    let merged = Merged {
        states: {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "Node".to_string(),
                vec![ScoringField {
                    number: 1,
                    kind: ScoringKind::Node,
                    child: Some("Node".to_string()),
                    range: None,
                    label: FieldLabel::Optional,
                }],
            );
            m
        },
        node_kinds: std::collections::HashMap::new(),
        roots: vec!["Node".to_string()],
        ..Default::default()
    };
    let (raw, reg) = graph::build(&merged);
    let partition = hopcroft::minimize(&raw, &reg, &raw.node_wire_types, |_, _| {});
    let compiled = graph::compile(&raw, &reg, &partition, &merged.roots);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("node.bin");
    serial::write(&compiled, &path).unwrap();
    let _ = std::mem::ManuallyDrop::new(dir);
    score_load::load_graph(&path).unwrap()
}

/// Flaw C2: `MAX_WIRE_DEPTH` frames of `score_message_multi` must fit in the
/// smallest stack any consumer of `score_all` gives it, which is
/// `std::thread::spawn`'s 2 MiB default. The constant's justification used to
/// be the word "comfortably"; this is the margin, measured.
///
/// Bisecting `stack_size` against this blob puts the walk's requirement at
/// 288 KiB for 501 frames and 576 KiB for 1002 — i.e. **≈ 590 bytes per
/// frame**, so the full cap costs ~576 KiB against this thread's 2 MiB.
///
/// This is **not** the binding constraint on the shared cap: `render_message`
/// in `prototext-core` needs ~1408 KiB for the same depth, 2.4× more. See
/// `MAX_WIRE_DEPTH`'s own doc comment for both figures and the real margin.
///
/// **Release only**, deliberately. The same bisection on a debug build puts
/// the frame at ≈ 4.8 KiB — 8× larger — so the cap wants ~4.7 MiB there and
/// 2 MiB is not enough. Release is what ships and what this repo builds, so
/// the release contract is the honest thing to assert; in debug this would
/// abort the test binary over a configuration no shipped code runs in.
/// (`prototext-core`'s two `deeply_nested_len_*` tests have the same
/// property but are not gated — that is pre-existing, and why `cargo test`
/// without `--release` already aborts there.)
///
/// Note the failure mode: a stack overflow is a `SIGSEGV`, which aborts the
/// whole test process rather than failing this test alone. That is loud and
/// unmistakable ("fatal runtime error: stack overflow"), but it does take the
/// sibling tests with it — so if this binary dies without naming a test, look
/// here first.
#[test]
#[cfg(not(debug_assertions))]
fn max_depth_walk_fits_in_a_default_thread_stack() {
    // Two levels past the cap, so the depth guard is what stops the descent
    // and the deepest frame really is reached.
    let levels = MAX_WIRE_DEPTH + 2;
    let outcome = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            let g = build_recursive_graph();
            let mut pb = Vec::new();
            for _ in 0..levels {
                pb = field_len(1, &pb);
            }
            let s = score_entry(&pb, &g, "Node");
            (s.vetoed, s.matches)
        })
        .expect("spawn")
        .join()
        .expect("the walk must not overflow a 2 MiB stack");

    let (vetoed, matches) = outcome;
    assert!(vetoed, "past the cap the walk vetoes rather than guessing");
    assert_eq!(
        matches,
        MAX_WIRE_DEPTH as u64 + 1,
        "one match per level entered before the guard fires — if this drops, \
         the walk stopped early and the deep frames were never allocated, \
         which would make the stack assertion vacuous"
    );
}

// ── Veto correctness and load validation (spec 0172) ─────────────────────────

/// C4/S1: a field number the wire format forbids must not resolve to a
/// schema field. `2^32 + 1` truncates to `1` under the `as u32` the lookup
/// used to perform, so it used to be credited a match against field 1 (or
/// vetoed for a wire-type mismatch against it) on the strength of a number
/// no schema can declare.
#[test]
fn out_of_range_field_number_does_not_alias() {
    let g = build_single_field_graph(ScoringKind::Uint32, None);

    // Tag for field number 2^32 + 1, wire type 0 — same wire type as the
    // graph's field 1, so an aliased lookup would find and match it.
    const WT_VARINT: u64 = 0;
    let mut pb = varint((((1u64 << 32) + 1) << 3) | WT_VARINT);
    pb.extend(varint(7));

    let s = score_entry(&pb, &g, "M");
    assert!(
        !s.vetoed,
        "an impossible field number is unknown, not fatal"
    );
    assert_eq!(s.matches, 0, "must not alias onto field 1");
    assert_eq!(s.unknowns, 1);
    assert_eq!(s.non_canonical, 1, "the tag itself is still penalized");

    // Control: the same wire type on the legal field number 1 *does* credit
    // a match, so this would fail if S1 had skipped the lookup wholesale.
    let ok = score_entry(&field_varint(1, 7), &g, "M");
    assert_eq!(ok.matches, 1);
    assert_eq!(ok.unknowns, 0);
}

/// C5/S2: `-1` on the wire is sign-extended to ten bytes, i.e.
/// `0xFFFF_FFFF_FFFF_FFFF`. The RANGE arm used to veto anything `>= 2^32`
/// outright, so the *canonical* encoding of a negative enum was fatal while
/// its non-canonical four-byte truncation was merely penalized — exactly
/// inverted.
#[test]
fn canonical_negative_enum_is_not_vetoed() {
    let g = build_single_field_graph(ScoringKind::Range, Some((-1, 2)));
    let pb = field_varint(1, (-1i64) as u64);
    let s = score_entry(&pb, &g, "M");
    assert!(!s.vetoed, "-1 is inside the declared range [-1, 2]");
    assert_eq!(s.matches, 1);
    assert_eq!(s.non_canonical, 0, "ten bytes is canonical for -1");
}

/// C5/S2 companion: a negative *outside* the declared range is treated like
/// any other out-of-range value — penalized, never vetoed (spec 0178 S2, which
/// removed the `strict_ranges` arm this test used to also exercise). What
/// matters is that it is decoded as a negative at all rather than
/// short-circuited by the old `>= 2^32` test.
#[test]
fn negative_enum_outside_range_is_penalized_not_vetoed() {
    let g = build_single_field_graph(ScoringKind::Range, Some((0, 3)));
    let pb = field_varint(1, (-99i64) as u64);

    let s = score_entry(&pb, &g, "M");
    assert!(!s.vetoed, "out of range is unlikely, not impossible");
    assert_eq!(s.out_of_range, 1);
    assert_eq!(
        s.non_canonical, 0,
        "ten bytes is the canonical encoding of a negative"
    );
}

/// C5/S2 regression: the four-byte truncation of a negative — the encoding
/// that already worked before the fix — still costs exactly one
/// `non_canonical` and nothing more, now that the canonical form reaches the
/// same code path.
#[test]
fn truncated_negative_enum_still_costs_exactly_one_penalty() {
    let g = build_single_field_graph(ScoringKind::Range, Some((-1, 2)));
    let pb = field_varint(1, 0xFFFF_FFFFu64); // -1, written in five bytes
    let s = score_entry(&pb, &g, "M");
    assert!(!s.vetoed);
    assert_eq!(
        s.non_canonical, 1,
        "one penalty for the truncation, none for the range"
    );
}

/// C8/S4: `root_offset` comes out of the file header and is attacker-
/// controlled. Unvalidated, `mmap.len() - root_offset` underflowed and
/// `from_raw_parts` fabricated a slice of nearly `usize::MAX` bytes — UB
/// before rkyv's validator ever ran.
#[test]
fn graph_with_out_of_range_root_offset_is_rejected() {
    let mut header = Vec::new();
    header.extend_from_slice(b"PTSGRAPH");
    header.extend_from_slice(&serial::GRAPH_VERSION.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes()); // reserved
    header.extend_from_slice(&u64::MAX.to_le_bytes()); // root_offset
    header.extend_from_slice(&[0u8; 32]); // some payload to map

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("bad-offset.bin");
    std::fs::write(&path, &header).expect("write graph");

    let err = score_load::load_graph(&path)
        .err()
        .expect("an out-of-range root offset must not load")
        .to_string();
    assert!(
        err.contains(&u64::MAX.to_string()),
        "load error should name the offending offset: {err}"
    );
}

/// Spec 0238 S10/N4: a scoring-graph database is a build artifact, so an
/// older version is rejected with a rebuild instruction rather than read
/// through a compatibility shim. The v2 → v3 bump added
/// `NodeEntry.ext_range_idx`, which changes the archived layout — reading a
/// v2 file with a v3 reader would silently misinterpret every node.
#[test]
fn graph_with_older_version_is_rejected_with_a_rebuild_instruction() {
    let mut header = Vec::new();
    header.extend_from_slice(b"PTSGRAPH");
    header.extend_from_slice(&(serial::GRAPH_VERSION - 1).to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes()); // reserved
    header.extend_from_slice(&24u64.to_le_bytes()); // root_offset
    header.extend_from_slice(&[0u8; 32]); // some payload to map

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("old-version.bin");
    std::fs::write(&path, &header).expect("write graph");

    let err = score_load::load_graph(&path)
        .err()
        .expect("an older graph version must not load")
        .to_string();
    assert!(
        err.contains(&(serial::GRAPH_VERSION - 1).to_string())
            && err.contains(&serial::GRAPH_VERSION.to_string()),
        "load error should name both the file's version and this build's: {err}"
    );
    assert!(
        err.contains("--schema-db-out"),
        "load error should tell the operator how to rebuild: {err}"
    );
}

/// Spec 0173 S1: the verdict now lives on the `ActiveEntry` rather than in
/// a side table keyed by `state_id`. The side table existed because the
/// mismatch loop clears entries and `active.retain()` compacts the vector
/// *in the middle of a tag iteration* — so a parallel array indexed by
/// position would hand the survivors each other's verdicts.
///
/// This pins that hazard directly, so a future rewrite back to positional
/// indexing fails here rather than in the field: one root mismatches on the
/// tag and is removed, and the two that outlive it must still be handled
/// with their own verdicts — one Found, one Unknown, which are the two arms
/// a swap would visibly exchange.
#[test]
fn survivors_keep_their_own_verdict_after_a_mismatch_retain() {
    // Three roots arranged in a cycle: for a LEN tag on field `f`, root
    // `R{f}` declares `f` as a varint and is a wire-type Mismatch, root
    // `R{f-1}` declares `f` as a string and is Found, and root `R{f+1}`
    // does not declare `f` at all and is Unknown. No two roots have the
    // same field/type set, so Hopcroft keeps all three apart.
    fn root(varint: u32, string: u32) -> Vec<ScoringField> {
        vec![
            ScoringField {
                number: varint,
                kind: ScoringKind::Uint64,
                child: None,
                range: None,
                label: FieldLabel::Optional,
            },
            ScoringField {
                number: string,
                kind: ScoringKind::LenString,
                child: None,
                range: None,
                label: FieldLabel::Optional,
            },
        ]
    }

    let mut states = std::collections::HashMap::new();
    states.insert("R1".to_string(), root(1, 2));
    states.insert("R2".to_string(), root(2, 3));
    states.insert("R3".to_string(), root(3, 1));

    let merged = Merged {
        states,
        node_kinds: std::collections::HashMap::new(),
        roots: vec!["R1".to_string(), "R2".to_string(), "R3".to_string()],
        ..Default::default()
    };
    let (raw, reg) = graph::build(&merged);
    let partition = hopcroft::minimize(&raw, &reg, &raw.node_wire_types, |_, _| {});
    let compiled = graph::compile(&raw, &reg, &partition, &merged.roots);
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("retain.bin");
    serial::write(&compiled, &path).expect("write");
    let _ = std::mem::ManuallyDrop::new(dir);
    let g = score_load::load_graph(&path).expect("load");

    // `group_by_state` orders the active set by state_id, so the removal
    // only shifts the survivors if the mismatching root sorts ahead of both
    // of them. That order is not ours to choose: `graph::build` assigns node
    // IDs by `HashMap` iteration order, which differs from process to
    // process. So read it back and aim the blob at whichever root sorts
    // first — the cycle above makes any of the three a valid mismatcher.
    let state_of = |fqdn: &str| -> u32 {
        g.roots
            .iter()
            .find(|r| r.fqdn.as_str() == fqdn)
            .map(|r| r.state_id.to_native())
            .unwrap_or_else(|| panic!("root '{fqdn}' not in graph"))
    };
    let f = (1..=3u32)
        .min_by_key(|i| state_of(&format!("R{i}")))
        .expect("three roots");
    let mismatch = format!("R{f}");
    let found_name = format!("R{}", if f == 1 { 3 } else { f - 1 });
    let unknown_name = format!("R{}", if f == 3 { 1 } else { f + 1 });

    let pb = field_len(f, b"hi");
    let results = walk::score_all(&pb, &g, &walk::ScoringOpts::default());

    assert!(entry_score(&results, &mismatch).vetoed);

    let found = entry_score(&results, &found_name);
    assert!(!found.vetoed);
    assert_eq!(
        found.matches, 1,
        "{found_name} must keep its own Found verdict"
    );
    assert_eq!(found.unknowns, 0);

    let unknown = entry_score(&results, &unknown_name);
    assert!(!unknown.vetoed);
    assert_eq!(
        unknown.unknowns, 1,
        "{unknown_name} must keep its own Unknown verdict"
    );
    assert_eq!(unknown.matches, 0);
}

// ── Packed and expanded repeated scalars (spec 0175) ──────────────────────────

/// A single root `P` covering every packability case:
///
/// ```text
/// message P {
///   repeated uint64 nums    = 1;  // packable, varint elements
///   repeated fixed64 f64s   = 2;  // packable, 8-byte elements
///   repeated fixed32 f32s   = 3;  // packable, 4-byte elements
///   optional uint64  one    = 4;  // NOT packable — not repeated
///   repeated string  strs   = 5;  // NOT packable — LEN element
///   repeated Status  states = 6;  // packable, range [0..2]
/// }
/// ```
fn build_packed_graph() -> score_load::LoadedGraph {
    let f = |number: u32, kind: ScoringKind, range, label| ScoringField {
        number,
        kind,
        child: None,
        range,
        label,
    };
    let fields = vec![
        f(1, ScoringKind::Uint64, None, FieldLabel::Repeated),
        f(2, ScoringKind::I64, None, FieldLabel::Repeated),
        f(3, ScoringKind::I32, None, FieldLabel::Repeated),
        f(4, ScoringKind::Uint64, None, FieldLabel::Optional),
        f(5, ScoringKind::LenString, None, FieldLabel::Repeated),
        f(6, ScoringKind::Range, Some((0, 2)), FieldLabel::Repeated),
    ];

    let mut states = std::collections::HashMap::new();
    states.insert("P".to_string(), fields);
    let merged = Merged {
        states,
        node_kinds: std::collections::HashMap::new(),
        roots: vec!["P".to_string()],
        ..Default::default()
    };

    let (raw, reg) = graph::build(&merged);
    let partition = hopcroft::minimize(&raw, &reg, &raw.node_wire_types, |_, _| {});
    let compiled = graph::compile(&raw, &reg, &partition, &merged.roots);
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("packed.bin");
    serial::write(&compiled, &path).expect("write");
    let _ = std::mem::ManuallyDrop::new(dir);
    score_load::load_graph(&path).expect("load")
}

/// A repeated varint field accepts the packed encoding, and scores it as one
/// wire occurrence rather than one per element.
#[test]
fn packed_varint_run_matches() {
    let g = build_packed_graph();

    let mut payload = varint(1);
    payload.extend(varint(300));
    payload.extend(varint(u64::MAX));
    let s = score_entry(&field_len(1, &payload), &g, "P");

    assert!(
        !s.vetoed,
        "packed encoding of a repeated field must not veto"
    );
    assert_eq!(
        s.matches, 1,
        "a packed run is one wire occurrence, not three"
    );
    assert_eq!(s.non_canonical, 0);
}

/// The expanded encoding of the same field still works — the regression guard
/// on the path spec 0175 did not mean to touch.
#[test]
fn expanded_varint_run_still_matches() {
    let g = build_packed_graph();

    let mut pb = field_varint(1, 1);
    pb.extend(field_varint(1, 300));
    pb.extend(field_varint(1, u64::MAX));
    let s = score_entry(&pb, &g, "P");

    assert!(!s.vetoed);
    assert_eq!(s.matches, 3, "three tags are three wire occurrences");
    assert_eq!(s.non_canonical, 0);
}

/// A packed 8-byte-element run whose length is not a multiple of 8 cannot
/// exist, so it vetoes rather than being penalized.
#[test]
fn packed_fixed64_run_of_wrong_length_vetoes() {
    let g = build_packed_graph();
    let s = score_entry(&field_len(2, &[0u8; 12]), &g, "P");
    assert!(s.vetoed, "12 bytes cannot be a run of fixed64");
}

/// The same 12 bytes are a valid run of three fixed32.
#[test]
fn packed_fixed32_run_of_right_length_matches() {
    let g = build_packed_graph();
    let s = score_entry(&field_len(3, &[0u8; 12]), &g, "P");
    assert!(!s.vetoed);
    assert_eq!(s.matches, 1);
}

/// A packed varint run whose last element's continuation bit is set runs past
/// the payload, which is impossible rather than merely unlikely.
#[test]
fn packed_varint_run_past_payload_end_vetoes() {
    let g = build_packed_graph();

    let mut payload = varint(1);
    payload.push(0x80); // continuation bit set, no following byte
    let s = score_entry(&field_len(1, &payload), &g, "P");

    assert!(s.vetoed, "unterminated packed varint should veto");
}

/// A zero-length run is legal — zero elements — but no conformant writer emits
/// one, so it matches and is penalized rather than vetoed.
#[test]
fn packed_empty_run_matches_and_is_penalized() {
    let g = build_packed_graph();
    let s = score_entry(&field_len(1, b""), &g, "P");

    assert!(!s.vetoed, "an empty packed run is legal");
    assert_eq!(s.matches, 1);
    assert_eq!(
        s.non_canonical, 1,
        "protoc omits the field rather than emitting a zero-length run"
    );
}

/// The narrowness guard (Goal 4): packability requires the field to be
/// repeated. A LEN tag on an `optional` varint field is still a mismatch.
#[test]
fn len_tag_on_optional_varint_field_still_vetoes() {
    let g = build_packed_graph();
    let s = score_entry(&field_len(4, &varint(7)), &g, "P");
    assert!(s.vetoed, "only a repeated field is packable");
}

/// Packability must not run backwards: a varint tag on a repeated *string*
/// field is still a wire-type mismatch.
#[test]
fn varint_tag_on_repeated_string_field_still_vetoes() {
    let g = build_packed_graph();
    let s = score_entry(&field_varint(5, 7), &g, "P");
    assert!(s.vetoed, "a string element has no expanded varint form");
}

/// Spec 0175 S4: a packed element is scored by exactly the same rules as the
/// expanded encoding of the same values. Asserted against that encoding rather
/// than a hardcoded number, so the two forms are pinned to each other.
#[test]
fn packed_enum_element_scores_like_the_expanded_one() {
    let g = build_packed_graph();

    let mut packed_payload = varint(1);
    packed_payload.extend(varint(99)); // outside [0..2]
    let packed = score_entry(&field_len(6, &packed_payload), &g, "P");

    let mut expanded = field_varint(6, 1);
    expanded.extend(field_varint(6, 99));
    let expanded = score_entry(&expanded, &g, "P");

    assert!(!packed.vetoed);
    assert!(!expanded.vetoed);
    assert_eq!(
        packed.out_of_range, expanded.out_of_range,
        "the out-of-range enum penalty must not depend on the encoding"
    );
    assert_eq!(packed.out_of_range, 1);
    assert_eq!(
        packed.non_canonical, expanded.non_canonical,
        "neither encoding is itself non-canonical here"
    );
    assert_eq!(packed.non_canonical, 0);
}

// ── Root subsets and partitions (spec 0217) ───────────────────────────────────

/// `n` roots, each a single-field message. Roots `i` and `i + n/2` declare
/// the *same* field, so Hopcroft collapses them onto one state — which is
/// what gives `partition_roots` groups larger than one to keep together.
fn build_many_root_graph(n: u32) -> score_load::LoadedGraph {
    let mut states = std::collections::HashMap::new();
    for i in 0..n {
        states.insert(
            format!("R{i}"),
            vec![ScoringField {
                number: (i % (n / 2).max(1)) + 1,
                kind: ScoringKind::Uint32,
                child: None,
                range: None,
                label: FieldLabel::Optional,
            }],
        );
    }
    let merged = Merged {
        states,
        node_kinds: std::collections::HashMap::new(),
        roots: (0..n).map(|i| format!("R{i}")).collect(),
        ..Default::default()
    };
    let (raw, reg) = graph::build(&merged);
    let partition = hopcroft::minimize(&raw, &reg, &raw.node_wire_types, |_, _| {});
    let compiled = graph::compile(&raw, &reg, &partition, &merged.roots);

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("many.bin");
    serial::write(&compiled, &path).expect("write graph");
    let _ = std::mem::ManuallyDrop::new(dir);
    score_load::load_graph(&path).expect("load graph")
}

/// Spec 0217 G2: sharding is a scheduling change, never a scoring change.
/// Every partition of the roots, scored subset by subset and reassembled,
/// must reproduce `score_all` counter for counter.
///
/// The reassembly is by FQDN rather than by position, because that is the
/// claim worth making: a subset's results come back in *subset* order, and
/// the merge must not depend on them lining up with graph order.
#[test]
fn a_sharded_sweep_matches_the_whole_sweep() {
    let g = build_many_root_graph(12);
    // Field 1 as a varint (a match for the R0/R6 pair, unknown for the
    // rest) followed by field 3 as LEN (a wire-type mismatch, so a veto,
    // for whoever declares field 3). A blob that lands on all three
    // verdicts is what makes the comparison mean something.
    let mut pb = field_varint(1, 7);
    pb.extend(field_len(3, b"hi"));

    let opts = walk::ScoringOpts::default();
    let whole = walk::score_all(&pb, &g, &opts);
    let of = |rs: &[walk::EntryScore], fqdn: &str| -> (u64, u64, u64, u64, u64, bool) {
        let r = rs
            .iter()
            .find(|r| r.fqdn == fqdn)
            .unwrap_or_else(|| panic!("'{fqdn}' missing"));
        (
            r.matches,
            r.unknowns,
            r.out_of_range,
            r.non_canonical,
            r.mismatches,
            r.vetoed,
        )
    };

    for n in 1..=8 {
        let parts = walk::partition_roots(&g, n);
        let mut sharded: Vec<walk::EntryScore> = Vec::new();
        for part in &parts {
            sharded.extend(walk::score_subset(&pb, &g, &opts, part, None));
        }
        assert_eq!(
            sharded.len(),
            whole.len(),
            "n={n}: every root must be scored exactly once"
        );
        for r in &whole {
            assert_eq!(
                of(&sharded, r.fqdn),
                of(&whole, r.fqdn),
                "n={n}: '{}' scored differently when sharded",
                r.fqdn
            );
        }
    }
}

/// Spec 0217 S1: the partition's whole purpose. Two roots sharing a graph
/// state are indistinguishable to the walk, so splitting them across parts
/// duplicates the traversal instead of dividing it — the partition must
/// keep a state group whole. It must also be a partition: disjoint, and
/// covering every root exactly once.
#[test]
fn a_partition_never_splits_a_state_group() {
    let g = build_many_root_graph(12);
    let state_of: std::collections::HashMap<u32, u32> = g
        .roots
        .iter()
        .enumerate()
        .map(|(i, r)| (i as u32, r.state_id.to_native()))
        .collect();

    // Past 6 there are more parts asked for than states to fill them, which
    // is the case spec 0218 leans on: the target part count is a constant,
    // so a small graph must clamp itself rather than yield empty parts.
    for n in 1..=20 {
        let parts = walk::partition_roots(&g, n);
        assert!(parts.len() <= n, "n={n}: at most n parts");
        assert!(parts.iter().all(|p| !p.is_empty()), "n={n}: no empty parts");

        let mut seen: Vec<u32> = parts.iter().flatten().copied().collect();
        seen.sort_unstable();
        assert_eq!(
            seen,
            (0..g.roots.len() as u32).collect::<Vec<_>>(),
            "n={n}: every root exactly once"
        );

        // Each state must appear in exactly one part.
        let mut home: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
        for (pi, part) in parts.iter().enumerate() {
            for r in part {
                let state = state_of[r];
                let first = *home.entry(state).or_insert(pi);
                assert_eq!(
                    first, pi,
                    "n={n}: state {state} is split across parts {first} and {pi}"
                );
            }
        }
    }
}

/// A subset's results come back in *subset* order, not graph order — the
/// property `a_sharded_sweep_matches_the_whole_sweep` relies on and that a
/// caller merging by position would silently violate.
#[test]
fn a_subset_reports_its_entries_in_subset_order() {
    let g = build_many_root_graph(6);
    let pb = field_varint(1, 7);
    let subset: Vec<u32> = vec![4, 1, 3];
    let results = walk::score_subset(&pb, &g, &walk::ScoringOpts::default(), &subset, None);
    let got: Vec<&str> = results.iter().map(|r| r.fqdn).collect();
    let want: Vec<&str> = subset
        .iter()
        .map(|&i| g.roots[i as usize].fqdn.as_str())
        .collect();
    assert_eq!(got, want);
}

/// A raised `cancel` flag stops the walk. Checked by the absence of the
/// matches an uncancelled walk over the same input does record, so the
/// test fails if the poll is ever dropped from the loop rather than
/// merely passing vacuously.
#[test]
fn a_raised_cancel_flag_stops_the_walk() {
    use std::sync::atomic::AtomicBool;

    let g = build_many_root_graph(6);
    let pb = field_varint(1, 7);
    let subset: Vec<u32> = vec![0, 1, 2];
    let opts = walk::ScoringOpts::default();

    let full = walk::score_subset(&pb, &g, &opts, &subset, None);
    assert!(
        full.iter().any(|r| r.matches > 0),
        "the uncancelled walk scored nothing, so the check below proves nothing",
    );

    let cancel = AtomicBool::new(true);
    let stopped = walk::score_subset(&pb, &g, &opts, &subset, Some(&cancel));
    assert!(
        stopped.iter().all(|r| r.matches == 0 && !r.vetoed),
        "the walk kept going past a raised cancel flag",
    );
}
