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

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use prototext_core::helpers::{payload_end, MAX_WIRE_DEPTH};
use smallvec::SmallVec;

use crate::build_scoring_graph::serial::{ArchivedCompiledGraph, ArchivedNodeEntry, NO_EXT_RANGES};

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
    /// Byte offset at which this root stopped consuming (spec 0238 S14).
    ///
    /// Under [`Policy::Scan`] that is the termination offset — the first byte
    /// of the tag this root could not carry — or `pb.len()` if the root
    /// consumed the whole buffer. Under [`Policy::Score`] it is `pb.len()`
    /// unconditionally, so every result is a `(score, termination)` pair and
    /// no caller has to branch on the policy.
    ///
    /// A `usize` rather than an `Option<usize>`: termination fires *before* a
    /// field is consumed, so a root cannot terminate at `pb.len()` —
    /// "terminated at the end" and "ran to the end" are the same fact, and
    /// there is no second state to encode. `Option<usize>` would also be 16
    /// bytes to `usize`'s 8, which at 49 255 corpus roots is 400 KB of extra
    /// result per `score_all` call.
    ///
    /// **Meaningless on a vetoed entry**, where it is `pb.len()` under both
    /// policies: a veto fires part-way through a field already consumed, at
    /// an offset that is not a record boundary and may lie past the true end
    /// of the record.
    pub termination: usize,
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
    /// An undeclared field number that falls inside this state's declared
    /// extension range set (spec 0238 S15). Produced only under
    /// [`Policy::Scan`]. Scored exactly like `Unknown` except that it carries
    /// no `unknowns` penalty: an extension's type is unknown, so it can be
    /// neither validated nor held against the candidate.
    Extension,
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
    /// Bumped by `set_vetoed` whenever a bit in `vetoed` goes from 0 to 1.
    ///
    /// `propagate_vetoes` exists to remove, from the *parent* frame's active
    /// set, entries a recursion vetoed. Comparing this across the recursion
    /// says whether the recursion vetoed anything at all, and if it did not
    /// there is nothing to remove — so the rescan of every entry in every
    /// `ActiveEntry` can be skipped outright (spec 0292).
    veto_epoch: u64,
    /// If set, print a message to stderr whenever this FQDN is vetoed.
    debug_fqdn: Option<String>,
    expand_any: bool,
    policy: Policy,
    /// See [`score_subset`]'s `cancel` parameter.
    cancel: Option<&'a AtomicBool>,
    /// The decoded `(value, overhang)` elements of the packed varint payload
    /// currently under the LEN arm, filled once per token by
    /// [`packed_varints_terminate`] and read by every candidate that reads that
    /// payload as a packed run (spec 0288 S1/S2).
    ///
    /// It lives here, rather than in the arm, so that its capacity survives the
    /// token: it is cleared per payload and reaches the high-water mark of the
    /// longest run in the blob, keeping spec 0179's no-allocation-in-steady-
    /// state posture. The arm takes it out with `mem::take` for the duration of
    /// the candidate loop, because `check_varint_value` borrows the whole
    /// `WalkState` mutably.
    packed_scratch: Vec<(u64, u64)>,
    /// Per-token memo of [`packed_run_verdict`], keyed on the child state id
    /// (spec 0289 S4).
    ///
    /// The run's verdict depends only on the payload and the leaf, so every
    /// candidate group resolving to the same child recomputes a bit-identical
    /// answer and breaks at the identical element. Spec 0288 shared the
    /// *decode* across those candidates and measured the remaining redundancy
    /// at 35x on googleapis: 875 123 235 `check_varint_value` calls against
    /// 25 019 063 distinct elements.
    ///
    /// A `Vec` scanned linearly rather than a map: the number of *distinct*
    /// children under one LEN tag is a handful, and at that size a scan beats
    /// hashing. Lives here, and is `mem::take`n by the arm, for the same two
    /// reasons as `packed_scratch` — capacity survives the token, and the
    /// application step below borrows `ws` mutably.
    elem_verdicts: Vec<(u32, ValueVerdict)>,
    /// `fqdn -> state_id` over every root, built on the first `Any` this walk
    /// resolves and not at all if it resolves none (spec 0291).
    ///
    /// The `Any` arm used to answer `type_url` with a linear scan over
    /// `graph.roots`, deref'ing an `ArchivedString` per root — 49 255 of them
    /// on googleapis, per occurrence. That showed up as **8.73% of a protolens
    /// startup** in rkyv's `string/repr.rs` alone, plus most of a further
    /// 1.61% in `slice::cmp`.
    ///
    /// Built lazily because a payload with no `Any` must not pay for it, and
    /// per `WalkState` — i.e. once per part — because the build amortizes over
    /// every occurrence in that part's walk. Measured on googleapis startup:
    /// 24 walks, 23 313 resolutions, so ~971 lookups pay for each build.
    any_index: Option<HashMap<&'a str, u32>>,
}

