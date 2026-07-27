// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Main-pane inference-mismatch heat cue (spec 0138, item 12 of
//! 2026-07-17 feedback) — a per-node cue, shown only when auto-inference
//! finds a strictly better-scoring type for a node's byte range than the
//! node's current effective type.

use super::heat_worker::RangeHeatEntry;
use super::tiered::Tier;
use super::*;

/// Preview width (spec 0152 G6) `heat_cue_for` asks `App::heat_lookup`
/// for — big enough to answer `heat_cue_from_stats`'s gate/level *and*
/// almost always big enough to double as the override pane's first
/// page too (see spec 0152's "plain terms" note).
pub(super) const HEAT_CUE_PREVIEW: usize = 8;

/// Leading gutter glyph (spec 0138 N1) — a filled circle reads clearly at
/// a single terminal cell width in both light and dark themes, and is
/// distinct from the fold marker (`▸`/`▾`) and every other glyph this
/// crate already uses.
pub(super) const HEAT_GLYPH: char = '●';

/// A node's computed heat cue (spec 0138 G2-G4, G9-G12): either a
/// `Mismatch` (red — the original gate, `best` strictly exceeds
/// `current`) or a `Tie` (blue — `current` already equals `best`, but at
/// least one other candidate shares that same top score, so the current
/// typing, while optimal, isn't the *unique* optimum).
#[derive(Clone, Copy)]
pub(super) struct HeatCue {
    /// Brightness level, 1..=12 (spec 0138 G5), bucketed from `best`
    /// (`Mismatch`) or from the shared top score (`Tie`) — same
    /// bucketing function either way.
    pub(super) level: u8,
    pub(super) kind: HeatCueKind,
}

#[derive(Clone, Copy)]
pub(super) enum HeatCueKind {
    /// `current: None` — the current type is vetoed (or unresolvable) for
    /// this range, distinct from a genuine score of `0` (spec 0154 G5).
    Mismatch { current: Option<i64>, best: i64 },
    Tie {
        tie_count: usize,
        /// The shared top score itself (feedback, 2026-07-20: shown
        /// alongside `tie_count` as `[{tie_count}@{score}]` — knowing
        /// *how many* candidates tie isn't as useful without knowing
        /// *at what score* they tie).
        score: i64,
    },
}

/// What a node's line should actually show (spec 0154 G6) — the
/// progressive counterpart to the old two-state `Pending`/`Resolved`
/// split: `best` and `current` each arrive independently, so the
/// display has more than two shapes.
#[derive(Clone, Copy)]
pub(super) enum HeatDisplay {
    /// `best` itself isn't known yet — `[?]`. Whether `current` is known
    /// or not is irrelevant here: Mismatch vs. Tie can't be determined
    /// without `best`, so there is no separate `[?/?]` state.
    Unknown,
    /// Nothing to show, and settled: either every candidate is vetoed
    /// (`best_score: None`), or `current` is the unique optimum.
    None,
    /// `best` is known but `current` isn't yet — `[?/{best}]`.
    PendingCurrent { best: i64 },
    /// Both known — a genuine `Mismatch` or `Tie` cue, glyph shown.
    Cue(HeatCue),
}

/// Per-node heat-cue resolution state (spec 0152 G6, restructured by
/// spec 0154 G4), parallel to `App::tree` (`App::heat_states`) — `best`
/// (from the range's window sweep) and `current` (the current type's
/// exact score) each arrive independently, so each is its own
/// pending-vs-available union rather than one all-or-nothing flag.
#[derive(Clone, Copy, Default)]
pub(super) struct HeatState {
    /// `None` — the range hasn't been scored at all yet. `Some(stats)`
    /// — scored; `stats.best_score: None` means every candidate was
    /// vetoed.
    pub(super) best: Option<RangeHeatStats>,
    /// `None` — the current type's score hasn't been computed yet.
    /// `Some(None)` — computed, vetoed (or no resolvable current type
    /// at all). `Some(Some(c))` — computed, score `c`.
    pub(super) current: Option<Option<i64>>,
}

