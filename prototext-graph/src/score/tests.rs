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
// Only `max_depth_walk_fits_in_a_default_thread_stack` reads it, and that
// test is release-only, so in a debug build this is dead — as is
// `build_recursive_graph`, its other exclusive dependency.
#[cfg(not(debug_assertions))]
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
    compile_and_load(&make_merged())
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
            truncated: 0,
            termination: 0,
        };
        f(&mut s);
        s.score()
    };

    let matched = one(|s| s.matches = 1);
    let cut = one(|s| s.truncated = 1);
    let unknown = one(|s| s.unknowns = 1);
    let oor = one(|s| s.out_of_range = 1);
    let non_canon = one(|s| s.non_canonical = 1);
    let missing_required = one(|s| s.mismatches = 1);

    assert_eq!(matched, 1);
    assert_eq!(cut, -5);
    assert_eq!(unknown, -10);
    assert_eq!(oor, -15);
    assert_eq!(non_canon, -20);
    assert_eq!(missing_required, -30);

    // Increasing suspicion, which is also the order the reports print.
    // `truncated` (spec 0310) is the mildest charge of all: it is evidence
    // about the capture, not about the writer and not about the schema fit.
    assert!(
        matched > cut
            && cut > unknown
            && unknown > oor
            && oor > non_canon
            && non_canon > missing_required,
        "coefficients must rank by suspicion: \
         matched {matched} > truncated {cut} > unknown {unknown} > out_of_range {oor} > \
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

/// TC-S4: an `Any` whose `type_url` names nothing in the graph scores its
/// `value` as a plain bytes match and does not recurse.
///
/// The miss is the branch spec 0291 rewrote: resolution used to be a linear
/// `find` that ran off the end of `graph.roots`, and is now a lookup that
/// returns `None`. A hit is already covered by TC-S1..S3, so this is the
/// half that had no guard.
#[test]
fn any_expansion_leaves_an_unresolvable_type_url_alone() {
    let g = build_graph_any();

    let value = field_len(1, b"hello");
    let any = any_bytes("type.googleapis.com/NoSuchType", &value);
    let pb = field_len(1, &any); // Wrapper { field1: Any }

    let s = score_entry(&pb, &g, "Wrapper");
    assert!(!s.vetoed, "an unresolvable type_url must not veto");
    assert_eq!(
        s.matches, 1,
        "only Wrapper.field1 matches; the value is opaque bytes"
    );
    assert_eq!(s.unknowns, 0);
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
///
/// Gated with its one caller below, which is release-only.
#[cfg(not(debug_assertions))]
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

/// Spec 0288 S4: buffering the run's decoded elements must not disturb the
/// order they are visited in, nor where the walk stops. `non_canonical` is the
/// count of offending elements *before* the first veto, so a summary of the run
/// could not reproduce it — which is the reason S1 buffers values instead.
///
/// The run is built so that the two facts are separable: two penalized elements,
/// then one that vetoes, then a third penalized one that must never be reached.
/// A buffer read to the end would report 3; reading it with the break in place
/// reports 2. Pinned against the expanded encoding of the same values, whose
/// break is a different mechanism entirely — `active.retain` drops the candidate
/// between tokens — so agreement is evidence and not a shared bug.
#[test]
fn packed_run_scores_identically_when_buffered() {
    let g = build_packed_graph();

    // -1 as a 5-byte int32: non-canonical, and out of the [0..2] range.
    const NEG: u64 = 0xFFFF_FFFF;
    // Neither a u32 nor a sign-extended i32 — impossible, so it vetoes.
    const GAP: u64 = 0x1_0000_0000;

    let mut payload = varint(NEG);
    payload.extend(varint(NEG));
    payload.extend(varint(GAP));
    payload.extend(varint(NEG));
    let packed = score_entry(&field_len(6, &payload), &g, "P");

    let mut pb = field_varint(6, NEG);
    pb.extend(field_varint(6, NEG));
    pb.extend(field_varint(6, GAP));
    pb.extend(field_varint(6, NEG));
    let expanded = score_entry(&pb, &g, "P");

    assert!(packed.vetoed, "the third element is an impossible int32");
    assert!(expanded.vetoed);
    assert_eq!(
        packed.non_canonical, expanded.non_canonical,
        "the prefix count must not depend on the encoding"
    );
    assert_eq!(
        packed.non_canonical, 2,
        "the fourth element is past the break and must not be counted"
    );
    assert_eq!(packed.out_of_range, expanded.out_of_range);
    assert_eq!(packed.out_of_range, 2);
}

/// Spec 0288 S1/S2: the scratch buffer is reused across tokens, so it must be
/// cleared before each run is decoded into it.
///
/// A stale tail is invisible to the run that produced it and only shows up on a
/// *shorter* following run, which is what this encodes: three penalized elements
/// then a single clean one. A buffer that grew instead of being cleared would
/// re-visit the first run's elements under the second run's candidate and report
/// 6 rather than 3.
#[test]
fn packed_scratch_is_cleared_between_runs() {
    let g = build_packed_graph();

    const NEG: u64 = 0xFFFF_FFFF;
    let mut long_run = varint(NEG);
    long_run.extend(varint(NEG));
    long_run.extend(varint(NEG));

    let mut pb = field_len(6, &long_run);
    pb.extend(field_len(6, &varint(1)));
    let s = score_entry(&pb, &g, "P");

    assert!(!s.vetoed);
    assert_eq!(s.matches, 2, "two runs are two wire occurrences");
    assert_eq!(
        s.non_canonical, 3,
        "the second run has one clean element, and inherits none"
    );
    assert_eq!(s.out_of_range, 3);
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

/// One state shared by `big` roots, plus `singles` roots each alone on a
/// state. The googleapis shape in miniature: on that corpus one group held
/// 4645 roots while most held one, which is what made balancing on the root
/// count fill three whole parts with almost no work (spec 0290).
fn build_lopsided_root_graph(big: u32, singles: u32) -> score_load::LoadedGraph {
    let mut states = std::collections::HashMap::new();
    let field = |number: u32| {
        vec![ScoringField {
            number,
            kind: ScoringKind::Uint32,
            child: None,
            range: None,
            label: FieldLabel::Optional,
        }]
    };
    // Field number 1 throughout, so Hopcroft collapses these onto one state.
    for i in 0..big {
        states.insert(format!("B{i}"), field(1));
    }
    for i in 0..singles {
        states.insert(format!("S{i}"), field(i + 2));
    }
    let roots: Vec<String> = (0..big)
        .map(|i| format!("B{i}"))
        .chain((0..singles).map(|i| format!("S{i}")))
        .collect();
    let merged = Merged {
        states,
        node_kinds: std::collections::HashMap::new(),
        roots: roots.clone(),
        ..Default::default()
    };
    let (raw, reg) = graph::build(&merged);
    let partition = hopcroft::minimize(&raw, &reg, &raw.node_wire_types, |_, _| {});
    let compiled = graph::compile(&raw, &reg, &partition, &roots);

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("lopsided.bin");
    serial::write(&compiled, &path).expect("write graph");
    let _ = std::mem::ManuallyDrop::new(dir);
    score_load::load_graph(&path).expect("load graph")
}

/// Spec 0290: a part is weighed by how many *groups* it holds, because a
/// group is one traversal however many roots share it.
///
/// The regression this guards is what the code actually did. With one group
/// of 40 roots and 40 singleton groups, balancing on the root count gives
/// that one group a part to itself and then spends the next 39 handouts
/// bringing other parts up to its size — so one part ends up holding a
/// single group and doing almost nothing, which is the 24-part googleapis
/// failure in miniature. Root counts are what looked even while that
/// happened; group counts are what did not.
#[test]
fn a_partition_is_balanced_by_group_count_not_root_count() {
    let g = build_lopsided_root_graph(40, 40);
    let state_of: std::collections::HashMap<u32, u32> = g
        .roots
        .iter()
        .enumerate()
        .map(|(i, r)| (i as u32, r.state_id.to_native()))
        .collect();

    for n in 2..=8 {
        let parts = walk::partition_roots(&g, n);
        let group_counts: Vec<usize> = parts
            .iter()
            .map(|p| {
                p.iter()
                    .map(|r| state_of[r])
                    .collect::<std::collections::HashSet<_>>()
                    .len()
            })
            .collect();
        let lo = *group_counts.iter().min().expect("a non-empty partition");
        let hi = *group_counts.iter().max().expect("a non-empty partition");
        assert!(
            hi - lo <= 1,
            "n={n}: group counts {group_counts:?} differ by more than one"
        );
    }
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

// ── The SCAN policy (spec 0238 S9, S12-S16) ──────────────────────────────────

/// Compile and load an arbitrary `Merged`, so a test can vary the schema
/// rather than the payload.
fn compile_and_load(merged: &Merged) -> score_load::LoadedGraph {
    let (raw, reg) = graph::build(merged);
    let partition = hopcroft::minimize(&raw, &reg, &raw.node_wire_types, |_, _| {});
    let compiled = graph::compile(&raw, &reg, &partition, &merged.roots);

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("test.bin");
    serial::write(&compiled, &path).expect("write graph");

    // Keep tempdir alive by leaking it — fine for tests.
    let _ = std::mem::ManuallyDrop::new(dir);
    score_load::load_graph(&path).expect("load graph")
}

fn scan_opts() -> walk::ScoringOpts {
    walk::ScoringOpts {
        policy: walk::Policy::Scan,
        ..Default::default()
    }
}

fn uint32(number: u32, label: FieldLabel) -> ScoringField {
    ScoringField {
        number,
        kind: ScoringKind::Uint32,
        child: None,
        range: None,
        label,
    }
}

fn string(number: u32, label: FieldLabel) -> ScoringField {
    ScoringField {
        number,
        kind: ScoringKind::LenString,
        child: None,
        range: None,
        label,
    }
}

/// A one-root schema, with `ext` as its extension ranges.
fn scan_merged(fields: Vec<ScoringField>, ext: &[(u32, u32)]) -> Merged {
    let mut states = std::collections::HashMap::new();
    states.insert("Rec".to_string(), fields);
    let mut ext_ranges = std::collections::HashMap::new();
    if !ext.is_empty() {
        ext_ranges.insert("Rec".to_string(), ext.to_vec());
    }
    Merged {
        states,
        node_kinds: std::collections::HashMap::new(),
        roots: vec!["Rec".to_string()],
        ext_ranges,
        has_extension_ranges: true,
    }
}

/// Test 7 (S9). Extension ranges are a precondition of `Scan`, not a
/// modifier: a graph built without reproto's flag has an empty range set on
/// *every* message, so honoring it silently would terminate on the first
/// custom option of every descriptor and return plausible, wrong answers.
#[test]
#[should_panic(expected = "--emit-extension-ranges")]
fn test_scan_requires_extension_ranges() {
    // `build_graph`'s fixture leaves `has_extension_ranges` false.
    let g = build_graph();
    let _ = walk::score_all(&field_varint(1, 1), &g, &scan_opts());
}

/// Test 8 (S12 rule 2). A singular field cannot appear twice in one record,
/// so its second appearance opens the next one. `required` terminates for the
/// same reason `optional` does — the rule is about cardinality.
#[test]
fn test_scan_terminates_on_repeated_singular() {
    for label in [FieldLabel::Optional, FieldLabel::Required] {
        let g = compile_and_load(&scan_merged(
            vec![string(1, label), uint32(2, FieldLabel::Repeated)],
            &[],
        ));

        let mut pb = field_len(1, b"a"); // 0..3
        pb.extend(field_varint(2, 7)); // 3..5
        pb.extend(field_len(1, b"b")); // the next record starts here
        pb.extend(field_varint(2, 9));

        let r = score_entry_opts(&pb, &g, "Rec", &scan_opts());
        assert_eq!(r.termination, 5, "{label:?}: wrong record boundary");
        assert_eq!(r.matches, 2, "{label:?}: scored past the boundary");
        assert_eq!(r.mismatches, 0, "{label:?}: the record is complete");

        // Under `Score` the same bytes are one long instance of `Rec`: the
        // second field 1 is a non-canonical duplicate, not a boundary.
        let s = score_entry(&pb, &g, "Rec");
        assert_eq!(s.termination, pb.len(), "{label:?}");
        assert_eq!(s.matches, 4, "{label:?}");
        assert_eq!(s.non_canonical, 1, "{label:?}");
    }
}

/// Test 9 (S12 rule 1, S15). A state with an empty range set has declared
/// itself closed, so an undeclared field number in it is a boundary — and the
/// *same* field number becomes innocent once the message declares room for it.
#[test]
fn test_scan_terminates_on_closed_state_unknown() {
    let mut pb = field_varint(1, 5); // 0..2
    pb.extend(field_varint(9, 1)); // undeclared

    let closed = compile_and_load(&scan_merged(vec![uint32(1, FieldLabel::Optional)], &[]));
    let r = score_entry_opts(&pb, &closed, "Rec", &scan_opts());
    assert_eq!(r.termination, 2, "a closed state must end at an unknown");
    assert_eq!(r.matches, 1);
    assert_eq!(r.unknowns, 0, "the field was never consumed");

    let open = compile_and_load(&scan_merged(
        vec![uint32(1, FieldLabel::Optional)],
        &[(9, 9)],
    ));
    let r = score_entry_opts(&pb, &open, "Rec", &scan_opts());
    assert_eq!(
        r.termination,
        pb.len(),
        "a declared extension range must not end the record"
    );
    assert_eq!(r.matches, 1);
    assert_eq!(
        r.unknowns, 0,
        "an extension is neither evidence for nor against (S15)"
    );

    // S15 is `Scan`-only: under `Score` an unknown is an unknown, whatever
    // the graph declares.
    assert_eq!(score_entry(&pb, &open, "Rec").unknowns, 1);
}

/// Test 10 (S13). The offset is the first byte of the terminating tag. A
/// one-byte error here is invisible to every other test in this file and
/// fatal to protoscan, so the terminating field is given a two-byte tag *and*
/// a length prefix: reading `pos` after the verdict would report 4 or 5, not
/// 2.
#[test]
fn test_scan_termination_offset_is_the_tag() {
    let g = compile_and_load(&scan_merged(vec![uint32(1, FieldLabel::Optional)], &[]));

    let mut pb = field_varint(1, 1); // 0..2
    let terminator = field_len(1000, b"xyz"); // tag is 2 bytes, prefix 1 more
    pb.extend(terminator.iter().copied());

    let r = score_entry_opts(&pb, &g, "Rec", &scan_opts());
    assert_eq!(r.termination, 2);
    assert_eq!(
        &pb[r.termination..r.termination + 2],
        &tag(1000, 2)[..],
        "the offset must land on the tag's first byte"
    );
}

/// Test 11 (S13). Cardinality runs at the termination point, not at EOF: a
/// `required` field that would have appeared after the boundary is genuinely
/// absent from the record that ended.
#[test]
fn test_scan_cardinality_applied_at_termination() {
    let g = compile_and_load(&scan_merged(
        vec![
            uint32(1, FieldLabel::Optional),
            uint32(5, FieldLabel::Required),
        ],
        &[],
    ));

    let mut pb = field_varint(1, 1); // 0..2
    pb.extend(field_varint(1, 2)); // the next record starts here
    pb.extend(field_varint(5, 3)); // the required field, beyond the boundary

    let r = score_entry_opts(&pb, &g, "Rec", &scan_opts());
    assert_eq!(r.termination, 2);
    assert_eq!(
        r.mismatches, 1,
        "the required field lies past the boundary, so the record lacks it"
    );
    assert_eq!(r.matches, 1);

    // Deferring the pass to EOF would have found field 5 present and charged
    // nothing — which is exactly what `Score` does, since it never stops.
    let s = score_entry(&pb, &g, "Rec");
    assert_eq!(s.mismatches, 0);
    assert_eq!(s.matches, 3);
}

/// Test 12 (S13). Termination is recorded, not obeyed: two roots whose rules
/// fire at different offsets are both scored correctly in a single pass. A
/// walk that halted at the first termination would truncate the other root.
#[test]
fn test_scan_roots_terminate_independently() {
    let mut states = std::collections::HashMap::new();
    states.insert(
        "RecA".to_string(),
        vec![
            uint32(1, FieldLabel::Optional),
            uint32(2, FieldLabel::Optional),
        ],
    );
    states.insert(
        "RecB".to_string(),
        vec![
            uint32(1, FieldLabel::Optional),
            uint32(2, FieldLabel::Repeated),
        ],
    );
    let g = compile_and_load(&Merged {
        states,
        node_kinds: std::collections::HashMap::new(),
        roots: vec!["RecA".to_string(), "RecB".to_string()],
        ext_ranges: std::collections::HashMap::new(),
        has_extension_ranges: true,
    });

    let mut pb = field_varint(1, 1); // 0..2
    pb.extend(field_varint(2, 2)); // 2..4
    pb.extend(field_varint(2, 3)); // 4..6 — a second singular 2 for RecA
    pb.extend(field_varint(1, 4)); // 6..8 — a second singular 1 for RecB

    let results = walk::score_all(&pb, &g, &scan_opts());
    let a = results.iter().find(|r| r.fqdn == "RecA").expect("RecA");
    let b = results.iter().find(|r| r.fqdn == "RecB").expect("RecB");

    assert_eq!((a.termination, a.matches), (4, 2));
    assert_eq!((b.termination, b.matches), (6, 3));
}

/// Test 13 (S12 rule 1, S15, S16). A custom option inside a declared range
/// runs to the end, costs nothing, and — carrying bytes that are not valid
/// UTF-8 — does not veto. The UTF-8 veto sits behind `is_string`, which an
/// unknown field never reaches; this pins that, because S15 is the first
/// change to make unknown-field handling policy-dependent.
#[test]
fn test_scan_does_not_terminate_on_custom_option() {
    let g = compile_and_load(&scan_merged(
        vec![string(1, FieldLabel::Optional)],
        &[(1000, 536_870_911)],
    ));

    let mut pb = field_len(1, b"a");
    pb.extend(field_len(1234, &[0xff, 0xfe]));

    let r = score_entry_opts(&pb, &g, "Rec", &scan_opts());
    assert!(!r.vetoed, "a custom option of unknown type must not veto");
    assert_eq!(r.termination, pb.len());
    assert_eq!(r.matches, 1);
    assert_eq!(r.unknowns, 0);
}

/// Test 14 (G4). `Score` is untouched by any of the above, including on a
/// graph that carries range data: `termination` is `pb.len()` and the
/// counters are what they were before the policy existed.
#[test]
fn test_score_policy_output_unchanged() {
    let g = compile_and_load(&scan_merged(
        vec![
            uint32(1, FieldLabel::Optional),
            uint32(2, FieldLabel::Repeated),
        ],
        &[(1000, 2000)],
    ));

    let mut pb = field_varint(1, 1);
    pb.extend(field_varint(2, 2));
    pb.extend(field_varint(1, 3)); // would terminate under `Scan`
    pb.extend(field_varint(1500, 4)); // in range, would be free under `Scan`
    pb.extend(field_varint(9, 5)); // out of range, would terminate under `Scan`

    let s = score_entry(&pb, &g, "Rec");
    assert_eq!(s.termination, pb.len());
    assert_eq!(s.matches, 3);
    assert_eq!(s.unknowns, 2, "G4: an unknown is an unknown under `Score`");
    assert_eq!(s.non_canonical, 1, "field 1 twice");
    assert!(!s.vetoed);
}

// ── Spec 0310: a cut range scores what it has ────────────────────────────────

fn cut_opts() -> walk::ScoringOpts {
    walk::ScoringOpts {
        end_undeclared: true,
        ..Default::default()
    }
}

/// Spec 0310 test 1. The same blob, cut at its tail, read twice: with the
/// end declared it is a lie and every candidate dies; with the end
/// undeclared it is a capture that ran out and keeps its evidence.
///
/// The score difference from the untruncated original is exactly the −5
/// charge plus the matches the cut removed — which is what makes the
/// coefficient a charge for being provisional rather than a charge for
/// the missing bytes (spec 0310 N5).
#[test]
fn a_cut_tail_scores_instead_of_vetoing() {
    let g = build_graph();

    let mut whole = field_varint(1, 1); // id (required)
    whole.extend(field_len(2, b"hello")); // name
    whole.extend(field_len(4, &field_varint(1, 42))); // child

    let intact = score_entry(&whole, &g, "Outer");
    assert!(!intact.vetoed);
    assert_eq!(intact.truncated, 0);
    assert_eq!(intact.matches, 4, "three outer fields and `child.value`");

    // Drop the last two bytes of `child`'s payload, so its declared length
    // now runs past the end of what is there.
    let cut = &whole[..whole.len() - 2];

    let declared = score_entry(cut, &g, "Outer");
    assert!(
        declared.vetoed,
        "with the end declared, an overrun is a length that lied"
    );

    let ran_out = score_entry_opts(cut, &g, "Outer", &cut_opts());
    assert!(!ran_out.vetoed, "a cut capture is not a wrong file");
    assert!(ran_out.truncated > 0);
    assert_eq!(
        ran_out.matches, 2,
        "the two fields before the cut still count; `child` does not"
    );
    assert_eq!(
        ran_out.score(),
        intact.score() - 5 - 2,
        "five for being provisional, two for the matches the cut removed"
    );
}

/// Spec 0310 test 2 / G2. The flag says the *outermost* range ran out. A
/// nested length that overruns its parent's declared end is a frame
/// contradicting itself, and still vetoes — otherwise the demotion would
/// have swallowed a veto that is about impossibility, not about capture.
#[test]
fn only_the_ran_out_sites_are_demoted() {
    let g = build_graph();

    // `child` declares 4 bytes but its payload holds a nested LEN whose own
    // length claims 40. The outer message is complete: its own declared
    // length fits inside the buffer with room to spare.
    let liar = [tag(1, 2), varint(40)].concat();
    let mut inner = liar.clone();
    inner.resize(4, 0);
    let mut pb = field_varint(1, 1);
    pb.extend(field_len(4, &inner));
    pb.extend(field_varint(3, 7)); // a whole field after the lie

    let s = score_entry_opts(&pb, &g, "Outer", &cut_opts());
    assert!(
        s.vetoed,
        "a length that overruns a declared parent end is still a veto"
    );
    assert_eq!(s.truncated, 0);
}

/// Spec 0310 S5. The cut frame runs no cardinality pass, so a `required`
/// field the cut removed is not charged as absent. Without the rule this
/// is −30 and the demotion recovers nothing.
#[test]
fn a_cut_frame_charges_no_absent_required_field() {
    let g = build_graph();

    // `Outer.id` is required and is written last, so the cut removes it.
    let mut pb = field_len(2, b"hello");
    pb.extend(field_len(4, &field_varint(1, 42)));
    pb.extend(field_varint(1, 1));
    let cut = &pb[..pb.len() - 4];

    let s = score_entry_opts(cut, &g, "Outer", &cut_opts());
    assert!(!s.vetoed);
    assert!(s.truncated > 0);
    assert_eq!(
        s.mismatches, 0,
        "a frame that did not end cannot report a field as absent"
    );
}

/// Spec 0310 N2. `Scan` reads an overrun as "this candidate root does not
/// start here", and owns the termination-offset contract a demoted overrun
/// would have to satisfy. The two options are not combinable.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "end_undeclared is not defined under Policy::Scan")]
fn scan_policy_refuses_the_flag() {
    let g = compile_and_load(&scan_merged(vec![uint32(1, FieldLabel::Optional)], &[]));
    let opts = walk::ScoringOpts {
        end_undeclared: true,
        ..scan_opts()
    };
    let _ = score_entry_opts(&field_varint(1, 1), &g, "Rec", &opts);
}

/// Spec 0310 S2, the three varint sites. `VarintResult::garbage` flattens
/// "ran out" together with "still going after ten bytes", and `next_pos`
/// does not separate them either — the overflow path forces it to `buflen`
/// as well. `varint_ran_out` is what separates them, and this is the case
/// that proves it does: an overlong varint that met its terminator, with
/// bytes still to come, must still veto.
#[test]
fn a_varint_overflow_before_the_end_is_not_a_cut() {
    let g = build_graph();

    let mut pb = tag(1, 0);
    pb.extend([0xFF; 11]); // eleven continuation bytes: overflow, not a cut
    pb.extend(field_varint(3, 7)); // and bytes after it, so it is not the tail

    let s = score_entry_opts(&pb, &g, "Outer", &cut_opts());
    assert!(s.vetoed, "an overlong varint is impossible, not incomplete");
    assert_eq!(s.truncated, 0);
}

// ── Spec 0313: a record ends at its last clean boundary ──────────────────────

/// Spec 0313 test 8 (S3). The rule is one rule: an anomaly that merely
/// counts ends the reading exactly as a veto does, and the entry reports the
/// boundary before it.
///
/// The anomaly is an enum value outside its declared range, chosen because
/// `scan_terminates` cannot foresee it — the lookahead reads the tag, and
/// this is a fact about the *value*. What follows it is a wire type of 7,
/// which no walk that reached it could survive; that the result is not
/// vetoed is how "the bytes beyond are never read" is observed.
#[test]
fn an_anomaly_stops_the_walk() {
    let g = compile_and_load(&scan_merged(
        vec![
            uint32(1, FieldLabel::Optional),
            ScoringField {
                number: 2,
                kind: ScoringKind::Range,
                child: None,
                range: Some((0, 2)),
                label: FieldLabel::Optional,
            },
        ],
        &[],
    ));

    let mut pb = field_varint(1, 1); // 0..2 — the last clean boundary
    pb.extend(field_varint(2, 99)); // 2..4 — outside (0, 2)
    pb.extend(tag(3, 7)); // an impossible wire type, never reached

    let r = score_entry_opts(&pb, &g, "Rec", &scan_opts());
    assert_eq!(r.termination, 2, "the boundary before the anomaly");
    assert_eq!(r.matches, 1, "the snapshot's count, not the walk's");
    assert_eq!(r.out_of_range, 0, "the anomaly is behind the boundary");
    assert!(!r.vetoed, "the walk stopped before the impossible tag");

    // The tail really is poisonous: `Score`, which reads on, chokes on it.
    assert!(score_entry(&pb, &g, "Rec").vetoed);
}

/// Spec 0313 test 6 / N1. `EntryScore::truncated` has no site to be set from
/// under `Policy::Scan`: `end_undeclared` is refused there (spec 0310 N2), so
/// every `cut_or_veto` takes its `veto_all` arm. A record whose last field
/// runs off the end therefore reports the clean prefix with the flag clear —
/// which is why S2's definition of clean does not name it.
#[test]
fn a_scan_never_sets_truncated() {
    let g = compile_and_load(&scan_merged(
        vec![
            uint32(1, FieldLabel::Optional),
            string(2, FieldLabel::Optional),
        ],
        &[],
    ));

    let mut pb = field_varint(1, 1); // 0..2 — the last clean boundary
    pb.extend(field_len(2, b"hello")); // declares five bytes...
    pb.truncate(pb.len() - 2); // ...and leaves three

    let r = score_entry_opts(&pb, &g, "Rec", &scan_opts());
    assert_eq!(r.truncated, 0, "unreachable under `Scan`, by N1");
    assert!(!r.vetoed, "an overrun reports the clean prefix instead");
    assert_eq!(r.termination, 2);
    assert_eq!(r.matches, 1);
}

// ── Spec 0324: a message with no fields is still a message ───────────────────

/// `Rec { optional Nothing a = 1; optional bytes b = 2; }` where `Nothing`
/// declares no field at all — the shape of `google.protobuf.Empty`, and the
/// one the walk used to read as a `bytes` leaf because a zero-field message
/// has no outgoing transition to be recognized by. Field 2 is a real `bytes`
/// leaf, so the graph holds both of the states this spec exists to tell
/// apart.
fn zero_field_child_merged() -> Merged {
    let mut states = std::collections::HashMap::new();
    states.insert(
        "Rec".to_string(),
        vec![
            ScoringField {
                number: 1,
                kind: ScoringKind::Node,
                child: Some("Nothing".to_string()),
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
    states.insert("Nothing".to_string(), vec![]);
    Merged {
        states,
        node_kinds: std::collections::HashMap::new(),
        roots: vec!["Rec".to_string()],
        ..Default::default()
    }
}

/// Spec 0324 test 1 (G1, S3): the predicate alone, isolated from any veto. A
/// clean unknown tag inside the zero-field field is a field the candidate
/// does not declare, so it costs `unknowns`. Before the fix the walk never
/// entered, and the payload scored `matches: 1` — a perfect reading of bytes
/// it had not looked at.
#[test]
fn a_zero_field_message_is_walked() {
    let g = compile_and_load(&zero_field_child_merged());
    let pb = field_len(1, &field_varint(7, 1));

    let s = score_entry(&pb, &g, "Rec");
    assert!(!s.vetoed, "a clean unknown tag is scored, not vetoed");
    assert_eq!(s.unknowns, 1, "field 7 is not declared by `Nothing`");
    assert_eq!(
        s.matches, 1,
        "the field itself still matches; only its contents are charged"
    );
}

/// Spec 0324 test 2 (G1): the Background's A/B, as an assertion. Wire type 7
/// is one no tag may carry, so it vetoes under a type with fields; it must
/// veto under a type without them too.
#[test]
fn a_fault_in_a_zero_field_message_vetoes_its_parent() {
    let g = compile_and_load(&zero_field_child_merged());
    let pb = field_len(1, &[0x0f]); // field 1, wire type 7

    assert!(
        score_entry(&pb, &g, "Rec").vetoed,
        "an impossible wire type inside `Nothing` disqualifies `Rec`"
    );
}

/// Spec 0324 test 3: the reported case. An unclosed group has no length
/// prefix to rescue it, so it is a fault of the frame it opens in — and the
/// veto must reach the *root* entry, which is what makes this
/// `propagate_vetoes` rather than a verdict local to the child walk.
#[test]
fn an_open_group_in_a_zero_field_message_vetoes() {
    let g = compile_and_load(&zero_field_child_merged());
    let pb = field_len(1, &tag(14, 3)); // START_GROUP, never closed

    assert!(
        score_entry(&pb, &g, "Rec").vetoed,
        "the open group must reach `Rec`, not stop at `Nothing`"
    );
}

/// Spec 0324 test 4: the common, legitimate case, and what stops the fix from
/// turning every `Empty` in a corpus into a penalty. A present-but-empty
/// zero-field message has nothing inside to charge.
#[test]
fn an_empty_zero_field_message_still_matches() {
    let g = compile_and_load(&zero_field_child_merged());
    let pb = field_len(1, &[]);

    let s = score_entry(&pb, &g, "Rec");
    assert!(!s.vetoed);
    assert_eq!(s.matches, 1);
    assert_eq!(s.unknowns, 0);
    assert_eq!(s.score(), 1, "an `Empty` costs nothing to read");
}

/// Spec 0324 test 5 (S2): the normalization, pinned where it would otherwise
/// break silently. `NodeEntry::wire_type` may say 10; `child_wire_type` may
/// not, because the walk compares it against a tag's wire type — a value no
/// tag can carry would make every message field a mismatch.
#[test]
fn a_message_node_is_not_a_bytes_leaf() {
    let merged = zero_field_child_merged();
    let (raw, reg) = graph::build(&merged);
    let partition = hopcroft::minimize(&raw, &reg, &raw.node_wire_types, |_, _| {});
    let compiled = graph::compile(&raw, &reg, &partition, &merged.roots);

    // Followed edge by edge from the root rather than counted, because the
    // node table also holds the reserved `Any` and `MessageSet` states
    // (spec 0089 §9) and they are message states too.
    let rec = compiled.roots[0].state_id;
    let child_of = |field: u32| {
        let t = compiled
            .transitions
            .iter()
            .find(|t| t.state_id == rec && t.field_number == field)
            .expect("the field is declared");
        let n = compiled
            .nodes
            .iter()
            .find(|n| n.state_id == t.child_state_id)
            .expect("the child has a node entry");
        (t.child_wire_type, n.wire_type, n.is_string, n.trans_len)
    };

    assert_eq!(
        child_of(1),
        (2, serial::WT_NODE_MESSAGE, false, 0),
        "`Nothing` is a message with no transitions, and its edge says LEN"
    );
    assert_eq!(
        child_of(2),
        (2, 2, false, 0),
        "the `bytes` leaf it used to be indistinguishable from is unchanged"
    );
    assert!(
        compiled
            .transitions
            .iter()
            .all(|t| t.child_wire_type != serial::WT_NODE_MESSAGE),
        "no edge may carry the internal discriminant"
    );
}
