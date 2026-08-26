// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Main-pane inference-mismatch heat cue (spec 0138) — a per-node cue,
//! shown only when auto-inference finds a strictly better-scoring type
//! for a node's byte range than the node's current effective type.

use super::heat_worker::RangeHeatEntry;
use super::tiered::Tier;
use super::*;

/// Preview width (spec 0152 G6) `heat_cue_for` asks `App::heat_lookup`
/// for — big enough to answer `heat_cue_from_stats`'s gate/level *and*
/// almost always big enough to double as the override pane's first
/// page too (see spec 0152's "plain terms" note).
pub(super) const HEAT_CUE_PREVIEW: usize = 8;

/// It is a *width*: at zero the worker would be asked for no candidates
/// at all, `heat_cue_from_stats` would have nothing to gate on, and every
/// cue would silently disappear. Checked at compile time, where a wrong
/// value cannot get past the build — a runtime `assert!` on a constant is
/// folded away and proves nothing.
const _: () = assert!(HEAT_CUE_PREVIEW > 0);

/// Leading gutter glyph (spec 0138 N1), in the column reserved for it
/// and distinct from the fold marker (`▶`/`▼`) beside it.
///
/// **`■` U+25A0 since spec 0327**, over the `●` U+25CF it was for its
/// first two hundred specs. The cue *is* a color — two hues over twelve
/// brightness levels (G5) — and a circle inks about half its cell, which
/// is where the middle of a twelve-step ramp stops being readable. A
/// fully inked cell gives the level the whole cell to be read in.
///
/// Spec 0322 turned `■` down for `render::ANOMALY_GLYPH` on the opposite
/// argument, and the difference is the scale, not the taste: the fold
/// column carries five *hues* at one brightness, where a large uniform
/// patch does hurt, and this column carries two hues at twelve
/// *brightnesses*, where area helps. Do not "unify" them.
///
/// Two properties a replacement must keep, both already documented at
/// `render::FOLD_GLYPH_OPEN`: no Emoji property, or a terminal supplies
/// its own color and destroys the message; and East Asian Ambiguous
/// width, the class of both the `●` this replaces and the `◆` in the
/// column beside it. The way back down is `●`, a one-line revert.
///
/// **`■` is the largest true square those two constraints leave.**
/// Tried and rejected: `█` U+2588 FULL BLOCK is larger in area but a
/// terminal cell is about 1:2, so a filled cell reads as a tall
/// rectangle, not as a bigger square. The squares that *are* bigger —
/// `◼` U+25FC, `⬛` U+2B1B — all carry the Emoji property and hand the
/// color to an emoji font, which is the one thing this glyph cannot
/// afford. Anything larger needs a second cell, not a second glyph.
///
/// A `&'static str` rather than a `char` (spec 0190 S9): a `char`
/// forces a `String` build at every render of every visible cue — one
/// heap allocation per cue per frame.
pub(super) const HEAT_GLYPH: &str = "■";

/// How much of what the heat machinery knows the main pane draws
/// (spec 0331 S1). Three states rather than the `i` toggle of spec
/// 0138, because "no cue here" used to mean four different things and
/// one of them — *this node's type is the best fit for these bytes* —
/// is a real answer worth reading.
///
/// The mode is read *after* resolution (`heat_cue_at`), so it changes
/// what is formatted and never what is asked for: `All` costs no
/// scoring the other two didn't already pay.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HeatCueMode {
    /// Nothing at all. The opening state: the cue's value is that it
    /// is rare enough to be worth looking at, and a reader who wants
    /// it asks.
    #[default]
    Off,
    /// The findings — mismatch, tie, and the two pendings.
    Findings,
    /// Those, plus a settled node's ` [{score}]` or ` [unmatched]`.
    All,
}

impl HeatCueMode {
    /// `i`. Forward is the direction that shows more, up to the wrap.
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Off => Self::Findings,
            Self::Findings => Self::All,
            Self::All => Self::Off,
        }
    }

    /// `I`, for the reader who overshot.
    pub(crate) fn prev(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::Findings => Self::Off,
            Self::All => Self::Findings,
        }
    }
}

