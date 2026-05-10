// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Direct scoring walk over the protobuf wire format.
//!
//! Replicates the exact logic of prototext-core's `parse_message` /
//! `decode_len_field` / `decode_packed_varints`, but emits scoring signals
//! directly instead of building a ProtoTextMessage tree.
//!
//! Scoring rules (spec 0042 §3):
//!   Veto    — wire parse error, wire-type/proto-type mismatch on a declared
//!             field, invalid UTF-8 on a string field, varint outside enum
//!             range, invalid packed records, invalid group end, open-ended
//!             group, mismatched group end field number.
//!   Match   — field number present in current state, wire content compatible.
//!   Unknown — field number absent from current state (no schema for field).
//!
//! Non-canonical encodings (overhang bytes on varints, out-of-range field
//! numbers) are noted but do not veto by themselves — they score as match or
//! unknown per the field presence rule, with `non_canonical` incremented so
//! callers can apply a quality penalty.

use crate::build_scoring_graph::serial::ArchivedCompiledGraph;

// ── Wire-type constants (mirrors prototext-core/src/helpers/wire.rs) ──────────

const WT_VARINT: u32 = 0;
const WT_I64: u32 = 1;
const WT_LEN: u32 = 2;
const WT_START_GROUP: u32 = 3;
const WT_END_GROUP: u32 = 4;
const WT_I32: u32 = 5;

// ── Public types ──────────────────────────────────────────────────────────────

pub struct MatchScore {
    pub matches: u64,
    pub unknowns: u64,
    pub non_canonical: u64, // overhang bytes, out-of-range field numbers, truncated negatives
    pub vetoed: bool,
}

impl MatchScore {
    /// Integer score: matches×1 + unknowns×(−10) + non_canonical×(−20).
    pub fn score(&self) -> i64 {
        self.matches as i64 - 10 * self.unknowns as i64 - 20 * self.non_canonical as i64
    }

