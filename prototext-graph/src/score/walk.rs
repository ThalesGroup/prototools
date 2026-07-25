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

use prototext_core::helpers::{payload_end, MAX_WIRE_DEPTH};
use smallvec::SmallVec;

use crate::build_scoring_graph::serial::{ArchivedCompiledGraph, ArchivedNodeEntry};

// ── Wire-type constants (mirrors prototext-core/src/helpers/wire.rs) ──────────

const WT_VARINT: u32 = 0;
const WT_I64: u32 = 1;
const WT_LEN: u32 = 2;
const WT_START_GROUP: u32 = 3;
const WT_END_GROUP: u32 = 4;
const WT_I32: u32 = 5;

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
    /// If set, print a message to stderr whenever this FQDN is vetoed.
    debug_fqdn: Option<String>,
    strict_ranges: bool,
    expand_any: bool,
}

impl<'a> WalkState<'a> {
    fn new(
        graph: &'a ArchivedCompiledGraph,
        scores: &'a mut Vec<EntryScore>,
        opts: &ScoringOpts,
    ) -> Self {
        let n = scores.len();
        let words = n.div_ceil(64);
        WalkState {
            graph,
            scores,
            vetoed: vec![0u64; words],
            debug_fqdn: std::env::var("PROTOTEXT_DEBUG_FQDN").ok(),
            strict_ranges: opts.strict_ranges,
            expand_any: opts.expand_any,
        }
    }

    fn is_vetoed(&self, e: u16) -> bool {
        let e = e as usize;
        (self.vetoed[e / 64] >> (e % 64)) & 1 == 1
    }