/// A node's computed heat cue (spec 0138 G2-G4, G9-G12): either a
/// `Mismatch` (green — `best` strictly exceeds `current`) or a `Tie`
/// (blue — `current` already equals `best`, but at least one other
/// candidate shares that same top score, so the current typing, while
/// optimal, isn't the *unique* optimum).
#[derive(Clone, Copy)]
pub(super) struct HeatCue {
    /// Ramp fraction in `[0, 1]` (spec 0336 S3), derived from `best`
    /// (`Mismatch`) or from the shared top score (`Tie`) — same source
    /// either way. Spec 0337 replaces the derivation; the field type
    /// is already the right shape for it.
    pub(super) t: f32,
    pub(super) kind: HeatCueKind,
}

#[derive(Clone, Copy)]
pub(super) enum HeatCueKind {
    /// `current: None` — the current type is vetoed (or unresolvable) for
    /// this range, distinct from a genuine score of `0` (spec 0154 G5).
    Mismatch { current: Option<i64>, best: i64 },
    Tie {
        tie_count: usize,
        /// The shared top score itself, shown alongside `tie_count` as
        /// `[{tie_count}@{score}]`: *how many* candidates tie isn't
        /// much use without knowing *at what score* they tie.
        score: i64,
    },
}

/// What a node's line should actually show (spec 0154 G6) — `best`
/// and `current` each arrive independently, so the display has more
/// than two shapes.
#[derive(Clone, Copy)]
pub(super) enum HeatDisplay {
    /// `best` itself isn't known yet — `[?]`. Whether `current` is known
    /// or not is irrelevant here: Mismatch vs. Tie can't be determined
    /// without `best`, so there is no separate `[?/?]` state.
    Unknown,
    /// Genuinely nothing: suppressed by the mode, a non-header line, or
    /// a node that cannot be overridden at all.
    None,
    /// Settled, with no finding to report — spec 0331, drawn only in
    /// `HeatCueMode::All`. `Some(score)`: the current type is the
    /// unique best fit, and `score` is the number both halves agree on.
    /// `None`: every candidate for this range was vetoed, current
    /// included, so there is no number and the cue says so in words.
    Settled { score: Option<i64> },
    /// `best` is known but `current` isn't yet — `[?/{best}]`.
    PendingCurrent { best: i64 },
    /// Both known — a genuine `Mismatch` or `Tie` cue, glyph shown.
    Cue(HeatCue),
}

/// Not scored / not computed yet — the `None` of both halves below.
const UNSCORED: i32 = i32::MIN;
/// Scored, but every candidate was vetoed (`best_score`) or the current
/// type is vetoed / unresolvable (`current`).
const VETOED: i32 = i32::MIN + 1;
/// The most negative *real* score representable. Clamping to this
/// rather than to `i32::MIN` is load-bearing (spec 0220 S2): a score
/// that saturated onto a sentinel would read back as "not scored",
/// `settled()` would answer `false` forever, and `App::prefetch_step`'s
/// skip would stop firing — the node would be re-scored on every worker
/// progress event.
pub(super) const SCORE_FLOOR: i32 = i32::MIN + 2;

/// Per-node heat-cue resolution state (spec 0154 G4), parallel to
/// `App::tree` (`App::heat_states`) — `best` (from the range's window
/// sweep) and `current` (the current type's exact score) each arrive
/// independently, so each is its own pending-vs-available union rather
/// than one all-or-nothing flag.
///
/// Three plain numbers rather than the two nested `Option`s the
/// accessors present (spec 0220 S1): as `Option<RangeHeatStats>` +
/// `Option<Option<i64>>` this was 40 bytes, of which 14 were pure
/// alignment padding, and there is one of these per arena slot — 4.7M
/// of them on googleapis, so 190MB. The sentinel encoding stays inside
/// this file; every call site keeps reading and writing the same
/// `Option`s via `new`/`best`/`current`.
#[derive(Clone, Copy)]
pub(super) struct HeatState {
    best_score: i32,
    current: i32,
    best_count: u32,
}

/// Spec 0220 S1. Pinned the way `decode.rs` pins the node slot, and in
/// the production module for the same reason: a plain `cargo build`
/// must catch it, since a byte added here is 4.7 MB on googleapis.
const _: () = assert!(std::mem::size_of::<HeatState>() == 12);

/// Written by hand, not derived (spec 0220 S3): all-zero would mean
/// "scored, best score 0, current score 0", so every node in a fresh
/// document would report `settled()` and show a stale cue.
impl Default for HeatState {
    fn default() -> Self {
        Self {
            best_score: UNSCORED,
            current: UNSCORED,
            best_count: 0,
        }
    }
}