impl<'a, 'g> WalkState<'a, 'g> {
    fn new(
        graph: &'a ArchivedCompiledGraph,
        scores: &'a mut Vec<EntryScore<'g>>,
        opts: &ScoringOpts,
        cancel: Option<&'a AtomicBool>,
    ) -> Self {
        let n = scores.len();
        let words = n.div_ceil(64);
        WalkState {
            graph,
            scores,
            vetoed: vec![0u64; words],
            veto_epoch: 0,
            debug_fqdn: std::env::var("PROTOTEXT_DEBUG_FQDN").ok(),
            expand_any: opts.expand_any,
            policy: opts.policy,
            cancel,
            packed_scratch: Vec::new(),
            elem_verdicts: Vec::new(),
            any_index: None,
        }
    }

    /// The root state an `Any`'s `type_url` names, or `None` if the graph has
    /// no root by that name (spec 0291).
    ///
    /// The index is built on the first call and reused for the rest of the
    /// walk. `or_insert` keeps the *first* root of a duplicated name, which is
    /// what the linear `find` it replaced returned.
    fn resolve_any_root(&mut self, fqdn: &str) -> Option<u32> {
        let graph = self.graph;
        let index = self.any_index.get_or_insert_with(|| {
            let mut m = HashMap::with_capacity(graph.roots.len());
            for r in graph.roots.iter() {
                m.entry(r.fqdn.as_str()).or_insert(r.state_id.to_native());
            }
            m
        });
        index.get(fqdn).copied()
    }

