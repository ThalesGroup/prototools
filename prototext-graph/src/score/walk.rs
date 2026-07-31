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

/// `TransitionEntry::label` for a repeated field (`graph.rs:238-242`).
const LABEL_REPEATED: u8 = 2;

// ── Multi-entry types (spec 0048) ─────────────────────────────────────────────

/// Per-entry scoring counters for the multi-entry walk.
pub struct EntryScore<'g> {
    /// Borrowed from the graph's `ArchivedString`, which outlives every
    /// call. Copying it allocated once per root *before scoring began*, so
    /// a 13 000-root graph paid 13 000 allocations to score a 40-byte blob
    /// that vetoes on its first tag.
    pub fqdn: &'g str,
    pub matches: u64,
    pub unknowns: u64,
    /// Values outside a `Range` leaf's declared extent: a `bool` other than
    /// 0/1, or a closed enum's undeclared number (spec 0178 S1). Legal on the
    /// wire, so never a veto — but no conformant writer emits one.
    ///
    /// Separate from `non_canonical` so a consumer can tell "outside the
    /// declared range" from "sloppy encoding"; the two are adjacent, not
    /// equal, in weight.
    pub out_of_range: u64,
    pub non_canonical: u64,
    /// A `required` field this schema declares that the blob does not contain
    /// (`apply_cardinality_multi`, label 1, count 0 — the only site).
    ///
    /// Despite the name this is **not** a wire-type mismatch, which vetoes and
    /// never reaches here.
    pub mismatches: u64,
    pub vetoed: bool,
}

impl EntryScore<'_> {
    /// Coefficients rank by how damning the signal is (spec 0178 S1), which is
    /// also the order the reports print them in.
    ///
    /// `unknowns` is mildest because it has a benign explanation — a newer
    /// sender talking to an older schema is what forward compatibility is for.
    /// `out_of_range` and `non_canonical` are evidence about *writer
    /// conformance*: the schema still fits, the writer was odd. `mismatches` is
    /// the heaviest because it is evidence about *schema fit* with no benign
    /// reading: `required` is precisely what a conformant writer cannot omit.
    pub fn score(&self) -> i64 {
        self.matches as i64
            - 10 * self.unknowns as i64
            - 15 * self.out_of_range as i64
            - 20 * self.non_canonical as i64
            - 30 * self.mismatches as i64
    }
}

/// This active entry's schema verdict for one wire tag.
///
/// `label` is carried on `Found` so occurrences are recorded only for it.
#[derive(Clone, Copy)]
enum Verdict {
    Unknown,
    Mismatch,
    Found(u32, u8), // (child_state_id, label)
    /// A LEN tag on a repeated scalar field: the packed encoding of the same
    /// values the field's own wire type would carry expanded (spec 0175 S2).
    /// The element wire type is carried instead of the label, since only a
    /// repeated field is packable and the LEN arm needs it to read the run.
    FoundPacked(u32, u8), // (child_state_id, element wire type)
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
    ///
    /// The index is `u32` rather than `u16` (spec 0179 S1) because the
    /// `u16` capped a corpus at 65 535 root message types while
    /// `docs/schema-match.md` targets 100 000+; googleapis alone compiles
    /// to 49 255. It is a runtime index only — nothing in the serialized
    /// graph changed.
    ///
    /// The inline capacity is 4, and that is the load-bearing number, not
    /// the element width: it covers 93.4% of states on a real corpus, and
    /// keeping it unchanged is what makes the widening allocation-neutral
    /// (every spill decision is bit-identical to the `u16` version).
    /// `[u32; 2]` has the same `size_of` and looks free for that reason —
    /// measured, it costs 21.9% more allocations. `[u32; 8]` removes
    /// allocations and is *slower*, with higher peak RSS. See spec 0179.
    entries: SmallVec<[u32; 4]>,
    /// Per-frame occurrence counts for fields that received a Found verdict.
    /// field_number → how many times seen in this message/group frame.
    ///
    /// Inline rather than a `Vec` (spec 0179 S2) because this was the
    /// walk's single largest allocation site: 81.6% of everything
    /// `score_all` allocated. Half of all `ActiveEntry` record nothing at
    /// all — those never allocated either way — but the other half took a
    /// 64-byte heap block (a `Vec`'s first capacity for a 16-byte element)
    /// to hold, 98% of the time, one or two pairs.
    ///
    /// Capacity 2 covers 98.15% of frames; capacity 4 covers 99.87% and
    /// measured slower and larger.
    occurrences: SmallVec<[(u32, u32); 2]>, // sorted by field_number
    /// This entry's verdict for the wire tag currently being processed.
    /// Overwritten at the top of every tag iteration and never read across
    /// iterations.
    ///
    /// Held here rather than in a side table keyed by `state_id` because
    /// `active.retain()` compacts the vector mid-iteration: positional
    /// indices into a parallel array do not survive it. The state_id-keyed
    /// lookup that used to bridge that gap was a linear scan run once per
    /// entry per tag — O(A²) per tag, where A is the number of distinct
    /// live states. A field moves with its owner through `retain` for free,
    /// and `propagate_vetoes` and every `retain` here only ever *remove*
    /// entries, never reorder or rebuild them, so an entry's verdict stays
    /// its own.
    verdict: Verdict,
}