impl HeatState {
    pub(super) fn new(best: Option<RangeHeatStats>, current: Option<Option<i64>>) -> Self {
        let (best_score, best_count) = match best {
            None => (UNSCORED, 0),
            Some(stats) => (
                match stats.best_score {
                    None => VETOED,
                    Some(score) => clamp_score(score),
                },
                // Saturating, not truncating: a wrapped count could turn
                // a real tie into `best_count == 0` and lose a cue.
                stats.best_count.min(u32::MAX as usize) as u32,
            ),
        };
        Self {
            best_score,
            current: match current {
                None => UNSCORED,
                Some(None) => VETOED,
                Some(Some(score)) => clamp_score(score),
            },
            best_count,
        }
    }

    pub(super) fn best(&self) -> Option<RangeHeatStats> {
        match self.best_score {
            UNSCORED => None,
            VETOED => Some(RangeHeatStats {
                best_score: None,
                best_count: self.best_count as usize,
            }),
            score => Some(RangeHeatStats {
                best_score: Some(score as i64),
                best_count: self.best_count as usize,
            }),
        }
    }

    pub(super) fn current(&self) -> Option<Option<i64>> {
        match self.current {
            UNSCORED => None,
            VETOED => Some(None),
            score => Some(Some(score as i64)),
        }
    }

    /// No more per-frame rechecking needed — a computed property, not a
    /// stored tag (spec 0154 G4): either every candidate is vetoed (in
    /// which case `current` is irrelevant), or both `best` and `current`
    /// are individually known.
    pub(super) fn settled(&self) -> bool {
        self.best_score != UNSCORED && (self.best_score == VETOED || self.current != UNSCORED)
    }
}