    fn set_vetoed(&mut self, e: u16, reason: &str) {
        let ei = e as usize;
        if self.vetoed[ei / 64] & (1 << (ei % 64)) != 0 {
            return; // already vetoed
        }
        self.vetoed[ei / 64] |= 1 << (ei % 64);
        self.scores[ei].vetoed = true;
        if let Some(ref dbg) = self.debug_fqdn {
            if self.scores[ei].fqdn == *dbg {
                eprintln!("[veto] {} — {}", self.scores[ei].fqdn, reason);
            }
        }
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

/// Options controlling walk behaviour.
pub struct ScoringOpts {
    /// If true (default), out-of-range RANGE values are vetoed.
    /// If false (--no-strict-ranges), they increment non_canonical instead.
    pub strict_ranges: bool,
    /// If true (default), google.protobuf.Any fields are expanded using the
    /// type resolved from type_url and scored against the wrapped type.
    /// If false (--no-expand-any), value scores as a plain bytes match.
    pub expand_any: bool,
}

impl Default for ScoringOpts {
    fn default() -> Self {
        Self {
            strict_ranges: true,
            expand_any: true,
        }
    }
}

/// Score all root entries in `graph` simultaneously against `pb`.
/// Returns one `EntryScore` per root entry, in graph order.
pub fn score_all(pb: &[u8], graph: &ArchivedCompiledGraph, opts: &ScoringOpts) -> Vec<EntryScore> {
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

    let mut ws = WalkState::new(graph, &mut scores, opts);
    score_message_multi(pb, 0, initial_active, None, &mut ws, 0);

    scores
}

/// Score a single named root entry in `graph` against `pb`, without walking
/// any other root candidates.  `fqdn` may be given with or without a leading
/// dot, matching either form stored in `graph.roots`.  Returns `None` if no
/// root entry matches `fqdn`.
pub fn score_one(
    pb: &[u8],
    fqdn: &str,
    graph: &ArchivedCompiledGraph,
    opts: &ScoringOpts,
) -> Option<EntryScore> {
    let want = fqdn.trim_start_matches('.');
    let root = graph
        .roots
        .iter()
        .find(|r| r.fqdn.trim_start_matches('.') == want)?;

    let mut scores = vec![EntryScore {
        fqdn: root.fqdn.as_str().to_owned(),
        matches: 0,
        unknowns: 0,
        mismatches: 0,
        non_canonical: 0,
        vetoed: false,
    }];

    let mut entries: SmallVec<[u16; 4]> = SmallVec::new();
    entries.push(0);
    let initial_active = vec![ActiveEntry {
        state_id: root.state_id.to_native(),
        entries,
        occurrences: Vec::new(),
    }];

    let mut ws = WalkState::new(graph, &mut scores, opts);
    score_message_multi(pb, 0, initial_active, None, &mut ws, 0);

    scores.pop()
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
//
// Iterative on purpose (spec 0171 §S3). Matching group nesting needs a
// counter, not a call stack — a START_GROUP tag costs one byte, so the
// recursive form this replaced could be made to demand a million frames from a
// 1 MB range. Being unable to overflow is what lets this need no depth cap of
// its own.
//
// One check is given up in exchange. The recursive form validated every level's
// closing field number against its own opener; doing that here would need a
// `Vec<u64>` of open field numbers, an allocation in a routine that exists to
// be allocation-free. Only the outermost is checked. That is safe because the
// answer is only ever used to find where an *unscored* group ends: a tolerated
// inner mismatch changes no verdict, only which bytes are skipped, and those
// bytes contribute nothing either way.

fn parse_group_blind(buf: &[u8], mut pos: usize, expected_field: u64) -> Option<usize> {
    let buflen = buf.len();
    let mut depth: usize = 0;
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
                pos = payload_end(pos, 8, buflen)?;
            }
            WT_LEN => {
                let vr = parse_varint(buf, pos);
                if vr.garbage.is_some() {
                    return None;
                }
                pos = payload_end(vr.next_pos, vr.value, buflen)?;
            }
            WT_START_GROUP => {
                depth += 1;
            }
            WT_END_GROUP => {
                if depth == 0 {
                    if tag.field_number != expected_field {
                        return None; // mismatched group end
                    }
                    return Some(pos);
                }
                depth -= 1;
            }
            WT_I32 => {
                pos = payload_end(pos, 4, buflen)?;
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
            // wire_type 8 (UINT32) and 9 (INT32) are internal discriminants;
            // their actual protobuf wire type is 0 (varint).
            let wt = n[mid].wire_type;
            return if wt == 8 || wt == 9 { 0 } else { wt };
        } else {
            hi = mid;
        }
    }
    // Should never happen for a well-formed graph.
    u8::MAX
}

/// Binary search for the node with the given `state_id` (nodes are sorted by
/// state_id, per the schema-lookup invariant above).
fn find_node(graph: &ArchivedCompiledGraph, state_id: u32) -> Option<&ArchivedNodeEntry> {
    let n = &graph.nodes;
    let start = n.partition_point(|e| e.state_id.to_native() < state_id);
    n.get(start).filter(|e| e.state_id.to_native() == state_id)
}

/// True iff `state_id` has at least one outgoing transition, i.e. is a
/// message/group state rather than a leaf (string/bytes) state.
fn state_has_transitions(graph: &ArchivedCompiledGraph, state_id: u32) -> bool {
    let t = &graph.transitions;
    let start = t.partition_point(|e| e.state_id.to_native() < state_id);
    t.get(start)
        .is_some_and(|e| e.state_id.to_native() == state_id)
}

// ── Cardinality check helpers ─────────────────────────────────────────────────

/// Increment occurrences[field_number] by 1.  The vec is kept sorted.
fn record_occurrence(occurrences: &mut Vec<(u32, u64)>, field_number: u32) {
    match occurrences.binary_search_by_key(&field_number, |&(f, _)| f) {
        Ok(i) => occurrences[i].1 += 1,
        Err(i) => occurrences.insert(i, (field_number, 1)),
    }
}

// ── Multi-entry parallel walk (spec 0048) ─────────────────────────────────────

/// Veto all entries in `active` and clear the set.
fn veto_all(active: &mut Vec<ActiveEntry>, ws: &mut WalkState, reason: &str) {
    for ae in active.iter() {
        for &e in &ae.entries {
            ws.set_vetoed(e, reason);
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

// ── Any expansion helpers (spec 0089 §8) ──────────────────────────────────────

/// Block ID 0 is permanently reserved for `google.protobuf.Any` (spec 0089 §9).
const ANY_BLOCK_ID: u32 = 0;

/// Scan `any_bytes` for field 1 (WT_LEN, wire type 2) and return its value as
/// a UTF-8 `&str`, or `None` if absent, empty, or not valid UTF-8.
fn extract_type_url(any_bytes: &[u8]) -> Option<&str> {
    let mut pos = 0;
    let buflen = any_bytes.len();
    while pos < buflen {
        let tag = parse_wiretag(any_bytes, pos);
        if tag.garbage.is_some() {
            return None;
        }
        let field_number = tag.field_number;
        let wire_type = tag.wire_type;
        pos = tag.next_pos;
        match wire_type {
            WT_VARINT => {
                let vr = parse_varint(any_bytes, pos);
                if vr.garbage.is_some() {
                    return None;
                }
                pos = vr.next_pos;
            }
            WT_I64 => {
                pos = payload_end(pos, 8, buflen)?;
            }
            WT_LEN => {
                let lr = parse_varint(any_bytes, pos);
                if lr.garbage.is_some() {
                    return None;
                }
                let end = payload_end(lr.next_pos, lr.value, buflen)?;
                let payload = &any_bytes[lr.next_pos..end];
                pos = end;
                if field_number == 1 {
                    // Field 1 = type_url (string).
                    if payload.is_empty() {
                        return None;
                    }
                    return std::str::from_utf8(payload).ok();
                }
            }
            WT_START_GROUP => {
                // Skip nested group blindly; for Any payloads this should not appear.
                pos = parse_group_blind(any_bytes, pos, field_number)?;
            }
            WT_I32 => {
                pos = payload_end(pos, 4, buflen)?;
            }
            _ => return None,
        }
    }
    None
}

/// Scan `any_bytes` for field 2 (WT_LEN, wire type 2) and return its raw
/// byte slice (the `value` sub-field), or `None` if absent or the buffer is
/// malformed. Mirrors `extract_type_url` but scans for field 2 and returns
/// the raw bytes rather than interpreting them as UTF-8.
fn extract_any_value(any_bytes: &[u8]) -> Option<&[u8]> {
    let mut pos = 0;
    let buflen = any_bytes.len();
    while pos < buflen {
        let tag = parse_wiretag(any_bytes, pos);
        if tag.garbage.is_some() {
            return None;
        }
        let field_number = tag.field_number;
        let wire_type = tag.wire_type;
        pos = tag.next_pos;
        match wire_type {
            WT_VARINT => {
                let vr = parse_varint(any_bytes, pos);
                if vr.garbage.is_some() {
                    return None;
                }
                pos = vr.next_pos;
            }
            WT_I64 => {
                pos = payload_end(pos, 8, buflen)?;
            }
            WT_LEN => {
                let lr = parse_varint(any_bytes, pos);
                if lr.garbage.is_some() {
                    return None;
                }
                let end = payload_end(lr.next_pos, lr.value, buflen)?;
                let payload = &any_bytes[lr.next_pos..end];
                pos = end;
                if field_number == 2 {
                    // Field 2 = value (bytes).
                    return Some(payload);
                }
            }
            WT_START_GROUP => {
                pos = parse_group_blind(any_bytes, pos, field_number)?;
            }
            WT_I32 => {
                pos = payload_end(pos, 4, buflen)?;
            }
            _ => return None,
        }
    }
    None
}

fn score_message_multi(
    buf: &[u8],
    start: usize,
    mut active: Vec<ActiveEntry>,
    my_group: Option<u64>,
    ws: &mut WalkState,
    depth: usize,
) -> usize {
    let buflen = buf.len();
    let mut pos = start;

    // Hard cap on this function's genuine recursion, shared with
    // `prototext-core`'s renderer so both wire walkers refuse the same inputs
    // (spec 0171 §S1). `parse_group_blind` needs no cap of its own — it is
    // iterative.
    //
    // Depth here tracks the *byte range's* own LEN/group nesting, which for a
    // schema mismatch (scoring a range against a candidate type whose shape
    // doesn't actually match the bytes) can run far deeper than any well-formed
    // document would — in the worst case, proportional to the buffer's length
    // divided by the smallest possible LEN-field overhead (tag + zero-length
    // prefix), i.e. tens of thousands of levels for an ordinary-sized document
    // — a legitimate (if unlikely) stack-overflow risk on its own, though not
    // the cause of the 2026-07-25 segfault report that first prompted this cap
    // (that turned out to be an unrelated `App`-field-drop-order
    // use-after-unmap race in `protolens`, not a recursion-depth issue here).
    //
    // Unlike the renderer's cap, exceeding this one is a veto rather than a
    // local degradation: a range this walker cannot finish reading is a range
    // it cannot honestly score.
    if depth > MAX_WIRE_DEPTH {
        veto_all(&mut active, ws, "recursion depth exceeded");
        return buflen;
    }

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
                    veto_all(&mut active, ws, "open-ended group (EOF inside group)");
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
            veto_all(&mut active, ws, "garbage wire tag");
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
                    ws.set_vetoed(
                        e,
                        &format!(
                            "wire-type mismatch on field {field_number} (wire_type={wire_type})"
                        ),
                    );
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
                    veto_all(&mut active, ws, "truncated varint body");
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
                            let node = find_node(ws.graph, child);
                            let mut do_veto = false;
                            if let Some(n) = node {
                                let wt = n.wire_type;
                                let ri = n.range_idx.to_native();
                                match wt {
                                    9 => {
                                        // INT32: veto if in invalid gap (0xFFFF_FFFF, 0xFFFF_FFFF_8000_0000)
                                        if val > 0xFFFF_FFFF && val < 0xFFFF_FFFF_8000_0000u64 {
                                            do_veto = true;
                                        } else if (0x8000_0000u64..=0xFFFF_FFFFu64).contains(&val) {
                                            // truncated negative int32 (4-byte encoding)
                                            for &e in &ae.entries {
                                                ws.scores[e as usize].non_canonical += 1;
                                            }
                                        }
                                    }
                                    8 => {
                                        // UINT32: veto if > 32-bit
                                        if val > 0xFFFF_FFFF {
                                            do_veto = true;
                                        }
                                    }
                                    0 if ri != 0xFFFF => {
                                        // RANGE (bool / enum)
                                        if val >= (1u64 << 32) {
                                            do_veto = true;
                                        } else {
                                            if (0x8000_0000u64..=0xFFFF_FFFFu64).contains(&val) {
                                                for &e in &ae.entries {
                                                    ws.scores[e as usize].non_canonical += 1;
                                                }
                                            }
                                            let range = ws.graph.ranges.get(ri as usize);
                                            if let Some(range) = range {
                                                let (min, max) = (
                                                    range.0.to_native() as i64,
                                                    range.1.to_native() as i64,
                                                );
                                                let signed = val as i32 as i64;
                                                if signed < min || signed > max {
                                                    if ws.strict_ranges {
                                                        do_veto = true;
                                                    } else {
                                                        for &e in &ae.entries {
                                                            ws.scores[e as usize].non_canonical +=
                                                                1;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    _ => {} // UINT64 or non-varint: no range check
                                }
                            }
                            if do_veto {
                                for &e in &ae.entries {
                                    ws.set_vetoed(
                                        e,
                                        &format!(
                                            "varint value out of range on field {field_number}"
                                        ),
                                    );
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
                let Some(end) = payload_end(pos, 8, buflen) else {
                    veto_all(&mut active, ws, "truncated I64 body");
                    return buflen;
                };
                pos = end;
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
                    veto_all(&mut active, ws, "truncated LEN length prefix");
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

                let Some(end) = payload_end(pos, lr.value, buflen) else {
                    veto_all(&mut active, ws, "LEN body extends past end of buffer");
                    return buflen;
                };
                let payload = &buf[pos..end];
                pos = end;

                let mut child_pairs: Vec<(u32, u16)> = Vec::new();

                for ae in active.iter_mut() {
                    match verdict_for(ae.state_id) {
                        Verdict::Unknown => {
                            for &e in &ae.entries {
                                ws.scores[e as usize].unknowns += 1;
                            }
                        }
                        Verdict::Found(child, _label) => {
                            let is_message = state_has_transitions(ws.graph, child);
                            let node = find_node(ws.graph, child);
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
                                        ws.set_vetoed(
                                            e,
                                            &format!(
                                                "invalid UTF-8 on string field {field_number}"
                                            ),
                                        );
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
                    // Separate Any candidates (child_state == ANY_BLOCK_ID) from
                    // normal message candidates (spec 0089 §6).
                    let (any_pairs, normal_pairs): (Vec<_>, Vec<_>) = child_pairs
                        .into_iter()
                        .partition(|(child, _)| *child == ANY_BLOCK_ID);

                    if !any_pairs.is_empty() && ws.expand_any {
                        // Resolve type_url from the Any payload.
                        let resolved_state: Option<u32> =
                            extract_type_url(payload).and_then(|type_url| {
                                let fqdn = if let Some(slash) = type_url.rfind('/') {
                                    &type_url[slash + 1..]
                                } else {
                                    type_url
                                };
                                ws.graph
                                    .roots
                                    .iter()
                                    .find(|r| r.fqdn.as_str() == fqdn)
                                    .map(|r| r.state_id.to_native())
                            });
                        match resolved_state {
                            Some(root_state) => {
                                // Recurse into the wrapped type: replace state_id 0
                                // with the resolved root entry state; keep entry indices.
                                // Recurse into the `value` sub-field's bytes only —
                                // not the whole Any body (spec 0107).
                                let value_payload = extract_any_value(payload).unwrap_or(&[]);
                                let any_active =
                                    group_by_state(any_pairs.iter().map(|&(_, e)| (root_state, e)));
                                score_message_multi(
                                    value_payload,
                                    0,
                                    any_active,
                                    None,
                                    ws,
                                    depth + 1,
                                );
                            }
                            None => {
                                // Unknown type_url or not a root: score value as plain
                                // bytes match — one match, no penalty (already counted
                                // above when we pushed to child_pairs).
                                // No recursion needed; match was already recorded.
                            }
                        }
                    } else if !any_pairs.is_empty() {
                        // expand_any disabled: plain bytes match already counted above.
                    }

                    if !normal_pairs.is_empty() {
                        let child_active = group_by_state(normal_pairs.into_iter());
                        score_message_multi(payload, 0, child_active, None, ws, depth + 1);
                    }
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
                    let np = score_message_multi(
                        buf,
                        pos,
                        child_active,
                        Some(field_number),
                        ws,
                        depth + 1,
                    );
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
                            veto_all(&mut active, ws, "malformed unknown group");
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
                                        ws.set_vetoed(e, "malformed group (blind fallback)");
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
                    veto_all(&mut active, ws, "unexpected END_GROUP outside any group");
                    return buflen;
                }
                Some(expected) => {
                    if field_number != expected {
                        veto_all(
                            &mut active,
                            ws,
                            &format!(
                                "mismatched END_GROUP: expected field {expected}, got {field_number}"
                            ),
                        );
                        return buflen;
                    }
                    for ae in &active {
                        apply_cardinality_multi(ws.graph, ae, ws.scores);
                    }
                    return pos;
                }
            },

            WT_I32 => {
                let Some(end) = payload_end(pos, 4, buflen) else {
                    veto_all(&mut active, ws, "truncated I32 body");
                    return buflen;
                };
                pos = end;
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