/// Global walk state shared across all recursion levels.
struct WalkState<'a, 'g> {
    graph: &'a ArchivedCompiledGraph,
    scores: &'a mut Vec<EntryScore<'g>>,
    /// Flat bitset: bit i is set iff entry i has been permanently vetoed.
    vetoed: Vec<u64>,
    /// If set, print a message to stderr whenever this FQDN is vetoed.
    debug_fqdn: Option<String>,
    expand_any: bool,
}

impl<'a, 'g> WalkState<'a, 'g> {
    fn new(
        graph: &'a ArchivedCompiledGraph,
        scores: &'a mut Vec<EntryScore<'g>>,
        opts: &ScoringOpts,
    ) -> Self {
        let n = scores.len();
        let words = n.div_ceil(64);
        WalkState {
            graph,
            scores,
            vetoed: vec![0u64; words],
            debug_fqdn: std::env::var("PROTOTEXT_DEBUG_FQDN").ok(),
            expand_any: opts.expand_any,
        }
    }

    fn is_vetoed(&self, e: u32) -> bool {
        let e = e as usize;
        (self.vetoed[e / 64] >> (e % 64)) & 1 == 1
    }

    /// `reason` is a closure because it is read only when
    /// `PROTOTEXT_DEBUG_FQDN` names this exact candidate — i.e. never, in
    /// production. It used to be a `&str`, which meant the `format!` call
    /// sites below built and dropped a `String` once per candidate per
    /// mismatching tag.
    fn set_vetoed(&mut self, e: u32, reason: impl FnOnce() -> String) {
        let ei = e as usize;
        if self.vetoed[ei / 64] & (1 << (ei % 64)) != 0 {
            return; // already vetoed
        }
        self.vetoed[ei / 64] |= 1 << (ei % 64);
        self.scores[ei].vetoed = true;
        if let Some(ref dbg) = self.debug_fqdn {
            if self.scores[ei].fqdn == *dbg {
                eprintln!("[veto] {} — {}", self.scores[ei].fqdn, reason());
            }
        }
    }
}

/// Group entries by their state_id, producing one `ActiveEntry` per distinct state.
fn group_by_state(pairs: impl Iterator<Item = (u32, u32)>) -> Vec<ActiveEntry> {
    let mut v: Vec<(u32, u32)> = pairs.collect();
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
            occurrences: SmallVec::new(),
            verdict: Verdict::Unknown,
        });
    }
    result
}

/// Options controlling walk behaviour.
pub struct ScoringOpts {
    /// If true (default), google.protobuf.Any fields are expanded using the
    /// type resolved from type_url and scored against the wrapped type.
    /// If false (--no-expand-any), value scores as a plain bytes match.
    pub expand_any: bool,
}

impl Default for ScoringOpts {
    fn default() -> Self {
        Self { expand_any: true }
    }
}

/// Score all root entries in `graph` simultaneously against `pb`.
/// Returns one `EntryScore` per root entry, in graph order.
pub fn score_all<'g>(
    pb: &[u8],
    graph: &'g ArchivedCompiledGraph,
    opts: &ScoringOpts,
) -> Vec<EntryScore<'g>> {
    let all: Vec<u32> = (0..graph.roots.len() as u32).collect();
    score_subset(pb, graph, opts, &all)
}