/// Clamped at *both* ends, unconditionally (spec 0220 S2). The positive
/// side is not reachable under today's `EntryScore::score` coefficients,
/// but a one-sided clamp would silently become wrong the day one of
/// them changes, and the second compare is free next to the memory the
/// narrowing saves.
fn clamp_score(score: i64) -> i32 {
    score.clamp(SCORE_FLOOR as i64, i32::MAX as i64) as i32
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
/// score axis into 12 levels. Returns a level in `1..=12`.
///
/// Spec 0336 N1: this function is kept unchanged; spec 0337 replaced the
/// `t`-derivation with a log scale without touching the bucketing.
/// Tests only — nothing in the production path calls it.
#[cfg(test)]
pub(super) fn heat_level(best_score: i64) -> u8 {
    const BOUNDARIES: [i64; 11] = [1, 2, 3, 5, 8, 13, 21, 34, 55, 89, 144];
    for (i, &boundary) in BOUNDARIES.iter().enumerate() {
        if best_score <= boundary {
            return (i + 1) as u8;
        }
    }
    12
}

/// Spec 0337 S1: the ramp fraction from a score and a log-space anchor.
///
/// ```text
/// t = clamp(ln(score) / anchor, 0.0, 1.0)   for score >= 1
/// t = 0.0                                    otherwise
/// ```
///
/// Score 1 is the natural floor (`ln(1) = 0`), so a square can only
/// move toward the top as unrelated nodes are scored. The anchor is the
/// 95th-percentile of the document's graded scores (spec 0337 S4),
/// ratcheted upward, so it never brightens existing squares.
pub(super) fn heat_fraction(score: i64, anchor: f32) -> f32 {
    if score < 1 {
        return 0.0;
    }
    ((score as f32).ln() / anchor).clamp(0.0, 1.0)
}

/// Spec 0337 S5: the default anchor — `ln(144) ≈ 4.97` — is the top of
/// the Fibonacci ladder spec 0336 replaced. Before calibration, a
/// small document therefore renders exactly as it did under the old
/// bucketing; large documents converge away as evidence lands. Stored
/// as the constant rather than re-computing `ln(144)` every call.
pub(super) const HEAT_ANCHOR_DEFAULT: f32 = 4.9698_f32; // ln(144)

/// Minimum sample count before the 95th-percentile anchor is consulted
/// (spec 0337 S4). With fewer samples the percentile is untrustworthy
/// and the anchor stays at its default.
const HISTOGRAM_MIN_SAMPLES: u32 = 64;

/// Number of buckets in the log-score histogram. Sixty-four buckets of
/// 0.25 nats each span `[0, 16]`, covering scores from 1 to ≈ 8.9M
/// before the final bucket catches everything beyond (spec 0337 S2).
const HISTOGRAM_BUCKETS: usize = 64;

/// Width of one bucket in nats (spec 0337 S2).
const HISTOGRAM_BUCKET_WIDTH: f32 = 0.25;

/// Spec 0337 S2: histogram of `ln(best_score)` over nodes that draw a
/// graded square — mismatches and ties. The 95th-percentile ratcheted
/// upward is the live `heat_anchor` (spec 0337 S4).
///
/// One writer only: every `heat_states[idx]` write goes through
/// `App::record_heat_state` (spec 0337 S3), which is the only place
/// that increments a bucket.
pub(super) struct HeatHistogram {
    buckets: [u32; HISTOGRAM_BUCKETS],
    total: u32,
}

impl Default for HeatHistogram {
    fn default() -> Self {
        Self {
            buckets: [0u32; HISTOGRAM_BUCKETS],
            total: 0,
        }
    }
}

impl HeatHistogram {
    /// Total number of samples recorded so far. Tests only.
    #[cfg(test)]
    pub(super) fn total(&self) -> u32 {
        self.total
    }

    /// Record one settled graded-square score. Called only by
    /// `record_heat_state` on a transition into a graded `Cue`.
    pub(super) fn record(&mut self, score: i64) {
        if score < 1 {
            return;
        }
        let bucket = ((score as f32).ln() / HISTOGRAM_BUCKET_WIDTH) as usize;
        let bucket = bucket.min(HISTOGRAM_BUCKETS - 1);
        self.buckets[bucket] = self.buckets[bucket].saturating_add(1);
        self.total = self.total.saturating_add(1);
    }

    /// The 95th-percentile of recorded scores, as a log-space value.
    /// Returns `None` when fewer than `HISTOGRAM_MIN_SAMPLES` have been
    /// recorded (spec 0337 S4's guard against a single large score
    /// locking the anchor permanently).
    pub(super) fn p95(&self) -> Option<f32> {
        if self.total < HISTOGRAM_MIN_SAMPLES {
            return None;
        }
        // The 95th-percentile bucket: smallest bucket whose cumulative
        // count reaches 95% of the total.
        let threshold = (self.total as f32 * 0.95).ceil() as u32;
        let mut cum = 0u32;
        for (i, &count) in self.buckets.iter().enumerate() {
            cum += count;
            if cum >= threshold {
                // Return the *top* of bucket i: `(i+1) * width`, which
                // is the natural anchor for scores in that bucket.
                return Some((i + 1) as f32 * HISTOGRAM_BUCKET_WIDTH);
            }
        }
        // All samples in the last bucket — return its top.
        Some(HISTOGRAM_BUCKETS as f32 * HISTOGRAM_BUCKET_WIDTH)
    }
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
    ///
    /// Hands back an owned `String` resolved out of the `FqdnTable`,
    /// not a `FqdnId` (spec 0212 N3): the heat cache cannot take an id,
    /// because two of the three sources below are not FQDNs at all (an
    /// override entry's own name, and `natural_type`'s primitive
    /// keywords like `int32`), and the cache is `Mutex`-shared with the
    /// background worker across a scoring boundary that takes `&str`.
    /// Spec 0337 S3: the one writer for `heat_states[idx]`. Writes the
    /// slot and, on a transition from unsettled into a graded `Cue`,
    /// feeds the histogram and ratchets the anchor.
    ///
    /// "One writer" is how spec 0337 G2's "whole arena" claim is
    /// verifiable: `heat_cue_resolve` and `prefetch_step` — the two
    /// production paths — both pass through here, so the histogram sees
    /// every settled score whether or not the row has been on screen.
    ///
    /// **Not called on `heat_states[d] = HeatState::default()`** in
    /// `override_apply`: those resets clear a slot back to unsettled
    /// and carry no score information. The histogram is forward-only
    /// (spec 0337 N5) and does not un-record.
    pub(super) fn record_heat_state(&mut self, idx: usize, state: HeatState) {
        let was_settled = self.heat_states[idx].settled();
        self.heat_states[idx] = state;
        // Feed the histogram only on the first settlement into a graded
        // square (mismatch or tie). A Settled variant carries no graded
        // square and must not move the anchor (spec 0337 S2).
        if !was_settled {
            if let Some(stats) = state.best() {
                if let Some(best) = stats.best_score {
                    // Is this a mismatch or a tie? Re-derive from the
                    // state rather than calling heat_display (which needs
                    // the anchor we are in the middle of updating).
                    let is_graded = match state.current() {
                        Some(Some(current)) => current < best || stats.best_count > 1,
                        Some(None) => true, // vetoed current → mismatch
                        None => false,      // current not yet known
                    };
                    if is_graded {
                        self.heat_histogram.record(best);
                        if let Some(p95) = self.heat_histogram.p95() {
                            if p95 > self.heat_anchor {
                                self.heat_anchor = p95;
                            }
                        }
                    }
                }
            }
        }
    }

    pub(super) fn current_type_key(&self, idx: usize) -> Option<String> {
        let span = &self.tree[idx].span;
        if span.kind == NodeKind::Message {
            return self.fqdns.get(span.type_fqdn).map(str::to_owned);
        }
        match self.resolve_active_override(idx) {
            Some(inner) => inner,
            None => self.natural_type(idx),
        }
    }

    /// This line's heat cue display, if any (spec 0154 G6) —
    /// `HeatDisplay::None` both when the cue is hidden (`i`) or gated
    /// absent, and when `line_idx` isn't a node's *first* line — see
    /// `heat_cue_at`.
    ///
    /// Spec 0154 G4: a settled node is read directly, no cache lock at
    /// all. An unsettled node goes through `self.heat_lookup` (pushing
    /// a request if either half is still missing), then re-reads
    /// `best`/`current` independently from the shared cache — either
    /// may already be known even when `heat_lookup`'s own all-or-
    /// nothing check reports a miss, which is what makes the
    /// progressive `[?]`/`[?/{best}]` states possible. With no worker
    /// (no scoring graph, or a test fixture), falls back to the
    /// synchronous logic, filling only whichever half is still missing.
    ///
    /// `heat_cues` (`i`/`I`) is consulted *last*, after resolving, so
    /// the background worker keeps fetching and caching cues for every
    /// visited line even while they are hidden and they are already
    /// warm the moment the user asks for them; only the returned value
    /// is suppressed here.
    pub(super) fn heat_cue_for(&mut self, line_idx: usize) -> HeatDisplay {
        match self.line_pos(line_idx) {
            Some(pos) => self.heat_cue_at(pos),
            None => HeatDisplay::None,
        }
    }

    /// `heat_cue_for` for a caller that already holds the line's owner —
    /// spec 0222 S3, which is what a drawn row carries. The draw path
    /// goes through here, so a frame costs no line-to-node lookups at
    /// all.
    pub(super) fn heat_cue_at(&mut self, pos: LinePos) -> HeatDisplay {
        // A node says its cue once, on the line it opens with. That
        // covers a bracketed node's closing brace — a cue is about the
        // node's own type, not about its brace — and a packed record,
        // which is *one* node however many element lines it draws. A
        // packed run repeating the same glyph down its whole length
        // would read as a column of separate findings and is one.
        if pos.line_in_node > 0 {
            return HeatDisplay::None;
        }
        let idx = pos.node;
        if !self.can_override(idx) {
            return HeatDisplay::None;
        }
        let display = self.heat_cue_resolve(idx);
        // Spec 0335 S2: an empty candidate list means two different
        // things, and only one of them is an answer. Applied to this
        // arm alone (0335 N2) — a node inference could never have had a
        // candidate for can still carry a real mismatch or tie.
        if matches!(display, HeatDisplay::Settled { score: None }) && !self.inference_applies(idx) {
            return HeatDisplay::None;
        }
        // Spec 0331 S3: the mode decides what of a resolved answer
        // reaches the screen, and nothing else.
        match self.heat_cues {
            HeatCueMode::Off => HeatDisplay::None,
            HeatCueMode::Findings => match display {
                HeatDisplay::Settled { .. } => HeatDisplay::None,
                other => other,
            },
            HeatCueMode::All => display,
        }
    }

    /// Whether inference could ever have produced a candidate for
    /// `idx`'s bytes (spec 0335 S1) — the gate that tells *nothing fits*
    /// apart from *there was no question*.
    ///
    /// **This is not `can_override`, and the two must not be unified.**
    /// `can_override` gates the override pane, which has to open on a
    /// varint so the reader can retype it to `sfixed32`; spec 0135 G3
    /// widened it to every wire type a value can carry, on purpose. This
    /// one asks whether a *message* search over these bytes was ever
    /// meaningful, and answers no far more often.
    ///
    /// `bytes` is admitted where every other scalar is refused: a
    /// `bytes` field is the schema declining to say what is in there,
    /// which is exactly what inference is for, and it is the usual home
    /// of an embedded message. A declared `string` is refused — it
    /// asserts UTF-8 text, and a message that is also valid UTF-8 is a
    /// coincidence rather than a finding.
    pub(super) fn inference_applies(&self, idx: usize) -> bool {
        use prost_reflect::Kind;
        use prototext_core::helpers::{WT_I32, WT_I64, WT_VARINT};
        // These bytes cannot be a nested message under any typing, so
        // the candidate list was empty before it was built. A packed
        // run whose slot still carries its element's wire type lands
        // here too, which is the answer we want for it either way.
        if matches!(
            u32::from(self.tree[idx].span.wire_type()),
            WT_VARINT | WT_I32 | WT_I64
        ) {
            return false;
        }
        match self.parent_field(idx).map(|field| field.kind()) {
            // No declaration to read — the document root, an unresolved
            // parent, or a field nothing declares. Nothing has said what
            // these bytes are, so the question stands.
            None => true,
            Some(Kind::Message(_) | Kind::Bytes) => true,
            Some(_) => false,
        }
    }

    /// Re-asks for the cursor row's cue, so that it sits at the head of
    /// the `Visible` band (spec 0262 S7).
    ///
    /// The cursor row used to be asked for at `Tier::User`, which made a
    /// held arrow key one `User` request per keystroke — and `User` now
    /// means "stop the whole pool for this". It is one of forty-odd rows
    /// on screen and it is scored the same way they are; what it is owed
    /// is to be *first* among them, and a re-ask is exactly that: a
    /// same-tier merging push moves an entry back to the head of its
    /// band (spec 0208 S4c).
    ///
    /// Called once per frame, after the window's own row-by-row pass, so
    /// that it is the last `Visible` push of the frame and therefore the
    /// front of the band. A settled cursor row pushes nothing at all —
    /// `heat_cue_resolve` returns before the lookup.
    ///
    /// `self.cursor` is a node index, not a line index, and it names
    /// the node whose bracket pair the caret belongs to whether the
    /// caret rests on the header line or the footer (spec 0142) — so
    /// this needs no line-map lookup.
    pub(super) fn refresh_cursor_heat_cue(&mut self) {
        // An empty document has no node for the cursor to be resting on;
        // the row-by-row pass never meets that case because it has no
        // rows to walk.
        if self.cursor < self.tree.len() && self.can_override(self.cursor) {
            self.heat_cue_resolve(self.cursor);
        }
    }

    /// The byte range a node's heat cue is scored over: the node's
    /// payload, tag and length prefix stripped.
    ///
    /// An element of a packed run scores the **whole record's** payload
    /// (spec 0184 S6), not the element's own few bytes: a packed run is
    /// one wire record and one unit of action — `t` on any element
    /// overrides the record (`splice_override`'s `siblings[0]` merge)
    /// and `current_type_key` resolves the record's override, since
    /// every element shares the record's positional path (S2). Scoring
    /// the element's own bytes would compare a record-level type key
    /// against element-level content.
    ///
    /// That needs no special case here. Since spec 0216 a packed run is
    /// a single slot whose `raw_range` spans the whole record, taken
    /// from the arena rather than from the render's spans
    /// (`overlay_spans`), so the ordinary stripping already yields the
    /// record's payload — and all N element lines already share the one
    /// `heat_caches` entry keyed on its start.
    ///
    /// Deriving the end from `raw_range` rather than from the record's
    /// own length varint is also what keeps this in bounds: the arena's
    /// end is a real offset into the blob, whereas `start + length` on a
    /// crafted length overflows and yields a reversed range that panics
    /// where it is sliced.
    ///
    /// **Every heat cache key goes through here**, and that is not
    /// tidiness. `heat_cue_resolve` *writes* the cache under this range
    /// and `prefetch_step_inner` pushes requests for the same node under
    /// it; five call sites used to open-code the body instead, so the
    /// two agreed only for as long as nobody edited one copy. A
    /// divergence has no failure signal — one would read a key the other
    /// never wrote, and the node would simply never settle.
    pub(super) fn heat_scored_range(&self, idx: usize) -> std::ops::Range<usize> {
        extract::message_payload_range(&self.blob, &self.tree[idx].span.raw_range)
    }

    /// What the caches currently know about the range starting at
    /// `start` under `current_key` — the whole of what reading one
    /// node's heat is, in one place, so `heat_cue_resolve` and
    /// `prefetch_step_inner` cannot come to read it two different ways.
    ///
    /// `tier` is whatever the caller pushed its request at: `peek` is a
    /// *promoting* read (spec 0164 G9), so reading a result back at a
    /// lower tier than it was asked for would undo the promotion the
    /// request earned and hand it to eviction ahead of a result nobody
    /// is looking at.
    ///
    /// `peek_with`, not `peek`: this runs per unsettled node per frame,
    /// and the two fields wanted sit alongside a whole ranked candidate
    /// list that `peek` would copy to reach them — with the cache's
    /// `Mutex` held against the worker.
    pub(super) fn read_heat_state(
        &self,
        start: usize,
        current_key: Option<&str>,
        tier: Tier,
    ) -> HeatState {
        let mut caches = self.heat_caches.lock().unwrap_or_else(|e| e.into_inner());
        let best = caches
            .by_range
            .peek_with(&start, tier, RangeHeatEntry::stats);
        let current = match current_key {
            None => Some(None),
            Some(key) => caches.peek_current(start, key, tier),
        };
        HeatState::new(best, current)
    }

    /// The core of `heat_cue_for`/`heat_cue_at` (spec 0154 G4) —
    /// everything past the line-index-to-node/eligibility gating, keyed
    /// directly on node index.
    ///
    /// This is also what notices a scoring answer (spec 0224 S2): an
    /// unsettled node re-reads the shared cache and rewrites its own
    /// `heat_states` entry on every frame in which its row is drawn, and
    /// spec 0192's `heat_dirty` owes such a frame within
    /// `HEAT_REPAINT_INTERVAL` of any worker completion. That is what
    /// makes the progressive `[?]` -> `[?/{best}]` -> cue sequence
    /// visible with no user action in between.
    fn heat_cue_resolve(&mut self, idx: usize) -> HeatDisplay {
        if self.heat_states[idx].settled() {
            return heat_display(self.heat_states[idx], self.heat_anchor);
        }
        let range = self.heat_scored_range(idx);
        let start = range.start;
        let current_key = self.current_type_key(idx);

        // Side effect only (pushes a request if either half is still
        // missing, merged into the queue per G3); the AND-gated return
        // value itself is discarded — `best`/`current` are re-read
        // independently just below, since either may already be known
        // even when this reports a miss.
        //
        // Spec 0262 S7: every main-pane row is `Visible`, the cursor's
        // included. Its precedence comes from `refresh_cursor_heat_cue`
        // asking again at the end of the frame, not from a tier of its
        // own — `Tier::User` now stops the whole worker pool, and a held
        // arrow key must not do that once per keystroke.
        let tier = Tier::Visible;
        self.heat_lookup(&range, current_key.as_deref(), 0, HEAT_CUE_PREVIEW, tier);

        let state = self.read_heat_state(start, current_key.as_deref(), tier);

        if state.settled() || self.heat_worker.is_some() {
            self.record_heat_state(idx, state);
            return heat_display(state, self.heat_anchor);
        }

        // No worker and still unsettled after an independent cache read
        // — either scoring is genuinely needed (the synchronous logic
        // below) or there's no scoring graph at all, in which case
        // nothing is ever going to resolve this node further: show
        // nothing rather than a permanent `[?]`. `heat_states[idx]` is
        // left untouched (still unsettled) so a cache write from
        // elsewhere is still picked up on a later call.
        // Cloned rather than borrowed (spec 0180 S2): the `Arc` clone
        // keeps `self` free to be borrowed mutably below.
        let Some(graph) = self.ctx.graph.clone() else {
            return HeatDisplay::None;
        };
        let graph = graph.graph();
        let range_bytes = &self.blob[range.clone()];
        let cut = override_pane::ends_where_the_bytes_end(&range, self.blob.len());
        let state = if let Some(best) = state.best() {
            // Window already covered — only the current type's score is
            // missing (spec 0154 G3's cheap path, mirrored here).
            let key = current_key
                .as_deref()
                .expect("unsettled with best known implies current is still pending");
            let score = override_pane::inferred_score(range_bytes, key, graph, cut);
            let mut caches = self.heat_caches.lock().unwrap_or_else(|e| e.into_inner());
            caches
                .current_score
                .upsert((start, key.to_string()), score, Tier::Visible);
            HeatState::new(Some(best), Some(score))
        } else {
            // The whole budget, not the worker's share: this arm is only
            // reached when there is no worker, so nothing else in the
            // session is sweeping (spec 0217 S6). `None` for the same
            // reason it is synchronous: this runs on the thread that
            // would have to raise the flag.
            let candidates =
                override_pane::inferred_candidates(range_bytes, graph, self.sweep_jobs, None, cut);
            let stats = derive_stats(&candidates);
            let current_entry = current_key
                .as_deref()
                .and_then(|key| score_of(&candidates, key));

            let mut caches = self.heat_caches.lock().unwrap_or_else(|e| e.into_inner());
            // At least `HEAT_CUE_PREVIEW` (what `heat_lookup` just
            // checked coverage against), and at least
            // `override_list_height` too (spec 0151 G6's
            // cross-population cap) — never narrower than either.
            let cap = self.override_list_height.max(1).max(HEAT_CUE_PREVIEW);
            let top_n: Vec<_> = candidates.iter().take(cap).cloned().collect();
            caches
                .by_range
                .upsert(start, RangeHeatEntry::new(stats, top_n), Tier::Visible);
            if let Some(key) = current_key.as_ref() {
                caches
                    .current_score
                    .upsert((start, key.clone()), current_entry, Tier::Visible);
            }
            // Spec 0250 S8 reserves this cache for the override pane's
            // whole-list request, and this arm is a *cue*. It writes
            // anyway because in the no-worker configuration it is the
            // only thing that ever will: with no worker there is
            // nothing for `heat_lookup` to push a request to, so the
            // pane's `[0, usize::MAX)` lookup can only ever be answered
            // out of what a cue already computed here — `by_range`'s
            // `top_n` is a screenful and can never cover it.
            caches.complete.insert(range.clone(), candidates);
            HeatState::new(Some(stats), Some(current_entry))
        };

        self.record_heat_state(idx, state);
        heat_display(state, self.heat_anchor)
    }
}

/// Pure gate/level computation over a `HeatState` (spec 0154 G6) —
/// split out from `heat_cue_resolve` so it's directly unit-testable
/// without a real scoring graph. The full display table: `[?]`
/// whenever `best` isn't known yet (no separate `[?/?]` state —
/// Mismatch vs. Tie can't be determined without `best` either way);
/// `Settled` when every candidate is vetoed or `current` is the unique
/// optimum; `[?/{best}]` while only `current` remains unknown;
/// otherwise a genuine `Mismatch`/`Tie` cue. `Option`-aware throughout:
/// a vetoed `current` is never conflated with a genuine `0` score.
///
/// Never `HeatDisplay::None`: a scoreable node in some state always has
/// *something* to say, and which of it reaches the screen is the mode's
/// decision, taken in `heat_cue_at`.
///
/// `anchor` is spec 0337's log-space scale top — passed in so this
/// function stays a pure computation and stays testable without a full
/// `App`.
pub(super) fn heat_display(state: HeatState, anchor: f32) -> HeatDisplay {
    let Some(stats) = state.best() else {
        return HeatDisplay::Unknown;
    };
    let Some(best) = stats.best_score else {
        // Every candidate vetoed, current included — settled, and with
        // no number to print (spec 0331).
        return HeatDisplay::Settled { score: None };
    };
    let Some(current) = state.current() else {
        return HeatDisplay::PendingCurrent { best };
    };
    match current {
        None => HeatDisplay::Cue(HeatCue {
            t: heat_fraction(best, anchor),
            kind: HeatCueKind::Mismatch {
                current: None,
                best,
            },
        }),
        Some(current) if current < best => HeatDisplay::Cue(HeatCue {
            t: heat_fraction(best, anchor),
            kind: HeatCueKind::Mismatch {
                current: Some(current),
                best,
            },
        }),
        Some(current) if current == best && stats.best_count > 1 => HeatDisplay::Cue(HeatCue {
            t: heat_fraction(best, anchor),
            kind: HeatCueKind::Tie {
                tie_count: stats.best_count,
                score: best,
            },
        }),
        // The unique optimum: the current type is the best fit for
        // these bytes and nothing else ties it. Settled, with a number
        // both halves agree on (spec 0331).
        Some(current) => HeatDisplay::Settled {
            score: Some(current),
        },
    }
}