    fn veto(&mut self) {
        self.vetoed = true;
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn score(pb: &[u8], root_state: u32, graph: &ArchivedCompiledGraph) -> MatchScore {
    let mut s = MatchScore {
        matches: 0,
        unknowns: 0,
        non_canonical: 0,
        vetoed: false,
    };
    score_message(pb, root_state, None, graph, &mut s);
    s
}

// ── Varint parser (mirrors parse_varint in prototext-core) ────────────────────

struct VarintResult {
    next_pos: usize,
    /// Some(raw) when truncated or overflowed.
    garbage: Option<()>,
    value: u64,
    /// Number of non-canonical overhang bytes.
    overhang: u64,
}

fn parse_varint(buf: &[u8], start: usize) -> VarintResult {
    let buflen = buf.len();
    if start == buflen {
        return VarintResult {
            next_pos: start,
            garbage: Some(()),
            value: 0,
            overhang: 0,
        };
    }

    let mut v: u64 = 0;
    let mut shift: u32 = 0;
    let mut pos = start;
    let mut too_big = false;

    loop {
        if pos >= buflen {
            return VarintResult {
                next_pos: buflen,
                garbage: Some(()),
                value: 0,
                overhang: 0,
            };
        }
        let b = buf[pos];
        pos += 1;
        let bits = (b & 0x7f) as u64;
        if shift < 64 {
            if shift == 63 && bits > 1 {
                too_big = true;
            } else {
                v |= bits << shift;
            }
        } else if bits != 0 {
            too_big = true;
        }
        shift += 7;
        if b & 0x80 == 0 {
            break;
        }
        if shift > 70 {
            // Absurdly long varint — consume continuation bytes.
            while pos < buflen {
                let b2 = buf[pos];
                pos += 1;
                if (b2 & 0x7f) != 0 {
                    too_big = true;
                }
                if b2 & 0x80 == 0 {
                    break;
                }
            }
            break;
        }
    }

    if too_big {
        return VarintResult {
            next_pos: buflen,
            garbage: Some(()),
            value: 0,
            overhang: 0,
        };
    }

    // Count overhang bytes: terminator 0x00 preceded by 0x80 bytes.
    let last_b = buf[pos - 1];
    let ohb = if last_b == 0x00 && pos > start + 1 {
        let mut count: u64 = 1;
        let mut p = pos - 2;
        while p > start && buf[p] == 0x80 {
            count += 1;
            p -= 1;
        }
        count
    } else {
        0
    };

    VarintResult {
        next_pos: pos,
        garbage: None,
        value: v,
        overhang: ohb,
    }
}

// ── Wire-tag parser (mirrors parse_wiretag in prototext-core) ─────────────────

struct TagResult {
    next_pos: usize,
    /// Some when wire type > 5 or varint truncated/overflowed.
    garbage: Option<()>,
    wire_type: u32,
    field_number: u64,
    overhang: u64,
    /// True when field number is 0 or >= 2^29.
    out_of_range: bool,
}

fn parse_wiretag(buf: &[u8], start: usize) -> TagResult {
    let buflen = buf.len();
    debug_assert!(start < buflen);

    let first = buf[start];
    let wtype = (first & 0x07) as u32;
    if wtype > 5 {
        // Invalid wire type — consume rest of buffer as garbage.
        return TagResult {
            next_pos: buflen,
            garbage: Some(()),
            wire_type: 0,
            field_number: 0,
            overhang: 0,
            out_of_range: false,
        };
    }

    let vr = parse_varint(buf, start);
    if vr.garbage.is_some() {
        return TagResult {
            next_pos: vr.next_pos,
            garbage: Some(()),
            wire_type: 0,
            field_number: 0,
            overhang: 0,
            out_of_range: false,
        };
    }

    let raw = vr.value;
    let field_number = raw >> 3;
    let oor = field_number == 0 || field_number >= (1 << 29);

    TagResult {
        next_pos: vr.next_pos,
        garbage: None,
        wire_type: wtype,
        field_number,
        overhang: vr.overhang,
        out_of_range: oor,
    }
}

// ── Group skip (mirrors prototext-core's group handling in parse_message) ──────
//
// Returns `Some(new_pos)` after the matching END_GROUP tag, or `None` on error.

fn skip_group(buf: &[u8], mut pos: usize, expected_field: u64) -> Option<usize> {
    let buflen = buf.len();
    loop {
        if pos == buflen {
            return None; // open-ended group
        }
        let tag = parse_wiretag(buf, pos);
        if tag.garbage.is_some() {
            return None;
        }
        pos = tag.next_pos;
        match tag.wire_type {
            WT_VARINT => {
                let vr = parse_varint(buf, pos);
                if vr.garbage.is_some() {
                    return None;
                }
                pos = vr.next_pos;
            }
            WT_I64 => {
                if pos + 8 > buflen {
                    return None;
                }
                pos += 8;
            }
            WT_LEN => {
                let vr = parse_varint(buf, pos);
                if vr.garbage.is_some() {
                    return None;
                }
                pos = vr.next_pos;
                let len = vr.value as usize;
                if pos + len > buflen {
                    return None;
                }
                pos += len;
            }
            WT_START_GROUP => {
                pos = skip_group(buf, pos, tag.field_number)?;
            }
            WT_END_GROUP => {
                if tag.field_number != expected_field {
                    return None; // mismatched group end
                }
                return Some(pos);
            }
            WT_I32 => {
                if pos + 4 > buflen {
                    return None;
                }
                pos += 4;
            }
            _ => return None,
        }
    }
}

// ── Schema lookup ─────────────────────────────────────────────────────────────
//
// The transition table is sorted by (state_id, field_number).  A single binary
// search finds the transition for the given (state, field_number).  Whether the
// stream wire type matches is determined by looking up the child node's
// wire_type in the node table (also sorted by state_id).

/// Result of looking up a (state, field_number) pair.
enum SchemaVerdict {
    /// Field number not declared in this state.
    Unknown,
    /// Field number declared; child node's wire_type does not match the stream.
    WireTypeMismatch,
    /// Match: child_state_id of the found transition.
    Found(u32),
}

fn find_transition(graph: &ArchivedCompiledGraph, state: u32, field_number: u32) -> Option<u32> {
    let t = &graph.transitions;
    let mut lo = 0usize;
    let mut hi = t.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let ts = t[mid].state_id.to_native();
        let tf = t[mid].field_number.to_native();
        if ts < state || (ts == state && tf < field_number) {
            lo = mid + 1;
        } else if ts == state && tf == field_number {
            return Some(t[mid].child_state_id.to_native());
        } else {
            hi = mid;
        }
    }
    None
}

fn node_wire_type(graph: &ArchivedCompiledGraph, state_id: u32) -> u8 {
    let n = &graph.nodes;
    let mut lo = 0usize;
    let mut hi = n.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let ns = n[mid].state_id.to_native();
        if ns < state_id {
            lo = mid + 1;
        } else if ns == state_id {
            return n[mid].wire_type;
        } else {
            hi = mid;
        }
    }
    // Should never happen for a well-formed graph.
    u8::MAX
}