/// `score_all` restricted to `roots` (indices into `graph.roots`).
/// Returns one `EntryScore` per element of `roots`, in `roots` order.
///
/// Spec 0217 S1. This is the real walk; `score_all` is it over every
/// index. Single-threaded, and deliberately ignorant of the fact that
/// anyone might be running several of these at once: the walk touches
/// nothing but `pb` and `graph`, both shared immutably, so N concurrent
/// calls need no synchronization and the library needs no threading.
///
/// The caller partitions with [`partition_roots`], which is not merely a
/// convenience — see its doc comment for why an arbitrary partition
/// duplicates work rather than dividing it.
pub fn score_subset<'g>(
    pb: &[u8],
    graph: &'g ArchivedCompiledGraph,
    opts: &ScoringOpts,
    roots: &[u32],
) -> Vec<EntryScore<'g>> {
    // Spec 0172 S5: enforced at load time by `load::check_root_count`,
    // which turns an oversized corpus into an `Err` instead of aborting
    // the process from inside a background scoring thread. This is the
    // invariant restated, not the check.
    //
    // Spec 0179 S1 widened the index from `u16` to `u32`, so the bound is
    // now 4 294 967 295 rather than 65 535. It stays an assertion rather
    // than becoming unreachable: `roots.len()` is a `usize`, so on a
    // 64-bit target it can still outrun what the index addresses.
    debug_assert!(
        graph.roots.len() <= u32::MAX as usize,
        "entry count {} exceeds u32::MAX (load::check_root_count should have rejected this graph)",
        graph.roots.len()
    );

    let mut scores: Vec<EntryScore<'g>> = roots
        .iter()
        .map(|&r| EntryScore {
            fqdn: graph.roots[r as usize].fqdn.as_str(),
            matches: 0,
            unknowns: 0,
            out_of_range: 0,
            non_canonical: 0,
            mismatches: 0,
            vetoed: false,
        })
        .collect();

    // The active set indexes `scores`, i.e. positions within `roots`, not
    // positions within `graph.roots` — which is why the result is in
    // `roots` order and why the caller's merge needs no remapping.
    let initial_active = group_by_state(
        roots
            .iter()
            .enumerate()
            .map(|(local, &r)| (graph.roots[r as usize].state_id.to_native(), local as u32)),
    );

    let mut ws = WalkState::new(graph, &mut scores, opts);
    score_message_multi(pb, 0, initial_active, None, &mut ws, 0);

    scores
}

/// Split `graph`'s roots into at most `n` parts, balanced by size and
/// never splitting a state group.
///
/// Spec 0217 S1. The grouping is the point, not the balancing. The walk
/// carries candidates grouped by graph state (`group_by_state`), and
/// Hopcroft minimization has already collapsed behaviorally equivalent
/// candidates, so two roots sharing a state are indistinguishable to the
/// walk and cost one traversal between them. Split such a pair across
/// two parts and *both* parts carry that state and both walk it: the
/// work is duplicated, not divided. Whole groups are disjoint by
/// construction, so they are the unit that can actually be shared out.
///
/// Balancing is largest-group-first onto the currently-smallest part.
/// That is O(G log G) on a few thousand groups, against seconds of
/// walking — it does not need to be cleverer.
///
/// Returns only non-empty parts, so the result may be shorter than `n`
/// (and is a single part when `n <= 1`, or when the graph has fewer
/// distinct states than `n`).
pub fn partition_roots(graph: &ArchivedCompiledGraph, n: usize) -> Vec<Vec<u32>> {
    let total = graph.roots.len();
    if n <= 1 || total == 0 {
        return if total == 0 {
            Vec::new()
        } else {
            vec![(0..total as u32).collect()]
        };
    }

    let mut by_state: Vec<(u32, u32)> = (0..total as u32)
        .map(|i| (graph.roots[i as usize].state_id.to_native(), i))
        .collect();
    by_state.sort_unstable();

    let mut groups: Vec<Vec<u32>> = Vec::new();
    let mut i = 0;
    while i < by_state.len() {
        let state = by_state[i].0;
        let mut group = Vec::new();
        while i < by_state.len() && by_state[i].0 == state {
            group.push(by_state[i].1);
            i += 1;
        }
        groups.push(group);
    }
    groups.sort_unstable_by_key(|g| std::cmp::Reverse(g.len()));

    let mut parts: Vec<Vec<u32>> = vec![Vec::new(); n.min(groups.len())];
    for group in groups {
        let smallest = parts
            .iter()
            .enumerate()
            .min_by_key(|(_, p)| p.len())
            .map(|(i, _)| i)
            .expect("at least one part while groups remain");
        parts[smallest].extend(group);
    }
    // Bin packing on the group sizes cannot leave a part empty here (there
    // are at least as many groups as parts). Sorting each part is not
    // required by anything downstream — `group_by_state` sorts what it is
    // given, and the ranking's order comes from `candidate_order` — it just
    // makes a part a canonical list of root indices rather than one in
    // largest-group-first order, which is what makes two runs comparable
    // when reading a trace.
    for part in &mut parts {
        part.sort_unstable();
    }
    parts
}