impl HeatState {
    /// No more per-frame rechecking needed — a computed property, not a
    /// stored tag (spec 0154 G4): either every candidate is vetoed (in
    /// which case `current` is irrelevant), or both `best` and `current`
    /// are individually known.
    pub(super) fn settled(&self) -> bool {
        match self.best {
            None => false,
            Some(stats) => stats.best_score.is_none() || self.current.is_some(),
        }
    }
}

/// Small, fixed-size summary of a range's inference-candidate list
/// (spec 0151 G1) — everything `heat_cue_from_stats` actually needs,
/// derived once and cached in place of the full `Vec<(String, i64)>`
/// `inferred_candidates` returns.
#[derive(Clone, Copy)]
pub(super) struct RangeHeatStats {
    /// `None` when every candidate for this range is vetoed
    /// (equivalently, `-inf`) — a real, cacheable value, not an
    /// absent-entry sentinel (see spec 0151 Background).
    pub(super) best_score: Option<i64>,
    /// Cardinality of the set of candidates sharing `best_score`. A
    /// unique winner has `best_count == 1`, never `0`. Meaningless
    /// (left `0`) when `best_score` is `None`.
    pub(super) best_count: usize,
}

/// Entry-count cap for `heat_range_cache`/`heat_current_score_cache`
/// (spec 0151 G4) — generous headroom for any realistically browsed
/// document tree; both value types are small fixed-size scalars, so
/// even a fully populated cache costs well under 1MB.
pub(super) const HEAT_CACHE_MAX_ENTRIES: usize = 8192;

/// Derives `RangeHeatStats` from a full candidate list (spec 0151 G1) —
/// always returns a value (no `Option`/`?`): an empty `candidates` list
/// is itself informative (`best_score: None`) and must still be cached,
/// not treated as "nothing to insert" (see Background).
pub(super) fn derive_stats(candidates: &[(String, i64)]) -> RangeHeatStats {
    let Some(&(_, best)) = candidates.first() else {
        return RangeHeatStats {
            best_score: None,
            best_count: 0,
        };
    };
    let best_count = candidates.iter().filter(|(_, s)| *s == best).count();
    RangeHeatStats {
        best_score: Some(best),
        best_count,
    }
}

/// Looks up one candidate's score by FQDN — `None` when `key` isn't in
/// `candidates` at all (vetoed for this range).
pub(super) fn score_of(candidates: &[(String, i64)], key: &str) -> Option<i64> {
    candidates
        .iter()
        .find(|(fqdn, _)| fqdn == key)
        .map(|(_, score)| *score)
}

/// Spec 0138 G5's Fibonacci brightness bucketing: `[1, 2, 3, 5, 8, 13,
/// 21, 34, 55, 89, 144]` as 11 ascending boundaries, partitioning the
/// score axis into 12 levels.
pub(super) fn heat_level(best_score: i64) -> u8 {
    const BOUNDARIES: [i64; 11] = [1, 2, 3, 5, 8, 13, 21, 34, 55, 89, 144];
    for (i, &boundary) in BOUNDARIES.iter().enumerate() {
        if best_score <= boundary {
            return (i + 1) as u8;
        }
    }
    12
}

impl App {
    /// A node's currently effective type, as a lookup key into its
    /// heat-cache candidate list (spec 0138 G3). Message/group nodes read
    /// `span.type_fqdn` directly — already kept in sync with any active
    /// override by `resettle_node` on every render pass (see
    /// `status_type_label`'s own doc comment), so no separate override
    /// lookup is needed. Scalar nodes consult `resolve_active_override`
    /// first, falling back to the schema-declared type (`natural_type`)
    /// when no override is active — mirroring `status_type_label`'s own
    /// fallback chain. A primitive-keyword or otherwise-unranked result
    /// simply won't be found in the candidate list — `heat_cue_from_
    /// stats` treats that as `current_entry: None` (spec 0151 G5), not
    /// a coincidental `0`.
    ///
    /// `pub(super)`: also used by `App::prefetch_step` (spec 0164 G7).
    pub(super) fn current_type_key(&self, idx: usize) -> Option<String> {
        let span = &self.tree[idx].span;
        if span.is_message {
            return span.type_fqdn.clone();
        }
        match self.resolve_active_override(idx) {
            Some(inner) => inner,
            None => self.natural_type(idx),
        }
    }

