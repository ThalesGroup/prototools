// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Direct scoring walk over the protobuf wire format.
//!
//! Two entry points are provided:
//!
//! - `score(pb, root_state, graph)` — single-entry walk (spec 0042).
//! - `score_all(pb, graph)` — multi-entry parallel walk (spec 0048): scores
//!   all root entries in the compiled graph simultaneously in one traversal.
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

use smallvec::SmallVec;

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
    pub mismatches: u64,    // absent required fields
    pub non_canonical: u64, // overhang bytes, out-of-range field numbers, truncated negatives
    pub vetoed: bool,
}

impl MatchScore {
    /// Integer score: matches×1 + unknowns×(−10) + mismatches×(−10) + non_canonical×(−20).
    pub fn score(&self) -> i64 {
        self.matches as i64
            - 10 * self.unknowns as i64
            - 10 * self.mismatches as i64
            - 20 * self.non_canonical as i64
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
        mismatches: 0,
        non_canonical: 0,
        vetoed: false,
    };
    score_message(pb, 0, root_state, None, graph, &mut s);
    s
}

// ── Multi-entry types (spec 0048) ─────────────────────────────────────────────

/// Per-entry scoring counters for the multi-entry walk.
pub struct EntryScore {
    pub fqdn: String,
    pub matches: u64,
    pub unknowns: u64,
    pub mismatches: u64,
    pub non_canonical: u64,
    pub vetoed: bool,
}

impl EntryScore {
    pub fn score(&self) -> i64 {
        self.matches as i64
            - 10 * self.unknowns as i64
            - 10 * self.mismatches as i64
            - 20 * self.non_canonical as i64
    }
}

/// One entry in the active set: a state_id shared by one or more entry indices.
///
/// Invariant: `entries` is never empty (an ActiveEntry is removed when its
/// last entry index is vetoed).
struct ActiveEntry {
    state_id: u32,
    /// Entry indices (into `WalkState::scores`) routing through this state
    /// at the current nesting level.  SmallVec avoids heap allocation for the
    /// common case of few entries per state.
    entries: SmallVec<[u16; 4]>,
    /// Per-frame occurrence counts for fields that received a Found verdict.
    /// field_number → how many times seen in this message/group frame.
    occurrences: Vec<(u32, u64)>, // sorted by field_number
}

/// Global walk state shared across all recursion levels.
struct WalkState<'a> {
    graph: &'a ArchivedCompiledGraph,
    scores: &'a mut Vec<EntryScore>,
    /// Flat bitset: bit i is set iff entry i has been permanently vetoed.
    vetoed: Vec<u64>,
}

impl<'a> WalkState<'a> {
    fn new(graph: &'a ArchivedCompiledGraph, scores: &'a mut Vec<EntryScore>) -> Self {
        let n = scores.len();
        let words = n.div_ceil(64);
        WalkState {
            graph,
            scores,
            vetoed: vec![0u64; words],
        }
    }

    fn is_vetoed(&self, e: u16) -> bool {
        let e = e as usize;
        (self.vetoed[e / 64] >> (e % 64)) & 1 == 1
    }

    fn set_vetoed(&mut self, e: u16) {
        let e = e as usize;
        self.vetoed[e / 64] |= 1 << (e % 64);
        self.scores[e].vetoed = true;
    }
}

/// Group entries by their state_id, producing one `ActiveEntry` per distinct state.
fn group_by_state(pairs: impl Iterator<Item = (u32, u16)>) -> Vec<ActiveEntry> {
    let mut v: Vec<(u32, u16)> = pairs.collect();
    v.sort_unstable_by_key(|&(s, _)| s);
    let mut result = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let state_id = v[i].0;
        let mut entries = SmallVec::new();
        while i < v.len() && v[i].0 == state_id {
            entries.push(v[i].1);
            i += 1;
        }
        result.push(ActiveEntry {
            state_id,
            entries,
            occurrences: Vec::new(),
        });
    }
    result
}