/// Score a single named root entry in `graph` against `pb`, without walking
/// any other root candidates.  `fqdn` may be given with or without a leading
/// dot, matching either form stored in `graph.roots`.  Returns `None` if no
/// root entry matches `fqdn`.
pub fn score_one<'g>(
    pb: &[u8],
    fqdn: &str,
    graph: &'g ArchivedCompiledGraph,
    opts: &ScoringOpts,
) -> Option<EntryScore<'g>> {
    let want = fqdn.trim_start_matches('.');
    let idx = graph
        .roots
        .iter()
        .position(|r| r.fqdn.trim_start_matches('.') == want)? as u32;

    // Spec 0217 S1: this is `score_subset` over a one-element subset, and
    // used to be the same setup written out a second time — a single root
    // is trivially its own state group, so the general path builds exactly
    // the active set the hand-written one did.
    score_subset(pb, graph, opts, &[idx]).pop()
}

// ── Wire primitives ───────────────────────────────────────────────────────────
//
// The parsing itself is prototext-core's. Scoring and rendering must agree
// byte-for-byte on where every field begins and ends, so there is exactly one
// implementation and this file adapts its result shape.
//
// The adaptation is not cosmetic. Core's result is built for a lossless
// round-trip: on garbage it hands back the offending bytes so the renderer can
// reproduce them, and it makes the success fields `Option` to prove they are
// only read on the good path. Scoring never reproduces anything — it only
// needs to know *that* the bytes were garbage — so the borrow is dropped here
// and the options are flattened, which is what keeps the ~30 call sites below
// free of `unwrap`.

struct VarintResult {
    next_pos: usize,
    /// `Some` when truncated or overflowed. The bytes themselves are of no use
    /// to a score, so they are not carried.
    garbage: Option<()>,
    value: u64,
    /// Number of non-canonical overhang bytes.
    overhang: u64,
}

fn parse_varint(buf: &[u8], start: usize) -> VarintResult {
    let vr = prototext_core::helpers::parse_varint(buf, start);
    VarintResult {
        next_pos: vr.next_pos,
        garbage: vr.varint_gar.map(|_| ()),
        value: vr.varint.unwrap_or(0),
        overhang: vr.varint_ohb.unwrap_or(0),
    }
}

struct TagResult {
    next_pos: usize,
    /// `Some` when wire type > 5 or the varint is truncated / overflowed.
    garbage: Option<()>,
    wire_type: u32,
    field_number: u64,
    overhang: u64,
    /// True when field number is 0 or >= 2^29.
    out_of_range: bool,
}