    /// This line's heat cue display, if any (spec 0138, restructured by
    /// spec 0154 G6) — `HeatDisplay::None` both when the cue is hidden
    /// (`i`) or gated absent, and when `line_idx` isn't a node's own
    /// header line (`line_to_node`, mirroring `line_has_active_
    /// override`'s restriction to header/footer lines — only the header
    /// side applies here, since a cue is about the node's own type, not
    /// its closing brace).
    ///
    /// Spec 0154 G4: a settled node is read directly, no cache lock at
    /// all. An unsettled node goes through `self.heat_lookup` (pushing
    /// a request if either half is still missing), then re-reads
    /// `best`/`current` independently from the shared cache — either
    /// may already be known even when `heat_lookup`'s own all-or-
    /// nothing check reports a miss, which is what makes the
    /// progressive `[?]`/`[?/{best}]` states possible. With no worker
    /// (no scoring graph, or a test fixture), falls back to spec
    /// 0151/0154's synchronous logic, filling only whichever half is
    /// still missing.
    ///
    /// `heat_cues_hidden` (`i`) is checked *last*, after resolving —
    /// the background worker keeps fetching and caching cues for every
    /// visited line even while hidden (2026-07-19 feedback), so cues
    /// are already warm the moment the user un-hides them; only the
    /// returned value (what's actually shown) is suppressed here.
    pub(super) fn heat_cue_for(&mut self, line_idx: usize) -> HeatDisplay {
        let Some(idx) = self.node_at_header_line(line_idx) else {
            return HeatDisplay::None;
        };
        if !self.can_override(idx) {
            return HeatDisplay::None;
        }
        let display = self.heat_cue_resolve(idx);
        if self.heat_cues_hidden {
            return HeatDisplay::None;
        }
        display
    }

    /// The byte range a node's heat cue is scored over.
    ///
    /// For an ordinary node that is its payload, tag and length prefix
    /// stripped. For an element of a packed run it is the **whole
    /// record's** payload (spec 0184 S6), not the element's own few
    /// bytes: a packed run is one wire record and one unit of action —
    /// `t` on any element overrides the record (`splice_override`'s
    /// `siblings[0]` merge) and `current_type_key` resolves the
    /// record's override, since every element shares the record's
    /// positional path (S2). Scoring the element's own bytes would
    /// compare a record-level type key against element-level content.
    ///
    /// Computed in O(1) from `packed_record_start` — the record's tag
    /// offset — rather than by walking the run, so this stays cheap on
    /// a run with many elements. It also means all N element lines
    /// share a single `heat_caches` entry, keyed on the payload start.
    pub(super) fn heat_scored_range(&self, idx: usize) -> std::ops::Range<usize> {
        let node = &self.tree[idx].span;
        let Some(record_start) = node.packed_record_start else {
            return extract::message_payload_range(&self.blob, &node.raw_range, None);
        };
        // The element's own `raw_range` ends where the element does, so
        // the record's end has to come from the record's own length
        // varint (same reconstruction as `packed_record_extent`).
        // Clamped, because a malformed length must not produce a range
        // that later slices out of bounds.
        let tag = prototext_core::helpers::parse_wiretag(&self.blob, record_start);
        let len = prototext_core::helpers::parse_varint(&self.blob, tag.next_pos);
        let start = len.next_pos.min(self.blob.len());
        let end = (start + len.varint.unwrap_or(0) as usize).min(self.blob.len());
        start..end
    }