    /// `Relaxed` because the flag carries no data: the walk reads it to
    /// decide whether to keep going, and everything it would need to
    /// observe alongside it is either immutable or already discarded by
    /// the setter.
    fn cancelled(&self) -> bool {
        self.cancel.is_some_and(|c| c.load(Ordering::Relaxed))
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
        self.veto_epoch += 1;
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

/// What the walk is being asked to find out (spec 0238 S11).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Policy {
    /// Score the whole buffer against every root: how well does this blob fit
    /// this type? Every root consumes `pb` to its end.
    #[default]
    Score,
    /// Score, and additionally find where each root's *record* ends — the
    /// first field that a well-formed instance of that type could not carry.
    Scan,
}

/// Options controlling walk behaviour.
///
/// New fields land here rather than in a new argument, so callers should
/// build one with `..Default::default()` rather than field by field. Rust has
/// no default-valued struct fields, so a complete literal has to name every
/// one of them and breaks on each addition (spec 0238 S11).
pub struct ScoringOpts {
    /// If true (default), google.protobuf.Any fields are expanded using the
    /// type resolved from type_url and scored against the wrapped type.
    /// If false (--no-expand-any), value scores as a plain bytes match.
    pub expand_any: bool,
    /// Defaults to [`Policy::Score`], the behaviour every caller had before
    /// the policy existed.
    pub policy: Policy,
}

impl Default for ScoringOpts {
    fn default() -> Self {
        Self {
            expand_any: true,
            policy: Policy::default(),
        }
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
    score_subset(pb, graph, opts, &all, None)
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
///
/// `cancel`, when supplied, is polled once per wire field at every
/// recursion level. Once it reads `true` the walk stops where it is and
/// unwinds, and **the scores returned are partial and meaningless** — a
/// candidate abandoned halfway through the blob has counted only the
/// matches it happened to reach. The caller sets the flag and so already
/// knows to discard the result; nothing is reported back here, because a
/// second return channel would only restate what the caller just did.
///
/// This exists because the walk is the one place in the workspace that
/// can run for seconds with no interruption point, which is long enough
/// for a UI shutting down to look hung while it waits for the walking
/// thread to join.
pub fn score_subset<'g>(
    pb: &[u8],
    graph: &'g ArchivedCompiledGraph,
    opts: &ScoringOpts,
    roots: &[u32],
    cancel: Option<&AtomicBool>,
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

    // Spec 0238 S9: extension ranges are a *precondition* of `Scan`, not a
    // modifier of it. `Scan` is strict — an empty range set means the message
    // permits no extension and an undeclared field ends the record — and a
    // graph built without reproto's flag has an empty set on every message.
    // Honoring that silently would terminate on the first custom option of
    // every descriptor and return plausible, wrong boundaries, so missing data
    // has to fail loudly rather than be read as an answer.
    assert!(
        opts.policy != Policy::Scan || graph.has_extension_ranges,
        "Policy::Scan needs a graph built with extension ranges; rebuild the \
         schema database with reproto's --emit-extension-ranges"
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
            // Overwritten only by an S12 termination, so "ran to the end" is
            // the value a root keeps by not stopping.
            termination: pb.len(),
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

    let mut ws = WalkState::new(graph, &mut scores, opts, cancel);
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
/// **Balancing counts groups, not roots** (spec 0290). Having just said
/// that a group is the unit of work, weighing a part by how many *roots*
/// it holds contradicts it, and the contradiction was measured: on
/// googleapis three of twenty-four parts held 23.7% of the roots and did
/// **0.82% of the work**, because one group is one traversal however
/// many roots share it. Those three parts were effectively idle and the
/// pool was 21 wide, not 24.
///
/// Neither key *predicts* cost — parts 3-23 were within 3.8% on group
/// count and varied 10x in time — so this buys 1-3% of makespan, not
/// more. It is a correctness-of-intent fix; the straggler is handled by
/// spec 0269's seat donation instead.
///
/// Groups are handed out largest-first, one per part in turn. With group
/// count as the key that *is* round-robin — every part is tied until the
/// wheel comes round, and `min_by_key` would return the first tie each
/// time — so it is written as the modulo directly, which also drops the
/// hand-out from O(G·n) to O(G).
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

    let k = n.min(groups.len());
    let mut parts: Vec<Vec<u32>> = vec![Vec::new(); k];
    for (i, group) in groups.into_iter().enumerate() {
        parts[i % k].extend(group);
    }
    // Round-robin over at least `k` groups cannot leave a part empty
    // (there are at least as many groups as parts). Sorting each part is not
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
    score_subset(pb, graph, opts, &[idx], None).pop()
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

/// True iff `state_id` declares an extension range containing `field_number`
/// (spec 0238 S12 rule 1, S15).
///
/// False for a state that declares none: `NO_EXT_RANGES` is what a *closed*
/// message says, and a closed message admits no unknown field at all.
fn in_ext_range(graph: &ArchivedCompiledGraph, state_id: u32, field_number: u32) -> bool {
    let Some(node) = find_node(graph, state_id) else {
        return false;
    };
    let idx = node.ext_range_idx.to_native();
    if idx == NO_EXT_RANGES {
        return false;
    }
    let set = &graph.ext_range_sets[idx as usize];
    let start = set.offset.to_native() as usize;
    let end = start + set.len.to_native() as usize;
    // Canonical sets are ascending, disjoint and non-adjacent (spec 0238 S3),
    // so a linear scan could stop early — but a set holds a handful of ranges
    // (three, on googleapis), and the branch to stop costs more than the loop.
    graph.ext_ranges[start..end]
        .iter()
        .any(|r| r.0.to_native() <= field_number && field_number <= r.1.to_native())
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

/// True iff `payload` is a run of varints ending exactly at its end, filling
/// `out` with the `(value, overhang)` of each element on the way.
///
/// A varint whose continuation bit runs past the payload, or that overflows 64
/// bits, makes the run *impossible* rather than merely unlikely — which is what
/// separates this check from a `non_canonical` penalty.
///
/// Spec 0288 S1: this scan is memoized per token, and it already decodes every
/// element. Keeping the values is what lets the per-candidate element check
/// below read them instead of re-deriving them once per active entry group —
/// the decode is shared, the verdict is not. `out` is truncated to zero on
/// entry, so a caller may hand in a buffer of any prior length; on a `false`
/// return its contents are a meaningless prefix, which is safe because the
/// caller tests the run before reading them (S5).
fn packed_varints_terminate(payload: &[u8], out: &mut Vec<(u64, u64)>) -> bool {
    out.clear();
    let mut p = 0usize;
    while p < payload.len() {
        let vr = parse_varint(payload, p);
        if vr.garbage.is_some() {
            return false;
        }
        p = vr.next_pos;
        out.push((vr.value, vr.overhang));
    }
    true
}

/// What one varint value costs one candidate: the penalties it accrues and
/// whether it vetoes.
///
/// Spec 0289 S1. This is the whole reason the check can be cached. Every
/// quantity here is a function of `(graph, node, val, overhang)` and of nothing
/// else — in particular not of the active entry group, which contributes only
/// the *set of entries* the counts are then added to. Separating the two lets
/// one derivation serve every candidate that resolved to the same leaf.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct ValueVerdict {
    /// The value is impossible for this leaf, so the candidate is dead.
    vetoed: bool,
    /// Overhang, and negatives written in the truncated 5-byte form.
    non_canonical: u64,
    /// Outside a `Range` leaf's declared extent — a penalty, never a veto.
    out_of_range: u64,
}

/// Per-element varint checks, shared by the expanded and the packed encoding of
/// the same values (spec 0175 S4): the value-overhang penalty, C5's 32-bit gap
/// veto, the `non_canonical` penalty for a negative written in the truncated
/// 5-byte form, and the `Range` leaf's `[min, max]` test.
///
/// Spec 0289 S1: this used to take `&mut WalkState` and `&ActiveEntry` and
/// write the penalties straight into `ws.scores`, which is what tied it to a
/// single candidate and made it uncacheable. It now returns them. The caller
/// owns applying them, and the veto and match bookkeeping, which differ between
/// one expanded occurrence and one element of a packed run.
#[inline]
fn check_varint_value(
    graph: &ArchivedCompiledGraph,
    node: Option<&ArchivedNodeEntry>,
    val: u64,
    overhang: u64,
) -> ValueVerdict {
    let mut v = ValueVerdict {
        vetoed: false,
        non_canonical: u64::from(overhang > 0),
        out_of_range: 0,
    };
    let Some(n) = node else {
        return v;
    };
    let ri = n.range_idx.to_native();
    match n.wire_type {
        9 => {
            // INT32: veto if in invalid gap (0xFFFF_FFFF, 0xFFFF_FFFF_8000_0000)
            if val > 0xFFFF_FFFF && val < 0xFFFF_FFFF_8000_0000u64 {
                v.vetoed = true;
                return v;
            }
            if (0x8000_0000u64..=0xFFFF_FFFFu64).contains(&val) {
                // truncated negative int32 (4-byte encoding)
                v.non_canonical += 1;
            }
            v
        }
        8 => {
            // UINT32: veto if > 32-bit
            v.vetoed = val > 0xFFFF_FFFF;
            v
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
                v.vetoed = true;
                return v;
            }
            if (0x8000_0000u64..=0xFFFF_FFFFu64).contains(&val) {
                // Negative written in the non-canonical 5-byte form.
                v.non_canonical += 1;
            }
            let Some(range) = graph.ranges.get(ri as usize) else {
                return v;
            };
            let (min, max) = (range.0.to_native() as i64, range.1.to_native() as i64);
            // The explicit `as u32` step makes this correct for both encodings:
            // it truncates the sign-extended form back to 32 bits, and is a
            // no-op on the 5-byte one.
            let signed = val as u32 as i32 as i64;
            if signed >= min && signed <= max {
                return v;
            }
            // Spec 0178 S2: a penalty, never a veto. Out of range is not out of
            // bounds — a `bool` is `value != 0` in every generated parser, so 2
            // reads as `true`, and a closed enum moves an undeclared number to
            // the unknown-field set rather than failing the parse. Both are
            // strong evidence against the candidate, which is what
            // `out_of_range`'s -15 is for, but neither is impossible, and the
            // governing principle vetoes only the impossible.
            v.out_of_range += 1;
            v
        }
        _ => v, // UINT64 or non-varint: no range check
    }
}

/// Add one [`ValueVerdict`]'s penalties to every entry of one candidate group.
///
/// Spec 0289 S2: the half of the old `check_varint_value` that genuinely
/// depends on `ae`. Kept separate so the derivation above can be shared while
/// this still runs per candidate.
#[inline]
fn apply_value_verdict(ws: &mut WalkState<'_, '_>, ae: &ActiveEntry, v: ValueVerdict) {
    if v.non_canonical == 0 && v.out_of_range == 0 {
        return;
    }
    for &e in &ae.entries {
        let s = &mut ws.scores[e as usize];
        s.non_canonical += v.non_canonical;
        s.out_of_range += v.out_of_range;
    }
}

/// The verdict of a whole packed varint run against one leaf, as a single
/// summed [`ValueVerdict`].
///
/// Spec 0289 S3. Order and break-on-first-veto are those of the element loop
/// this replaces, so the accumulated counts are still the offenders *before*
/// the break, exactly as spec 0288 S4 required of the per-element form.
fn packed_run_verdict(
    graph: &ArchivedCompiledGraph,
    node: Option<&ArchivedNodeEntry>,
    elements: &[(u64, u64)],
) -> ValueVerdict {
    let mut acc = ValueVerdict::default();
    for &(value, overhang) in elements {
        let v = check_varint_value(graph, node, value, overhang);
        acc.non_canonical += v.non_canonical;
        acc.out_of_range += v.out_of_range;
        if v.vetoed {
            acc.vetoed = true;
            break;
        }
    }
    acc
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
///
/// `since` is `ws.veto_epoch` read immediately before that recursion. If the
/// recursion vetoed nothing the epoch is unchanged and there is nothing to
/// remove, so the scan is skipped (spec 0292). This is overwhelmingly the
/// common case: measured over a googleapis startup, **1 071 701 of
/// 1 072 120 calls (99.96%)** veto nothing, and skipping them drops
/// **107.8 M of 107.9 M entry visits (99.90%)**. The scan is over every
/// entry of every `ActiveEntry`, which near the top of a walk is the whole
/// part's surviving candidate set — once per LEN child.
///
/// The skip is sound only because every veto raised *before* the recursion
/// has already been taken out of `active` by the arm that raised it (each
/// `set_vetoed` site is immediately followed by `ae.entries.clear()`, and
/// the LEN arm then drops the emptied `ActiveEntry`s). The `debug_assert`
/// is that invariant, checked on every skip.
fn propagate_vetoes(active: &mut Vec<ActiveEntry>, ws: &WalkState, since: u64) {
    if ws.veto_epoch == since {
        debug_assert!(
            active
                .iter()
                .all(|ae| ae.entries.iter().all(|&e| !ws.is_vetoed(e))),
            "propagate_vetoes skipped a live veto: an entry was vetoed \
             before the recursion without being removed from `active`",
        );
        return;
    }
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

/// True iff `tag` is the first field of the *next* record for this entry —
/// i.e. the record `ae` has been reading ends here (spec 0238 S12).
///
/// Evaluated per `ActiveEntry`, so per depth-0 *state* rather than per root:
/// entries sharing a state share their declared fields, their range set and
/// their occurrence counts, and so necessarily terminate together.
fn scan_terminates(graph: &ArchivedCompiledGraph, ae: &ActiveEntry, tag: &TagResult) -> bool {
    // A field number of 0 or >= 2^29 is undeclarable and unrangeable — no
    // extension clause can reach it — so rule 1 fires without a lookup.
    if tag.out_of_range {
        return true;
    }
    let field_number = tag.field_number as u32;
    match find_transition(graph, ae.state_id, field_number) {
        // Rule 1. Strict by construction: a state with an empty range set has
        // declared itself closed, and an undeclared field number in a closed
        // state is a boundary.
        None => !in_ext_range(graph, ae.state_id, field_number),
        // Rule 2. A singular field cannot appear twice in one record, so its
        // second appearance belongs to the next one. This is what makes a
        // `FileDescriptorSet` legible: the outer record header is a second
        // field 1. `required` terminates for the same reason `optional` does
        // — the rule is about cardinality, and a repeated `required` is a
        // repeated singular. (Under `Score` it stays a `mismatches`
        // candidate; nothing here changes that.)
        Some(tr) => {
            tr.label != LABEL_REPEATED
                && ae
                    .occurrences
                    .binary_search_by_key(&field_number, |&(f, _)| f)
                    .is_ok()
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

        // The walk's only interruption point (see `score_subset`'s
        // `cancel`). Reporting the buffer as fully consumed is what makes
        // the unwind immediate: a group's caller takes this as the
        // group's end position and so stops on its very next check, and a
        // submessage's caller — which walks a different buffer and
        // ignores the return — stops on that same check one field later.
        if ws.cancelled() {
            return buflen;
        }

        // ── Parse wire tag ────────────────────────────────────────────────────

        // Saved before the tag is decoded because this — not `pos` — is what
        // an S12 termination reports (spec 0238 S13). By the time the verdicts
        // below are known, `pos` has advanced past the tag and any length
        // prefix, so reading it at the termination point yields a boundary
        // several bytes late.
        let tag_start = pos;

        let tag = parse_wiretag(buf, pos);
        if tag.garbage.is_some() {
            veto_all(&mut active, ws, "garbage wire tag");
            return buflen;
        }
        let field_number = tag.field_number;
        let wire_type = tag.wire_type;
        pos = tag.next_pos;

        // ── SCAN termination (spec 0238 S12-S13) ──────────────────────────────
        //
        // Ahead of every penalty and every verdict, because a terminated entry
        // must not be charged for the tag that ended it: at this instant both
        // its score and its occurrences describe exactly the record that
        // ended, and so nothing has to be rolled back.
        //
        // Termination is recorded, not obeyed by the *walk*: roots terminate
        // at different offsets — each has its own singular fields and its own
        // extension ranges — so a walk that halted at the first one would
        // truncate every other root's score. One pass, N independent offsets.
        if ws.policy == Policy::Scan && depth == 0 {
            for ae in active.iter_mut() {
                if !scan_terminates(ws.graph, ae, &tag) {
                    continue;
                }
                // At the termination point, not at EOF: a `required` field
                // that would have appeared after the boundary is genuinely
                // absent from the record that ended, and deferring this pass
                // would instead judge it against occurrences polluted by the
                // record that follows.
                apply_cardinality_multi(ws.graph, ae, ws.scores);
                for &e in &ae.entries {
                    ws.scores[e as usize].termination = tag_start;
                }
                ae.entries.clear();
            }
            active.retain(|ae| !ae.entries.is_empty());
            if active.is_empty() {
                return tag_start;
            }
        }

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
                    None => {
                        // Spec 0238 S15: an unknown field the message has
                        // declared room for is neither evidence for nor
                        // against — its type is unknown, so it cannot be
                        // validated. Read only under `Scan`; under `Score` an
                        // unknown is an unknown even on a graph that carries
                        // range data.
                        if ws.policy == Policy::Scan
                            && in_ext_range(ws.graph, ae.state_id, field_number as u32)
                        {
                            Verdict::Extension
                        } else {
                            Verdict::Unknown
                        }
                    }
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
                            let v = check_varint_value(ws.graph, node, val, vr.overhang);
                            apply_value_verdict(ws, ae, v);
                            if v.vetoed {
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
                        // Spec 0238 S15: an extension is read like an unknown
                        // but costs nothing, so there is nothing to do.
                        Verdict::Extension => {}
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
                        // Spec 0238 S15: an extension is read like an unknown
                        // but costs nothing, so there is nothing to do.
                        Verdict::Extension => {}
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

                // Spec 0288 S6: likewise for UTF-8 validity, which is a
                // property of the bytes and was being re-derived at the
                // `is_string` test once per candidate reading this payload as a
                // string. `is_string` itself stays per candidate.
                let mut payload_utf8_ok: Option<bool> = None;

                // Spec 0288 S2: out of `ws` for the loop, back in after it, so
                // the elements decoded by the memoized scan can be read while
                // `check_varint_value` holds `&mut ws`. Three word moves, and
                // the allocation is carried to the next token.
                let mut packed_vals = std::mem::take(&mut ws.packed_scratch);

                // Spec 0289 S4: same trick, same reason. Cleared per token —
                // the verdicts memoized here are answers about *this* payload.
                let mut elem_verdicts = std::mem::take(&mut ws.elem_verdicts);
                elem_verdicts.clear();

                // `ws.graph` is a shared reference, so copying it out lets the
                // derivation run while `apply_value_verdict` holds `&mut ws`.
                let graph = ws.graph;

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
                                _ => *packed_varints_ok.get_or_insert_with(|| {
                                    packed_varints_terminate(payload, &mut packed_vals)
                                }),
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
                                // Spec 0288 S5: `needs_element_check` implies
                                // `elem_wt == WT_VARINT`, which took the `_`
                                // arm above, and `run_ok` was tested first and
                                // `continue`d on failure — so `packed_vals`
                                // holds this payload's elements, in order.
                                // Load-bearing: reordering those two tests
                                // breaks it.
                                debug_assert_eq!(
                                    packed_varints_ok,
                                    Some(true),
                                    "element check reached without a completed packed scan",
                                );
                                // Spec 0289 S4: derive once per distinct child,
                                // apply once per candidate. The scan over the
                                // memo is over distinct children under this one
                                // tag, not over candidates.
                                let v = match elem_verdicts.iter().find(|(c, _)| *c == child) {
                                    Some(&(_, v)) => v,
                                    None => {
                                        let v = packed_run_verdict(graph, node, &packed_vals);
                                        elem_verdicts.push((child, v));
                                        v
                                    }
                                };
                                apply_value_verdict(ws, ae, v);
                                do_veto = v.vetoed;
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
                                if is_string
                                    && !*payload_utf8_ok
                                        .get_or_insert_with(|| std::str::from_utf8(payload).is_ok())
                                {
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
                        // Spec 0238 S15: an extension is read like an unknown
                        // but costs nothing, so there is nothing to do. Not
                        // recursed into either — its type is unknown, so
                        // there is no state to recurse *with*.
                        Verdict::Extension => {}
                        Verdict::Mismatch => {}
                    }
                }
                // Before the recursion below, which needs `ws` whole — and
                // which is why the buffer cannot be borrowed across it.
                ws.packed_scratch = packed_vals;
                ws.elem_verdicts = elem_verdicts;

                active.retain(|ae| !ae.entries.is_empty());

                // Read after the retain above, which is what makes `active`
                // free of already-vetoed entries — the precondition
                // `propagate_vetoes`' skip relies on.
                let veto_epoch = ws.veto_epoch;

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
                                ws.resolve_any_root(fqdn)
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
                    propagate_vetoes(&mut active, ws, veto_epoch);
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
                        // Spec 0238 S15: stays out of the group like an
                        // `Unknown`, but is not pushed to `stay_out_entries`
                        // — that list exists only to charge the unknowns.
                        // The entry still advances to the group's end below,
                        // which `parse_group_blind` supplies when no entry
                        // recursed.
                        Verdict::Extension => {}
                        Verdict::Mismatch | Verdict::FoundPacked(_, _) => {} // already vetoed above
                    }
                }

                let new_pos = if !recurse_into.is_empty() {
                    // Recurse with schema — boundaries are determined by the group walk.
                    let child_active = group_by_state(recurse_into.iter().copied());
                    let veto_epoch = ws.veto_epoch;
                    let np = score_message_multi(
                        buf,
                        pos,
                        child_active,
                        Some(field_number),
                        ws,
                        depth + 1,
                    );
                    propagate_vetoes(&mut active, ws, veto_epoch);
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
                                if matches!(ae.verdict, Verdict::Unknown | Verdict::Extension) {
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
                        // Spec 0238 S15: an extension is read like an unknown
                        // but costs nothing, so there is nothing to do.
                        Verdict::Extension => {}
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
            veto_epoch: 0,
            debug_fqdn,
            expand_any: true,
            policy: Policy::Score,
            cancel: None,
            packed_scratch: Vec::new(),
            elem_verdicts: Vec::new(),
            any_index: None,
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
            ..Default::default()
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
            termination: 0,
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

// ── Tests: the packed scratch buffer (spec 0288 S1/S2) ───────────────────────

#[cfg(test)]
mod packed_scratch_tests {
    use super::*;

    fn varint(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return out;
            }
            out.push(b | 0x80);
        }
    }

    fn run_of(n: usize) -> Vec<u8> {
        (0..n).flat_map(|i| varint(i as u64)).collect()
    }

    /// Spec 0288 S2: the buffer reaches the high-water mark of the longest run
    /// and stays there. Spec 0179's no-allocation-in-steady-state posture is a
    /// constraint on this spec, and the walk visits one token after another
    /// with the same buffer — so a `Vec::new()` per token, or a `shrink`, would
    /// put an allocation back on the hot path once per LEN tag.
    ///
    /// The warmup is the whole point: the first call is *expected* to grow.
    /// What is asserted is that no later call does, including the ones that are
    /// longer than their immediate predecessor but shorter than the mark.
    #[test]
    fn packed_scratch_does_not_grow_after_warmup() {
        let mut scratch = Vec::new();

        assert!(packed_varints_terminate(&run_of(1024), &mut scratch));
        let mark = scratch.capacity();
        assert_eq!(scratch.len(), 1024);

        for n in [1usize, 512, 3, 1023, 700, 1024] {
            assert!(packed_varints_terminate(&run_of(n), &mut scratch));
            assert_eq!(scratch.len(), n, "the run's elements, and only those");
            assert_eq!(
                scratch.capacity(),
                mark,
                "the buffer reallocated on a run of {n} it had already sized for",
            );
        }
    }

    /// The clear happens on entry, not on success — so a run that is rejected
    /// leaves no tail behind for the next one. The walk does not read the
    /// buffer after a `false` (spec 0288 S5 tests `run_ok` first), but the next
    /// token's `packed_varints_terminate` writes over it, and that write must
    /// start from empty.
    #[test]
    fn a_rejected_run_leaves_nothing_for_the_next() {
        let mut scratch = Vec::new();
        assert!(packed_varints_terminate(&run_of(8), &mut scratch));

        let mut truncated = run_of(3);
        truncated.push(0x80); // continuation bit set, no following byte
        assert!(!packed_varints_terminate(&truncated, &mut scratch));

        assert!(packed_varints_terminate(&run_of(2), &mut scratch));
        assert_eq!(scratch.len(), 2);
    }
}