fn parse_wiretag(buf: &[u8], start: usize) -> TagResult {
    let tag = prototext_core::helpers::parse_wiretag(buf, start);
    TagResult {
        next_pos: tag.next_pos,
        garbage: tag.wtag_gar.map(|_| ()),
        wire_type: tag.wtype.unwrap_or(0),
        field_number: tag.wfield.unwrap_or(0),
        overhang: tag.wfield_ohb.unwrap_or(0),
        out_of_range: tag.wfield_oor.unwrap_or(false),
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
///
/// The count saturates (spec 0179 S2). It is the number of times one field
/// number appears in one message frame, and every occurrence costs at least
/// a tag byte, so reaching `u32::MAX` needs a single frame larger than
/// 4 GiB — not reachable in practice. But the value is derived from
/// attacker-chosen bytes, and a plain `+= 1` on a `u32` wraps silently in a
/// release build (`overflow-checks` is off), which is the defect class spec
/// 0171 exists to prevent. `saturating_add` is one instruction and total,
/// so the reachability argument does not have to be relied on.
fn record_occurrence(occurrences: &mut SmallVec<[(u32, u32); 2]>, field_number: u32) {
    match occurrences.binary_search_by_key(&field_number, |&(f, _)| f) {
        Ok(i) => occurrences[i].1 = occurrences[i].1.saturating_add(1),
        Err(i) => occurrences.insert(i, (field_number, 1)),
    }
}

// ── Packed / expanded repeated scalars (spec 0175) ────────────────────────────

/// True iff `payload` is a run of varints ending exactly at its end.
///
/// A varint whose continuation bit runs past the payload, or that overflows 64
/// bits, makes the run *impossible* rather than merely unlikely — which is what
/// separates this check from a `non_canonical` penalty.
fn packed_varints_terminate(payload: &[u8]) -> bool {
    let mut p = 0usize;
    while p < payload.len() {
        let vr = parse_varint(payload, p);
        if vr.garbage.is_some() {
            return false;
        }
        p = vr.next_pos;
    }
    true
}

/// Per-element varint checks, shared by the expanded and the packed encoding of
/// the same values (spec 0175 S4): the value-overhang penalty, C5's 32-bit gap
/// veto, the `non_canonical` penalty for a negative written in the truncated
/// 5-byte form, and the `Range` leaf's `[min, max]` test.
///
/// Returns true iff the value vetoes. The caller owns the veto and match
/// bookkeeping, which differ between one expanded occurrence and one element of
/// a packed run.
#[inline]
fn check_varint_value(
    ws: &mut WalkState<'_, '_>,
    ae: &ActiveEntry,
    node: Option<&ArchivedNodeEntry>,
    val: u64,
    overhang: u64,
) -> bool {
    if overhang > 0 {
        for &e in &ae.entries {
            ws.scores[e as usize].non_canonical += 1;
        }
    }
    let Some(n) = node else {
        return false;
    };
    let ri = n.range_idx.to_native();
    match n.wire_type {
        9 => {
            // INT32: veto if in invalid gap (0xFFFF_FFFF, 0xFFFF_FFFF_8000_0000)
            if val > 0xFFFF_FFFF && val < 0xFFFF_FFFF_8000_0000u64 {
                return true;
            }
            if (0x8000_0000u64..=0xFFFF_FFFFu64).contains(&val) {
                // truncated negative int32 (4-byte encoding)
                for &e in &ae.entries {
                    ws.scores[e as usize].non_canonical += 1;
                }
            }
            false
        }
        8 => {
            // UINT32: veto if > 32-bit
            val > 0xFFFF_FFFF
        }
        0 if ri != 0xFFFF => {
            // RANGE (bool / enum). Spec 0172 S2: mirrors the INT32 arm above. A
            // negative enum value is sign-extended to 64 bits on the wire, so
            // -1 arrives as 0xFFFF_FFFF_FFFF_FFFF; the only genuinely
            // impossible values are those in the gap between "too big for u32"
            // and "smallest sign-extended i32", which are neither encoding of
            // any 32-bit number. Vetoing on `val >= 1<<32` instead made the
            // *canonical* encoding fatal while merely penalizing the sloppy
            // 5-byte one.
            if val > 0xFFFF_FFFF && val < 0xFFFF_FFFF_8000_0000u64 {
                return true;
            }
            if (0x8000_0000u64..=0xFFFF_FFFFu64).contains(&val) {
                // Negative written in the non-canonical 5-byte form.
                for &e in &ae.entries {
                    ws.scores[e as usize].non_canonical += 1;
                }
            }
            let Some(range) = ws.graph.ranges.get(ri as usize) else {
                return false;
            };
            let (min, max) = (range.0.to_native() as i64, range.1.to_native() as i64);
            // The explicit `as u32` step makes this correct for both encodings:
            // it truncates the sign-extended form back to 32 bits, and is a
            // no-op on the 5-byte one.
            let signed = val as u32 as i32 as i64;
            if signed >= min && signed <= max {
                return false;
            }
            // Spec 0178 S2: a penalty, never a veto. Out of range is not out of
            // bounds — a `bool` is `value != 0` in every generated parser, so 2
            // reads as `true`, and a closed enum moves an undeclared number to
            // the unknown-field set rather than failing the parse. Both are
            // strong evidence against the candidate, which is what
            // `out_of_range`'s -15 is for, but neither is impossible, and the
            // governing principle vetoes only the impossible.
            for &e in &ae.entries {
                ws.scores[e as usize].out_of_range += 1;
            }
            false
        }
        _ => false, // UINT64 or non-varint: no range check
    }
}

// ── Multi-entry parallel walk (spec 0048) ─────────────────────────────────────

/// Veto all entries in `active` and clear the set.
///
/// `reason` is deliberately a plain `&str` rather than `set_vetoed`'s
/// closure: every caller but one passes a literal, and each of them is a
/// terminal error that ends the walk, so it is paid once — not once per
/// candidate per tag, which is what made laziness worth it there.
fn veto_all(active: &mut Vec<ActiveEntry>, ws: &mut WalkState, reason: &str) {
    for ae in active.iter() {
        for &e in &ae.entries {
            ws.set_vetoed(e, || reason.to_string());
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
                        scores[e as usize].non_canonical += (count - 1) as u64;
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
                        scores[e as usize].non_canonical += (count - 1) as u64;
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

        for ae in active.iter_mut() {
            // Spec 0172 S1: a field number of 0 or >= 2^29 cannot be
            // declared by any schema, so there is nothing to look it up
            // against — and narrowing it to u32 for the lookup would
            // alias it onto a real field (2^32+1 onto field 1), awarding
            // a match or a wire-type-mismatch veto on the strength of a
            // number the wire format forbids. The `non_canonical`
            // penalty above still applies; the tag just no longer
            // resolves. In the other branch `field_number as u32` is
            // sound by construction: `!out_of_range` establishes
            // `1 <= field_number < 2^29`.
            let v = if tag.out_of_range {
                Verdict::Unknown
            } else {
                match find_transition(ws.graph, ae.state_id, field_number as u32) {
                    None => Verdict::Unknown,
                    Some(tr) => {
                        let expected_wt = node_wire_type(ws.graph, tr.child_state_id) as u32;
                        if wire_type == expected_wt {
                            Verdict::Found(tr.child_state_id, tr.label)
                        } else if wire_type == WT_LEN
                            && tr.label == LABEL_REPEATED
                            && matches!(expected_wt, WT_VARINT | WT_I64 | WT_I32)
                        {
                            // Spec 0175 S2: a repeated scalar has two legal
                            // wire encodings and a reader must accept both.
                            // The element-wire-type clause is what keeps the
                            // rule from degrading into "LEN is always
                            // acceptable": string, bytes and message children
                            // report 2 and groups 3, so all of them fall
                            // outside {0, 1, 5} without a special case —
                            // including an empty message state, which has no
                            // transitions but still reports 2.
                            Verdict::FoundPacked(tr.child_state_id, expected_wt as u8)
                        } else {
                            Verdict::Mismatch
                        }
                    }
                }
            };
            ae.verdict = v;
        }

        // Apply mismatches: veto affected entries, then drop empty ActiveEntries.
        for ae in active.iter_mut() {
            if matches!(ae.verdict, Verdict::Mismatch) {
                for &e in &ae.entries {
                    ws.set_vetoed(e, || {
                        format!(
                            "wire-type mismatch on field {field_number} (wire_type={wire_type})"
                        )
                    });
                }
                ae.entries.clear();
            }
        }
        active.retain(|ae| !ae.entries.is_empty());

        if active.is_empty() {
            return pos;
        }

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
                    match ae.verdict {
                        Verdict::Unknown => {
                            for &e in &ae.entries {
                                ws.scores[e as usize].unknowns += 1;
                            }
                        }
                        Verdict::Found(child, _label) => {
                            let node = find_node(ws.graph, child);
                            let do_veto = check_varint_value(ws, ae, node, val, vr.overhang);
                            if do_veto {
                                for &e in &ae.entries {
                                    ws.set_vetoed(e, || {
                                        format!("varint value out of range on field {field_number}")
                                    });
                                }
                                ae.entries.clear();
                            } else {
                                record_occurrence(&mut ae.occurrences, field_number as u32);
                                for &e in &ae.entries {
                                    ws.scores[e as usize].matches += 1;
                                }
                            }
                        }
                        // `FoundPacked` is produced only for a LEN tag, so it
                        // cannot occur here. It is inert rather than
                        // `unreachable!()` on purpose: a panic reachable from
                        // wire-format-derived state is the shape of flaws C1
                        // through C4, and an unreachable-but-inert arm costs
                        // nothing.
                        Verdict::Mismatch | Verdict::FoundPacked(_, _) => {}
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
                    match ae.verdict {
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
                        Verdict::Mismatch | Verdict::FoundPacked(_, _) => {}
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

                let mut child_pairs: Vec<(u32, u32)> = Vec::new();

                // Spec 0175 S3: whether a packed varint run terminates at the
                // payload end depends on the payload alone, so it is scanned at
                // most once per LEN tag and memoized across every candidate
                // that reads this payload as one. Written as the natural inner
                // check it would be O(A × payload) per tag — the exact shape
                // spec 0173 removed from the verdict loop one arm above.
                let mut packed_varints_ok: Option<bool> = None;

                for ae in active.iter_mut() {
                    match ae.verdict {
                        Verdict::Unknown => {
                            for &e in &ae.entries {
                                ws.scores[e as usize].unknowns += 1;
                            }
                        }
                        Verdict::FoundPacked(child, elem_wt) => {
                            let run_ok = match elem_wt as u32 {
                                WT_I64 => payload.len().is_multiple_of(8),
                                WT_I32 => payload.len().is_multiple_of(4),
                                _ => *packed_varints_ok
                                    .get_or_insert_with(|| packed_varints_terminate(payload)),
                            };
                            if !run_ok {
                                for &e in &ae.entries {
                                    ws.set_vetoed(e, || {
                                        format!(
                                            "invalid packed run on field {field_number} \
                                             (element wire type {elem_wt}, {} payload bytes)",
                                            payload.len()
                                        )
                                    });
                                }
                                ae.entries.clear();
                                continue;
                            }

                            // A zero-length run is legal but no conformant
                            // writer emits one — protoc omits the field
                            // instead. Legal-but-suspicious is what
                            // `non_canonical` is for.
                            if payload.is_empty() {
                                for &e in &ae.entries {
                                    ws.scores[e as usize].non_canonical += 1;
                                }
                            }

                            // Spec 0175 S4: a packed element is scored by the
                            // same rules as the expanded encoding of the same
                            // value. Leaves with nothing to check are skipped
                            // outright: only int32/uint32 (the 32-bit gap) and
                            // bool/enum (the range) have a per-element verdict.
                            let node = find_node(ws.graph, child);
                            let needs_element_check = elem_wt as u32 == WT_VARINT
                                && node.is_some_and(|n| {
                                    matches!(n.wire_type, 8 | 9)
                                        || n.range_idx.to_native() != 0xFFFF
                                });
                            let mut do_veto = false;
                            if needs_element_check {
                                let mut p = 0usize;
                                while p < payload.len() {
                                    let vr = parse_varint(payload, p);
                                    p = vr.next_pos;
                                    if check_varint_value(ws, ae, node, vr.value, vr.overhang) {
                                        do_veto = true;
                                        break;
                                    }
                                }
                            }
                            if do_veto {
                                for &e in &ae.entries {
                                    ws.set_vetoed(e, || {
                                        format!(
                                            "packed varint value out of range on field \
                                             {field_number}"
                                        )
                                    });
                                }
                                ae.entries.clear();
                            } else {
                                // One wire occurrence, not one per element:
                                // awarding N would make the two legal encodings
                                // of identical data score differently, and since
                                // every candidate sees the same bytes the
                                // inflation discriminates between nothing.
                                record_occurrence(&mut ae.occurrences, field_number as u32);
                                for &e in &ae.entries {
                                    ws.scores[e as usize].matches += 1;
                                }
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
                                        ws.set_vetoed(e, || {
                                            format!("invalid UTF-8 on string field {field_number}")
                                        });
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
                let mut recurse_into: Vec<(u32, u32)> = Vec::new();
                let mut stay_out_entries: Vec<u32> = Vec::new();

                for ae in active.iter_mut() {
                    match ae.verdict {
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
                        Verdict::Mismatch | Verdict::FoundPacked(_, _) => {} // already vetoed above
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
                        if matches!(ae.verdict, Verdict::Found(_, _)) {
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
                        .all(|ae| !matches!(ae.verdict, Verdict::Found(_, _)))
                {
                    // All Found entries were vetoed; need blind walk for stay_out boundary.
                    match parse_group_blind(buf, pos, field_number) {
                        None => {
                            // stay_out entries also can't parse it — veto them too.
                            for ae in active.iter_mut() {
                                if matches!(ae.verdict, Verdict::Unknown) {
                                    for &e in &ae.entries {
                                        ws.set_vetoed(e, || {
                                            "malformed group (blind fallback)".to_string()
                                        });
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
                    match ae.verdict {
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
                        Verdict::Mismatch | Verdict::FoundPacked(_, _) => {}
                    }
                }
            }

            _ => unreachable!("wire type > 5 caught by parse_wiretag"),
        }
    }
}

// ── Tests: `set_vetoed`'s lazy reason (spec 0173 S2) ─────────────────────────

#[cfg(test)]
mod set_vetoed_tests {
    use super::*;
    use std::cell::Cell;

    fn walk_state<'a, 'g>(
        graph: &'a ArchivedCompiledGraph,
        scores: &'a mut Vec<EntryScore<'g>>,
        debug_fqdn: Option<String>,
    ) -> WalkState<'a, 'g> {
        let words = scores.len().div_ceil(64);
        WalkState {
            graph,
            scores,
            vetoed: vec![0u64; words],
            debug_fqdn,
            expand_any: true,
        }
    }

    /// A one-field graph, only ever used to satisfy `WalkState`'s `graph`
    /// field — `set_vetoed` never reads it.
    fn tiny_graph() -> crate::score::load::LoadedGraph {
        use crate::build_scoring_graph::load::{FieldLabel, Merged, ScoringField, ScoringKind};
        use crate::build_scoring_graph::{graph, hopcroft, serial};

        let mut states = std::collections::HashMap::new();
        states.insert(
            "pkg.Msg".to_string(),
            vec![ScoringField {
                number: 1,
                kind: ScoringKind::Uint64,
                child: None,
                range: None,
                label: FieldLabel::Optional,
            }],
        );
        let merged = Merged {
            states,
            node_kinds: std::collections::HashMap::new(),
            roots: vec!["pkg.Msg".to_string()],
        };
        let (raw, reg) = graph::build(&merged);
        let partition = hopcroft::minimize(&raw, &reg, &raw.node_wire_types, |_, _| {});
        let compiled = graph::compile(&raw, &reg, &partition, &merged.roots);
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tiny.bin");
        serial::write(&compiled, &path).expect("write");
        let _ = std::mem::ManuallyDrop::new(dir);
        crate::score::load::load_graph(&path).expect("load")
    }

    fn entry(fqdn: &str) -> EntryScore<'_> {
        EntryScore {
            fqdn,
            matches: 0,
            unknowns: 0,
            out_of_range: 0,
            non_canonical: 0,
            mismatches: 0,
            vetoed: false,
        }
    }

    /// The reason is a closure precisely so it costs nothing in production,
    /// where `PROTOTEXT_DEBUG_FQDN` is unset. Constructed directly rather
    /// than through `WalkState::new`, which reads that variable from the
    /// process environment — a value the rest of the test binary's threads
    /// share.
    #[test]
    fn reason_is_not_built_without_a_debug_fqdn() {
        let graph = tiny_graph();
        let mut scores = vec![entry("pkg.Msg")];
        let mut ws = walk_state(&graph, &mut scores, None);

        let built = Cell::new(false);
        ws.set_vetoed(0, || {
            built.set(true);
            "why".to_string()
        });

        assert!(ws.scores[0].vetoed, "the veto itself must still land");
        assert!(!built.get(), "no reader, so the reason must not be built");
    }

    /// …and it *is* built when the debug FQDN names this candidate, which is
    /// the one case the old eager `&str` served.
    #[test]
    fn reason_is_built_for_the_named_debug_fqdn() {
        let graph = tiny_graph();
        let mut scores = vec![entry("pkg.Msg"), entry("pkg.Other")];
        let mut ws = walk_state(&graph, &mut scores, Some("pkg.Msg".to_string()));

        let built = Cell::new(0u32);
        ws.set_vetoed(1, || {
            built.set(built.get() + 1);
            "why".to_string()
        });
        assert_eq!(built.get(), 0, "a different candidate: still not built");

        ws.set_vetoed(0, || {
            built.set(built.get() + 1);
            "why".to_string()
        });
        assert_eq!(built.get(), 1, "the named candidate: built once");

        // Already vetoed — the early return must precede the closure.
        ws.set_vetoed(0, || {
            built.set(built.get() + 1);
            "why".to_string()
        });
        assert_eq!(built.get(), 1, "a repeat veto must not build it again");
    }
}

// ── Tests: `record_occurrence`'s saturating count (spec 0179 S2) ─────────────

#[cfg(test)]
mod record_occurrence_tests {
    use super::*;

    /// The count is a `u32` since spec 0179 S2, so it *can* be driven to its
    /// ceiling — where the previous `u64` could not. Reaching it through the
    /// walk would take a single message frame over 4 GiB, so the function is
    /// called directly: the guarantee is the same and it costs nothing.
    ///
    /// What this guards against is a silent wrap in a release build, where
    /// `overflow-checks` is off and a plain `+= 1` would turn `u32::MAX`
    /// occurrences into zero — reading, downstream, as "the field was never
    /// seen", which is a *worse* answer than "seen very many times".
    #[test]
    fn the_count_saturates_rather_than_wrapping() {
        let mut occ: SmallVec<[(u32, u32); 2]> = SmallVec::new();
        occ.push((7, u32::MAX));

        record_occurrence(&mut occ, 7);

        assert_eq!(occ.as_slice(), &[(7, u32::MAX)]);
    }

    /// Insertion keeps the vec sorted by field number, which is what makes the
    /// `binary_search_by_key` in `apply_cardinality_multi` valid. Fields arrive
    /// in wire order, which is attacker-chosen and need not be ascending.
    #[test]
    fn entries_stay_sorted_regardless_of_arrival_order() {
        let mut occ: SmallVec<[(u32, u32); 2]> = SmallVec::new();
        for f in [9, 2, 5, 2] {
            record_occurrence(&mut occ, f);
        }
        assert_eq!(occ.as_slice(), &[(2, 2), (5, 1), (9, 1)]);
    }
}