    /// The shared core of `heat_cue_for` and `recheck_pending_heat_
    /// states` (spec 0152 G6/G8, spec 0154 G4) — everything past the
    /// line-index-to-node/eligibility gating, keyed directly on node
    /// index.
    fn heat_cue_resolve(&mut self, idx: usize) -> HeatDisplay {
        if self.heat_states[idx].settled() {
            return heat_display(self.heat_states[idx]);
        }
        let range = self.heat_scored_range(idx);
        let start = range.start;
        let current_key = self.current_type_key(idx);

        // Side effect only (pushes a request if either half is still
        // missing, merged into the queue per G3); the AND-gated return
        // value itself is discarded — `best`/`current` are re-read
        // independently just below, since either may already be known
        // even when this reports a miss. `Tier::Visible` (spec 0164
        // G1): this runs every frame for every visible node just to
        // re-check its own pending status — it must not repeatedly
        // jump ahead of a request a user action (`t`, arrow keys in
        // the override pane) just queued.
        self.heat_lookup(
            &range,
            current_key.as_deref(),
            0,
            HEAT_CUE_PREVIEW,
            Tier::Visible,
        );

        let state = {
            let mut caches = self.heat_caches.lock().unwrap_or_else(|e| e.into_inner());
            let best = caches
                .by_range
                .peek(&start, Tier::Visible)
                .map(|e| RangeHeatStats {
                    best_score: e.best_score,
                    best_count: e.best_count,
                });
            let current = match current_key.as_deref() {
                None => Some(None),
                Some(key) => caches
                    .current_score
                    .peek(&(start, key.to_string()), Tier::Visible),
            };
            HeatState { best, current }
        };

        if state.settled() || self.heat_worker.is_some() {
            self.heat_states[idx] = state;
            if state.settled() {
                self.pending_heat_recheck.remove(&idx);
            } else {
                self.pending_heat_recheck.insert(idx);
            }
            return heat_display(state);
        }

        // No worker and still unsettled after an independent cache read
        // — either scoring is genuinely needed (spec 0151/0154's
        // synchronous logic, below) or there's no scoring graph at all,
        // in which case nothing is ever going to resolve this node
        // further: show nothing rather than a permanent `[?]` (mirrors
        // the old `Pending`-forever behavior, which never showed
        // `[pending]` in that case either). `heat_states[idx]` is left
        // untouched (still unsettled) so a cache write from elsewhere
        // is still picked up on a later call.
        // Cloned rather than borrowed (spec 0180 S2): this used to be a
        // `&'static` copy, which detached from `self` for free; an `Arc`
        // clone is the honest equivalent and keeps `self` free to be
        // borrowed mutably below.
        let Some(graph) = self.ctx.graph.clone() else {
            return HeatDisplay::None;
        };
        let graph = graph.graph();
        let range_bytes = &self.blob[range.clone()];
        let state = if state.best.is_some() {
            // Window already covered — only the current type's score is
            // missing (spec 0154 G3's cheap path, mirrored here).
            let key = current_key
                .as_deref()
                .expect("unsettled with best known implies current is still pending");
            let score = override_pane::inferred_score(range_bytes, key, graph);
            let mut caches = self.heat_caches.lock().unwrap_or_else(|e| e.into_inner());
            caches
                .current_score
                .upsert((start, key.to_string()), score, Tier::Visible);
            HeatState {
                best: state.best,
                current: Some(score),
            }
        } else {
            let candidates = override_pane::inferred_candidates(range_bytes, graph);
            let stats = derive_stats(&candidates);
            let current_entry = current_key
                .as_deref()
                .and_then(|key| score_of(&candidates, key));

            let mut caches = self.heat_caches.lock().unwrap_or_else(|e| e.into_inner());
            // At least `HEAT_CUE_PREVIEW` (what `heat_lookup` just
            // checked coverage against), and at least
            // `override_list_height` too (spec 0151 G6's original
            // cross-population cap) — never narrower than either.
            let cap = self.override_list_height.max(1).max(HEAT_CUE_PREVIEW);
            let top_n: Vec<_> = candidates.iter().take(cap).cloned().collect();
            caches.by_range.upsert(
                start,
                RangeHeatEntry {
                    best_score: stats.best_score,
                    best_count: stats.best_count,
                    top_n,
                },
                Tier::Visible,
            );
            if let Some(key) = current_key.as_ref() {
                caches
                    .current_score
                    .upsert((start, key.clone()), current_entry, Tier::Visible);
            }
            caches.complete = Some((range.clone(), candidates));
            HeatState {
                best: Some(stats),
                current: Some(current_entry),
            }
        };

        self.heat_states[idx] = state;
        self.pending_heat_recheck.remove(&idx);
        heat_display(state)
    }