fn schema_verdict(
    graph: &ArchivedCompiledGraph,
    state: u32,
    field_number: u32,
    stream_wire_type: u32,
) -> SchemaVerdict {
    match find_transition(graph, state, field_number) {
        None => SchemaVerdict::Unknown,
        Some(child) => {
            let expected_wt = node_wire_type(graph, child) as u32;
            if stream_wire_type == expected_wt {
                SchemaVerdict::Found(child)
            } else {
                SchemaVerdict::WireTypeMismatch
            }
        }
    }
}

// ── Core recursive scoring walk ───────────────────────────────────────────────
//
// Structure: for each field,
//   1. Parse the wire tag.
//   2. Determine schema verdict by field number (Unknown / WireTypeMismatch / Found(child)).
//      WireTypeMismatch vetoes before body consumption.
//   3. Consume the body (dispatch on stream wire_type for structural validity).
//   4. For Found(child): apply node-level checks (enum range, UTF-8, recursion).

fn score_message(
    buf: &[u8],
    state: u32,
    my_group: Option<u64>,
    graph: &ArchivedCompiledGraph,
    s: &mut MatchScore,
) {
    let buflen = buf.len();
    let mut pos = 0;

    loop {
        if pos == buflen || s.vetoed {
            return;
        }

        // ── Parse wire tag ────────────────────────────────────────────────────

        let tag = parse_wiretag(buf, pos);
        if tag.garbage.is_some() {
            s.veto();
            return;
        }
        let field_number = tag.field_number;
        let wire_type = tag.wire_type;
        pos = tag.next_pos;
        if tag.overhang > 0 {
            s.non_canonical += 1;
        }
        if tag.out_of_range {
            s.non_canonical += 1;
        }

        // ── Schema verdict ────────────────────────────────────────────────────

        let verdict = schema_verdict(graph, state, field_number as u32, wire_type);
        if matches!(verdict, SchemaVerdict::WireTypeMismatch) {
            s.veto();
            return;
        }

        // ── Wire-type dispatch ────────────────────────────────────────────────

        match wire_type {
            // ── VARINT ───────────────────────────────────────────────────────
            WT_VARINT => {
                let vr = parse_varint(buf, pos);
                if vr.garbage.is_some() {
                    s.veto();
                    return;
                }
                pos = vr.next_pos;
                let val = vr.value;
                if vr.overhang > 0 {
                    s.non_canonical += 1;
                }
                match verdict {
                    SchemaVerdict::Unknown => s.unknowns += 1,
                    SchemaVerdict::Found(child) => {
                        // Check for ENUM range if the child node has enum_range_idx set.
                        let node = graph.nodes.iter().find(|n| n.state_id.to_native() == child);
                        if let Some(n) = node {
                            let eri = n.enum_range_idx.to_native();
                            if eri != 0xFFFF {
                                if val >= (1u64 << 32) {
                                    s.veto();
                                    return;
                                }
                                // Truncated negative: val in [0x80000000, 0xFFFFFFFF] is a
                                // 5-byte non-canonical encoding of a negative int32/enum.
                                if val >= 0x8000_0000 && val <= 0xFFFF_FFFF {
                                    s.non_canonical += 1;
                                }
                                if let Some(range) = graph.enum_ranges.get(eri as usize) {
                                    let (min, max) =
                                        (range.0.to_native() as i64, range.1.to_native() as i64);
                                    let signed = val as i32 as i64;
                                    if signed < min || signed > max {
                                        s.veto();
                                        return;
                                    }
                                }
                            }
                        }
                        s.matches += 1;
                    }
                    SchemaVerdict::WireTypeMismatch => unreachable!(),
                }
            }

            // ── FIXED 64 ─────────────────────────────────────────────────────
            WT_I64 => {
                if pos + 8 > buflen {
                    s.veto();
                    return;
                }
                pos += 8;
                match verdict {
                    SchemaVerdict::Unknown => s.unknowns += 1,
                    SchemaVerdict::Found(_) => s.matches += 1,
                    SchemaVerdict::WireTypeMismatch => unreachable!(),
                }
            }

            // ── LENGTH-DELIMITED ─────────────────────────────────────────────
            WT_LEN => {
                let lr = parse_varint(buf, pos);
                if lr.garbage.is_some() {
                    s.veto();
                    return;
                }
                pos = lr.next_pos;
                if lr.overhang > 0 {
                    s.non_canonical += 1;
                }
                let length = lr.value as usize;
                if pos + length > buflen {
                    s.veto();
                    return;
                }
                let payload = &buf[pos..pos + length];
                pos += length;

                match verdict {
                    SchemaVerdict::Unknown => s.unknowns += 1,
                    SchemaVerdict::Found(child) => {
                        // Check child node attributes.
                        let node = graph.nodes.iter().find(|n| n.state_id.to_native() == child);
                        let is_leaf_node = graph
                            .transitions
                            .iter()
                            .all(|t| t.state_id.to_native() != child);
                        if is_leaf_node {
                            // Leaf LEN node: check is_string.
                            if node.map_or(false, |n| n.is_string) {
                                if std::str::from_utf8(payload).is_err() {
                                    s.veto();
                                    return;
                                }
                            }
                            s.matches += 1;
                        } else {
                            // Non-leaf (message) node: recurse.
                            s.matches += 1;
                            score_message(payload, child, None, graph, s);
                        }
                    }
                    SchemaVerdict::WireTypeMismatch => unreachable!(),
                }
            }

            // ── START GROUP ──────────────────────────────────────────────────
            WT_START_GROUP => {
                match skip_group(buf, pos, field_number) {
                    None => {
                        s.veto();
                        return;
                    }
                    Some(new_pos) => pos = new_pos,
                }
                match verdict {
                    SchemaVerdict::Unknown => s.unknowns += 1,
                    SchemaVerdict::Found(_) => s.matches += 1,
                    SchemaVerdict::WireTypeMismatch => unreachable!(),
                }
            }

            // ── END GROUP ────────────────────────────────────────────────────
            WT_END_GROUP => match my_group {
                None => {
                    s.veto();
                    return;
                }
                Some(expected) => {
                    if field_number != expected {
                        s.veto();
                        return;
                    }
                    return;
                }
            },

            // ── FIXED 32 ─────────────────────────────────────────────────────
            WT_I32 => {
                if pos + 4 > buflen {
                    s.veto();
                    return;
                }
                pos += 4;
                match verdict {
                    SchemaVerdict::Unknown => s.unknowns += 1,
                    SchemaVerdict::Found(_) => s.matches += 1,
                    SchemaVerdict::WireTypeMismatch => unreachable!(),
                }
            }

            _ => unreachable!("wire type > 5 caught by parse_wiretag"),
        }
    }
}