/// Score all root entries in `graph` simultaneously against `pb`.
/// Returns one `EntryScore` per root entry, in graph order.
pub fn score_all(pb: &[u8], graph: &ArchivedCompiledGraph) -> Vec<EntryScore> {
    assert!(
        graph.roots.len() <= u16::MAX as usize,
        "entry count {} exceeds u16::MAX",
        graph.roots.len()
    );

    let mut scores: Vec<EntryScore> = graph
        .roots
        .iter()
        .map(|r| EntryScore {
            fqdn: r.fqdn.as_str().to_owned(),
            matches: 0,
            unknowns: 0,
            mismatches: 0,
            non_canonical: 0,
            vetoed: false,
        })
        .collect();

    let initial_active = group_by_state(
        graph
            .roots
            .iter()
            .enumerate()
            .map(|(i, r)| (r.state_id.to_native(), i as u16)),
    );

    let mut ws = WalkState::new(graph, &mut scores);
    score_message_multi(pb, 0, initial_active, None, &mut ws);

    scores
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

// ── Group blind-walk (mirrors prototext-core's group handling in parse_message) ─
//
// Full structural walk with no schema — used for Unknown-verdict groups and as
// fallback when all recurse_into entries are vetoed.
// Returns `Some(new_pos)` after the matching END_GROUP tag, or `None` on error.

fn parse_group_blind(buf: &[u8], mut pos: usize, expected_field: u64) -> Option<usize> {
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
                pos = parse_group_blind(buf, pos, tag.field_number)?;
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
    /// Match: child_state_id and label of the found transition.
    Found(u32, u8), // (child_state_id, label)
}

struct TransitionResult {
    child_state_id: u32,
    label: u8,
}

fn find_transition(
    graph: &ArchivedCompiledGraph,
    state: u32,
    field_number: u32,
) -> Option<TransitionResult> {
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
            return Some(TransitionResult {
                child_state_id: t[mid].child_state_id.to_native(),
                label: t[mid].label,
            });
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
        Some(tr) => {
            let expected_wt = node_wire_type(graph, tr.child_state_id) as u32;
            if stream_wire_type == expected_wt {
                SchemaVerdict::Found(tr.child_state_id, tr.label)
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

// ── Cardinality check helpers ─────────────────────────────────────────────────

/// Increment occurrences[field_number] by 1.  The vec is kept sorted.
fn record_occurrence(occurrences: &mut Vec<(u32, u64)>, field_number: u32) {
    match occurrences.binary_search_by_key(&field_number, |&(f, _)| f) {
        Ok(i) => occurrences[i].1 += 1,
        Err(i) => occurrences.insert(i, (field_number, 1)),
    }
}

/// Apply end-of-frame cardinality checks for `state` against `occurrences`.
fn apply_cardinality(
    graph: &ArchivedCompiledGraph,
    state: u32,
    occurrences: &[(u32, u64)],
    s: &mut MatchScore,
) {
    // Scan all transitions from `state`.
    let t = &graph.transitions;
    let start = t.partition_point(|e| e.state_id.to_native() < state);
    for entry in &t[start..] {
        if entry.state_id.to_native() != state {
            break;
        }
        let fn_ = entry.field_number.to_native();
        let count = occurrences
            .binary_search_by_key(&fn_, |&(f, _)| f)
            .map(|i| occurrences[i].1)
            .unwrap_or(0);
        match entry.label {
            0 => {
                // Optional: >1 is non-canonical
                if count > 1 {
                    s.non_canonical += count - 1;
                }
            }
            1 => {
                // Required: 0 is mismatch, >1 is non-canonical
                if count == 0 {
                    s.mismatches += 1;
                } else if count > 1 {
                    s.non_canonical += count - 1;
                }
            }
            _ => {} // Repeated: no constraint
        }
    }
}

fn score_message(
    buf: &[u8],
    start: usize,
    state: u32,
    my_group: Option<u64>,
    graph: &ArchivedCompiledGraph,
    s: &mut MatchScore,
) -> usize {
    let buflen = buf.len();
    let mut pos = start;
    let mut occurrences: Vec<(u32, u64)> = Vec::new();

    loop {
        if pos == buflen || s.vetoed {
            if !s.vetoed {
                if my_group.is_some() {
                    // Reached EOF while still inside a group — open-ended group → veto.
                    s.veto();
                    return buflen;
                }
                apply_cardinality(graph, state, &occurrences, s);
            }
            return pos;
        }

        // ── Parse wire tag ────────────────────────────────────────────────────

        let tag = parse_wiretag(buf, pos);
        if tag.garbage.is_some() {
            s.veto();
            return buflen;
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
            return buflen;
        }

        // ── Wire-type dispatch ────────────────────────────────────────────────

        match wire_type {
            // ── VARINT ───────────────────────────────────────────────────────
            WT_VARINT => {
                let vr = parse_varint(buf, pos);
                if vr.garbage.is_some() {
                    s.veto();
                    return buflen;
                }
                pos = vr.next_pos;
                let val = vr.value;
                if vr.overhang > 0 {
                    s.non_canonical += 1;
                }
                match verdict {
                    SchemaVerdict::Unknown => s.unknowns += 1,
                    SchemaVerdict::Found(child, _label) => {
                        let node = graph.nodes.iter().find(|n| n.state_id.to_native() == child);
                        if let Some(n) = node {
                            let eri = n.enum_range_idx.to_native();
                            if eri != 0xFFFF {
                                if val >= (1u64 << 32) {
                                    s.veto();
                                    return buflen;
                                }
                                if (0x8000_0000..=0xFFFF_FFFF).contains(&val) {
                                    s.non_canonical += 1;
                                }
                                if let Some(range) = graph.enum_ranges.get(eri as usize) {
                                    let (min, max) =
                                        (range.0.to_native() as i64, range.1.to_native() as i64);
                                    let signed = val as i32 as i64;
                                    if signed < min || signed > max {
                                        s.veto();
                                        return buflen;
                                    }
                                }
                            }
                        }
                        record_occurrence(&mut occurrences, field_number as u32);
                        s.matches += 1;
                    }
                    SchemaVerdict::WireTypeMismatch => unreachable!(),
                }
            }

            // ── FIXED 64 ─────────────────────────────────────────────────────
            WT_I64 => {
                if pos + 8 > buflen {
                    s.veto();
                    return buflen;
                }
                pos += 8;
                match verdict {
                    SchemaVerdict::Unknown => s.unknowns += 1,
                    SchemaVerdict::Found(_, _) => {
                        record_occurrence(&mut occurrences, field_number as u32);
                        s.matches += 1;
                    }
                    SchemaVerdict::WireTypeMismatch => unreachable!(),
                }
            }

            // ── LENGTH-DELIMITED ─────────────────────────────────────────────
            WT_LEN => {
                let lr = parse_varint(buf, pos);
                if lr.garbage.is_some() {
                    s.veto();
                    return buflen;
                }
                pos = lr.next_pos;
                if lr.overhang > 0 {
                    s.non_canonical += 1;
                }
                let length = lr.value as usize;
                if pos + length > buflen {
                    s.veto();
                    return buflen;
                }
                let payload = &buf[pos..pos + length];
                pos += length;

                match verdict {
                    SchemaVerdict::Unknown => s.unknowns += 1,
                    SchemaVerdict::Found(child, _label) => {
                        let node = graph.nodes.iter().find(|n| n.state_id.to_native() == child);
                        let is_leaf_node = graph
                            .transitions
                            .iter()
                            .all(|t| t.state_id.to_native() != child);
                        if is_leaf_node {
                            if node.is_some_and(|n| n.is_string)
                                && std::str::from_utf8(payload).is_err()
                            {
                                s.veto();
                                return buflen;
                            }
                            record_occurrence(&mut occurrences, field_number as u32);
                            s.matches += 1;
                        } else {
                            record_occurrence(&mut occurrences, field_number as u32);
                            s.matches += 1;
                            score_message(payload, 0, child, None, graph, s);
                        }
                    }
                    SchemaVerdict::WireTypeMismatch => unreachable!(),
                }
            }

            // ── START GROUP ──────────────────────────────────────────────────
            WT_START_GROUP => match verdict {
                SchemaVerdict::Unknown => {
                    match parse_group_blind(buf, pos, field_number) {
                        None => {
                            s.veto();
                            return buflen;
                        }
                        Some(new_pos) => pos = new_pos,
                    }
                    s.unknowns += 1;
                }
                SchemaVerdict::Found(child, _label) => {
                    record_occurrence(&mut occurrences, field_number as u32);
                    s.matches += 1;
                    pos = score_message(buf, pos, child, Some(field_number), graph, s);
                }
                SchemaVerdict::WireTypeMismatch => unreachable!(),
            },

            // ── END GROUP ────────────────────────────────────────────────────
            WT_END_GROUP => match my_group {
                None => {
                    s.veto();
                    return buflen;
                }
                Some(expected) => {
                    if field_number != expected {
                        s.veto();
                        return buflen;
                    }
                    apply_cardinality(graph, state, &occurrences, s);
                    return pos;
                }
            },

            // ── FIXED 32 ─────────────────────────────────────────────────────
            WT_I32 => {
                if pos + 4 > buflen {
                    s.veto();
                    return buflen;
                }
                pos += 4;
                match verdict {
                    SchemaVerdict::Unknown => s.unknowns += 1,
                    SchemaVerdict::Found(_, _) => {
                        record_occurrence(&mut occurrences, field_number as u32);
                        s.matches += 1;
                    }
                    SchemaVerdict::WireTypeMismatch => unreachable!(),
                }
            }

            _ => unreachable!("wire type > 5 caught by parse_wiretag"),
        }
    }
}

// ── Multi-entry parallel walk (spec 0048) ─────────────────────────────────────

/// Veto all entries in `active` and clear the set.
fn veto_all(active: &mut Vec<ActiveEntry>, ws: &mut WalkState) {
    for ae in active.iter() {
        for &e in &ae.entries {
            ws.set_vetoed(e);
        }
    }
    active.clear();
}

/// Remove newly-vetoed entries from every ActiveEntry in `active`, then drop
/// empty ActiveEntries.  Called after returning from a sub-message recursion.
fn propagate_vetoes(active: &mut Vec<ActiveEntry>, ws: &WalkState) {
    for ae in active.iter_mut() {
        ae.entries.retain(|e| !ws.is_vetoed(*e));
    }
    active.retain(|ae| !ae.entries.is_empty());
}

/// Apply end-of-frame cardinality checks for the multi-entry walk.
/// Called once per ActiveEntry at the end of each message/group frame.
fn apply_cardinality_multi(
    graph: &ArchivedCompiledGraph,
    ae: &ActiveEntry,
    scores: &mut [EntryScore],
) {
    let state = ae.state_id;
    let t = &graph.transitions;
    let start = t.partition_point(|e| e.state_id.to_native() < state);
    for entry in &t[start..] {
        if entry.state_id.to_native() != state {
            break;
        }
        let fn_ = entry.field_number.to_native();
        let count = ae
            .occurrences
            .binary_search_by_key(&fn_, |&(f, _)| f)
            .map(|i| ae.occurrences[i].1)
            .unwrap_or(0);
        match entry.label {
            0 => {
                if count > 1 {
                    for &e in &ae.entries {
                        scores[e as usize].non_canonical += count - 1;
                    }
                }
            }
            1 => {
                if count == 0 {
                    for &e in &ae.entries {
                        scores[e as usize].mismatches += 1;
                    }
                } else if count > 1 {
                    for &e in &ae.entries {
                        scores[e as usize].non_canonical += count - 1;
                    }
                }
            }
            _ => {} // Repeated: no constraint
        }
    }
}

fn score_message_multi(
    buf: &[u8],
    start: usize,
    mut active: Vec<ActiveEntry>,
    my_group: Option<u64>,
    ws: &mut WalkState,
) -> usize {
    let buflen = buf.len();
    let mut pos = start;

    // Verdict attached directly to each ActiveEntry for one field iteration.
    // Stored by state_id (stable across retain) rather than Vec index.
    // label is included so occurrences can be recorded only for Found.
    #[derive(Clone, Copy)]
    enum Verdict {
        Unknown,
        Mismatch,
        Found(u32, u8), // (child_state_id, label)
    }

    // Reusable per-field verdict buffer: (state_id, verdict).
    // Keyed by state_id so it remains valid after active.retain().
    let mut verdicts: Vec<(u32, Verdict)> = Vec::new();

    loop {
        if pos == buflen || active.is_empty() {
            if !active.is_empty() {
                if my_group.is_some() {
                    // Reached EOF while still inside a group — open-ended group → veto.
                    veto_all(&mut active, ws);
                    return buflen;
                }
                for ae in &active {
                    apply_cardinality_multi(ws.graph, ae, ws.scores);
                }
            }
            return pos;
        }

        // ── Parse wire tag ────────────────────────────────────────────────────

        let tag = parse_wiretag(buf, pos);
        if tag.garbage.is_some() {
            veto_all(&mut active, ws);
            return buflen;
        }
        let field_number = tag.field_number;
        let wire_type = tag.wire_type;
        pos = tag.next_pos;

        // ── Wire-level non-canonical penalties (all active entries) ───────────

        if tag.overhang > 0 || tag.out_of_range {
            for ae in &active {
                for &e in &ae.entries {
                    if tag.overhang > 0 {
                        ws.scores[e as usize].non_canonical += 1;
                    }
                    if tag.out_of_range {
                        ws.scores[e as usize].non_canonical += 1;
                    }
                }
            }
        }

        // ── Schema verdict per active-entry group ─────────────────────────────
        //
        // Keyed by state_id so lookups remain valid after active.retain().

        verdicts.clear();
        for ae in active.iter() {
            let v = match find_transition(ws.graph, ae.state_id, field_number as u32) {
                None => Verdict::Unknown,
                Some(tr) => {
                    let expected_wt = node_wire_type(ws.graph, tr.child_state_id) as u32;
                    if wire_type == expected_wt {
                        Verdict::Found(tr.child_state_id, tr.label)
                    } else {
                        Verdict::Mismatch
                    }
                }
            };
            verdicts.push((ae.state_id, v));
        }

        // Apply mismatches: veto affected entries, then drop empty ActiveEntries.
        for ae in active.iter_mut() {
            let v = verdicts
                .iter()
                .find(|(sid, _)| *sid == ae.state_id)
                .map(|(_, v)| v);
            if matches!(v, Some(Verdict::Mismatch)) {
                for &e in &ae.entries {
                    ws.set_vetoed(e);
                }
                ae.entries.clear();
            }
        }
        active.retain(|ae| !ae.entries.is_empty());

        if active.is_empty() {
            return pos;
        }

        // Helper: look up the verdict for the given state_id.
        // Returns Unknown if not found (shouldn't happen for entries still in active).
        let verdict_for = |sid: u32| -> Verdict {
            verdicts
                .iter()
                .find(|(s, _)| *s == sid)
                .map(|(_, v)| *v)
                .unwrap_or(Verdict::Unknown)
        };

        // ── Consume wire body ─────────────────────────────────────────────────

        match wire_type {
            WT_VARINT => {
                let vr = parse_varint(buf, pos);
                if vr.garbage.is_some() {
                    veto_all(&mut active, ws);
                    return buflen;
                }
                pos = vr.next_pos;
                let val = vr.value;

                for ae in active.iter_mut() {
                    match verdict_for(ae.state_id) {
                        Verdict::Unknown => {
                            for &e in &ae.entries {
                                ws.scores[e as usize].unknowns += 1;
                            }
                        }
                        Verdict::Found(child, _label) => {
                            // Value overhang: only for Found entries.
                            if vr.overhang > 0 {
                                for &e in &ae.entries {
                                    ws.scores[e as usize].non_canonical += 1;
                                }
                            }
                            let node = ws
                                .graph
                                .nodes
                                .iter()
                                .find(|n| n.state_id.to_native() == child);
                            let mut do_veto = false;
                            if let Some(n) = node {
                                let eri = n.enum_range_idx.to_native();
                                if eri != 0xFFFF {
                                    if val >= (1u64 << 32) {
                                        do_veto = true;
                                    } else {
                                        // Truncated negative non-canonical.
                                        if (0x8000_0000..=0xFFFF_FFFF).contains(&val) {
                                            for &e in &ae.entries {
                                                ws.scores[e as usize].non_canonical += 1;
                                            }
                                        }
                                        let range = ws.graph.enum_ranges.get(eri as usize);
                                        if let Some(range) = range {
                                            let (min, max) = (
                                                range.0.to_native() as i64,
                                                range.1.to_native() as i64,
                                            );
                                            let signed = val as i32 as i64;
                                            if signed < min || signed > max {
                                                do_veto = true;
                                            }
                                        }
                                    }
                                }
                            }
                            if do_veto {
                                for &e in &ae.entries {
                                    ws.set_vetoed(e);
                                }
                                ae.entries.clear();
                            } else {
                                record_occurrence(&mut ae.occurrences, field_number as u32);
                                for &e in &ae.entries {
                                    ws.scores[e as usize].matches += 1;
                                }
                            }
                        }
                        Verdict::Mismatch => {} // already handled above
                    }
                }
                active.retain(|ae| !ae.entries.is_empty());
            }

            WT_I64 => {
                if pos + 8 > buflen {
                    veto_all(&mut active, ws);
                    return buflen;
                }
                pos += 8;
                for ae in active.iter_mut() {
                    match verdict_for(ae.state_id) {
                        Verdict::Unknown => {
                            for &e in &ae.entries {
                                ws.scores[e as usize].unknowns += 1;
                            }
                        }
                        Verdict::Found(_, _) => {
                            record_occurrence(&mut ae.occurrences, field_number as u32);
                            for &e in &ae.entries {
                                ws.scores[e as usize].matches += 1;
                            }
                        }
                        Verdict::Mismatch => {}
                    }
                }
            }

            WT_LEN => {
                let lr = parse_varint(buf, pos);
                if lr.garbage.is_some() {
                    veto_all(&mut active, ws);
                    return buflen;
                }
                pos = lr.next_pos;

                // Length-prefix overhang: all active entries at this depth.
                if lr.overhang > 0 {
                    for ae in &active {
                        for &e in &ae.entries {
                            ws.scores[e as usize].non_canonical += 1;
                        }
                    }
                }

                let length = lr.value as usize;
                if pos + length > buflen {
                    veto_all(&mut active, ws);
                    return buflen;
                }
                let payload = &buf[pos..pos + length];
                pos += length;

                let mut child_pairs: Vec<(u32, u16)> = Vec::new();

                for ae in active.iter_mut() {
                    match verdict_for(ae.state_id) {
                        Verdict::Unknown => {
                            for &e in &ae.entries {
                                ws.scores[e as usize].unknowns += 1;
                            }
                        }
                        Verdict::Found(child, _label) => {
                            let is_message = ws
                                .graph
                                .transitions
                                .iter()
                                .any(|t| t.state_id.to_native() == child);
                            let node = ws
                                .graph
                                .nodes
                                .iter()
                                .find(|n| n.state_id.to_native() == child);
                            if is_message {
                                record_occurrence(&mut ae.occurrences, field_number as u32);
                                for &e in &ae.entries {
                                    ws.scores[e as usize].matches += 1;
                                    child_pairs.push((child, e));
                                }
                            } else {
                                let is_string = node.is_some_and(|n| n.is_string);
                                if is_string && std::str::from_utf8(payload).is_err() {
                                    for &e in &ae.entries {
                                        ws.set_vetoed(e);
                                    }
                                    ae.entries.clear();
                                } else {
                                    record_occurrence(&mut ae.occurrences, field_number as u32);
                                    for &e in &ae.entries {
                                        ws.scores[e as usize].matches += 1;
                                    }
                                }
                            }
                        }
                        Verdict::Mismatch => {}
                    }
                }
                active.retain(|ae| !ae.entries.is_empty());

                if !child_pairs.is_empty() {
                    let child_active = group_by_state(child_pairs.into_iter());
                    score_message_multi(payload, 0, child_active, None, ws);
                    propagate_vetoes(&mut active, ws);
                }
            }

            WT_START_GROUP => {
                // Split active into recurse_into (Found) and stay_out (Unknown).
                let mut recurse_into: Vec<(u32, u16)> = Vec::new();
                let mut stay_out_entries: Vec<u16> = Vec::new();

                for ae in active.iter_mut() {
                    match verdict_for(ae.state_id) {
                        Verdict::Found(child, _label) => {
                            for &e in &ae.entries {
                                recurse_into.push((child, e));
                            }
                        }
                        Verdict::Unknown => {
                            for &e in &ae.entries {
                                stay_out_entries.push(e);
                            }
                        }
                        Verdict::Mismatch => {} // already vetoed above
                    }
                }

                let new_pos = if !recurse_into.is_empty() {
                    // Recurse with schema — boundaries are determined by the group walk.
                    let child_active = group_by_state(recurse_into.iter().copied());
                    let np = score_message_multi(buf, pos, child_active, Some(field_number), ws);
                    propagate_vetoes(&mut active, ws);
                    // Record occurrences and matches for surviving Found entries.
                    for ae in active.iter_mut() {
                        if matches!(verdict_for(ae.state_id), Verdict::Found(_, _)) {
                            record_occurrence(&mut ae.occurrences, field_number as u32);
                            for &e in &ae.entries {
                                ws.scores[e as usize].matches += 1;
                            }
                        }
                    }
                    np
                } else {
                    // All entries are Unknown — use parse_group_blind for boundary.
                    match parse_group_blind(buf, pos, field_number) {
                        None => {
                            veto_all(&mut active, ws);
                            return buflen;
                        }
                        Some(np) => np,
                    }
                };

                // Stay-out entries advance to new_pos (wire boundary is the same for all).
                // If recurse_into was non-empty but all vetoed, use parse_group_blind.
                let final_pos = if !recurse_into.is_empty()
                    && active
                        .iter()
                        .all(|ae| !matches!(verdict_for(ae.state_id), Verdict::Found(_, _)))
                {
                    // All Found entries were vetoed; need blind walk for stay_out boundary.
                    match parse_group_blind(buf, pos, field_number) {
                        None => {
                            // stay_out entries also can't parse it — veto them too.
                            for ae in active.iter_mut() {
                                if matches!(verdict_for(ae.state_id), Verdict::Unknown) {
                                    for &e in &ae.entries {
                                        ws.set_vetoed(e);
                                    }
                                    ae.entries.clear();
                                }
                            }
                            active.retain(|ae| !ae.entries.is_empty());
                            return buflen;
                        }
                        Some(np) => np,
                    }
                } else {
                    new_pos
                };

                // Apply unknowns for stay_out entries.
                for &e in &stay_out_entries {
                    if !ws.is_vetoed(e) {
                        ws.scores[e as usize].unknowns += 1;
                    }
                }

                pos = final_pos;
                active.retain(|ae| !ae.entries.is_empty());
            }

            WT_END_GROUP => match my_group {
                None => {
                    veto_all(&mut active, ws);
                    return buflen;
                }
                Some(expected) => {
                    if field_number != expected {
                        veto_all(&mut active, ws);
                        return buflen;
                    }
                    for ae in &active {
                        apply_cardinality_multi(ws.graph, ae, ws.scores);
                    }
                    return pos;
                }
            },

            WT_I32 => {
                if pos + 4 > buflen {
                    veto_all(&mut active, ws);
                    return buflen;
                }
                pos += 4;
                for ae in active.iter_mut() {
                    match verdict_for(ae.state_id) {
                        Verdict::Unknown => {
                            for &e in &ae.entries {
                                ws.scores[e as usize].unknowns += 1;
                            }
                        }
                        Verdict::Found(_, _) => {
                            record_occurrence(&mut ae.occurrences, field_number as u32);
                            for &e in &ae.entries {
                                ws.scores[e as usize].matches += 1;
                            }
                        }
                        Verdict::Mismatch => {}
                    }
                }
            }

            _ => unreachable!("wire type > 5 caught by parse_wiretag"),
        }
    }
}