    /// Re-checks the shared cache for every node not yet `settled`
    /// (spec 0152 G6/G8, spec 0154 G4) — called whenever the worker
    /// thread reports progress. Reads `HeatCaches` directly rather than
    /// going through `self.heat_lookup` (unlike `heat_cue_resolve`) so
    /// a still-missing entry is never re-pushed onto the request queue
    /// — pointless here: the queue already merges by range (G3), so any
    /// earlier in-flight request for this node is either already
    /// covering it or will itself report progress again once popped.
    /// Updates every node's state to whatever is now known, even if
    /// still only partially settled — that's what makes the
    /// progressive `[?]` -> `[?/{best}]` -> cue sequence visible across
    /// successive worker-progress wakeups.
    pub(super) fn recheck_pending_heat_states(&mut self) {
        let candidates: Vec<usize> = self.pending_heat_recheck.iter().copied().collect();
        for idx in candidates {
            // Out of range after a live-preview truncation (spec 0161)
            // shrank `tree`/`heat_states` since this index was queued.
            if idx >= self.heat_states.len() {
                self.pending_heat_recheck.remove(&idx);
                continue;
            }
            if self.heat_states[idx].settled() {
                self.pending_heat_recheck.remove(&idx);
                continue;
            }
            if !self.can_override(idx) {
                self.pending_heat_recheck.remove(&idx);
                continue;
            }
            let range = {
                let node = &self.tree[idx].span;
                extract::message_payload_range(
                    &self.blob,
                    &node.raw_range,
                    node.packed_record_start,
                )
            };
            let start = range.start;
            let current_key = self.current_type_key(idx);

            let mut caches = self.heat_caches.lock().unwrap_or_else(|e| e.into_inner());
            let best = caches
                .by_range
                .peek(&start, Tier::Visible)
                .map(|e| RangeHeatStats {
                    best_score: e.best_score,
                    best_count: e.best_count,
                });
            let current = match current_key.as_deref() {
                None => Some(None),
                Some(key) => caches
                    .current_score
                    .peek(&(start, key.to_string()), Tier::Visible),
            };
            drop(caches);

            let state = HeatState { best, current };
            self.heat_states[idx] = state;
            if state.settled() {
                self.pending_heat_recheck.remove(&idx);
            }
        }
    }
}

/// Pure gate/level computation over a `HeatState` (spec 0151 G5, spec
/// 0138 G2-G4/G9, restructured by spec 0154 G6) — split out from
/// `heat_cue_resolve` so it's directly unit-testable without a real
/// scoring graph. Implements the full progressive display table: `[?]`
/// whenever `best` isn't known yet (no separate `[?/?]` state —
/// Mismatch vs. Tie can't be determined without `best` either way);
/// nothing shown when every candidate is vetoed or `current` is the
/// unique optimum; `[?/{best}]` while only `current` remains unknown;
/// otherwise a genuine `Mismatch`/`Tie` cue. `Option`-aware throughout:
/// a vetoed `current` is never conflated with a genuine `0` score.
pub(super) fn heat_display(state: HeatState) -> HeatDisplay {
    let Some(stats) = state.best else {
        return HeatDisplay::Unknown;
    };
    let Some(best) = stats.best_score else {
        return HeatDisplay::None; // every candidate vetoed — no cue possible
    };
    let Some(current) = state.current else {
        return HeatDisplay::PendingCurrent { best };
    };
    match current {
        None => HeatDisplay::Cue(HeatCue {
            level: heat_level(best),
            kind: HeatCueKind::Mismatch {
                current: None,
                best,
            },
        }),
        Some(current) if current < best => HeatDisplay::Cue(HeatCue {
            level: heat_level(best),
            kind: HeatCueKind::Mismatch {
                current: Some(current),
                best,
            },
        }),
        Some(current) if current == best && stats.best_count > 1 => HeatDisplay::Cue(HeatCue {
            level: heat_level(best),
            kind: HeatCueKind::Tie {
                tie_count: stats.best_count,
                score: best,
            },
        }),
        _ => HeatDisplay::None, // unique optimum
    }
}
