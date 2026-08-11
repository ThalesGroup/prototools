// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0235: a search that answers while it is still being typed.
//!
//! A search here is a *value* rather than a call. `SearchSweep` holds
//! where a walk has got to and what it has found; `App::search_sweep_
//! step` moves it on by a bounded slice and returns the same
//! `Progressed`/`Idle` verdict `prefetch_step` does, because it plugs
//! into the same socket in `run_loop`'s receive loop. That is the whole
//! of how the keyboard stays free during a two-second scan: the sweep
//! never holds the thread for longer than a slice, and abandoning one
//! is a struct assignment rather than a cancellation protocol.
//!
//! The same three panes' searches all run through here, and so do the
//! non-incremental entry points (`n`, `N`, and a `/` confirmed with an
//! empty pattern): those build a sweep, run it to the end in one call,
//! and apply the result. One walk, one matching rule, three callers.

use std::borrow::Cow;
use std::collections::VecDeque;

use super::navigation::PathScratch;
use super::search_cursor::{Segment, SegmentScan};
use super::selection::SelectionSpan;
use super::*;

/// Spec 0235 S4: how many candidates one `search_sweep_step` visits.
///
/// A responsiveness knob and nothing else — it changes no result, only
/// how long a keystroke can wait behind a slice. A slice of 1 would
/// instead spend around a tenth of the whole sweep in `try_recv`.
///
/// Measured on googleapis.desc (5 281 124 lines, spec 0235's Measured
/// outcome): 5 282 slices, worst slice 222–797 µs and typically
/// 250–400 µs, which is the whole of the added key latency. Converging
/// the sweep a slice at a time costs 647–961 ms against the 644–935 ms
/// of the same walk run in one call, so the slicing itself is below the
/// noise.
pub(super) const SEARCH_SWEEP_SLICE: usize = 1000;

/// Spec 0274 S13: how far above the window `multi_row_occurrences`
/// looks for a match that reaches down into it.
///
/// A match may begin anywhere at all, but a highlight pass that scanned
/// back to wherever that is would stop being bounded by the pane. This
/// bounds it: a cross-row match starting up to this many rows above the
/// top of the window still tints the part of itself that is on screen,
/// and one starting further up does not. Fifty rows is about a
/// screenful, so the answer settles as soon as the row the match starts
/// on is within a page of being visible.
const SEARCH_HIGHLIGHT_LEAD: usize = 50;

/// What one `search_sweep_step` did — `Progressed` and `Idle` are
/// `PrefetchStep`'s, since `run_loop` treats the two the same way:
/// `Progressed` means do not sleep yet, `Idle` means there is nothing
/// left to do here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SweepStep {
    Progressed,
    /// Spec 0274 S10: a segment scan is in flight on a worker thread.
    /// The other three idle-arm jobs are skipped — they rewrite the
    /// document the scan is reading — and the loop sleeps anyway,
    /// because a scan can run for seconds and the drawing core is
    /// reserved (0265) precisely so that it is free.
    Waiting,
    Idle,
}

/// Which pane a search is running against. Fixed when the prompt opens
/// (or when `n`/`N` fires) rather than re-read per slice, so that a
/// focus change mid-sweep cannot make a walk change what it is walking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SearchScope {
    Main,
    Override,
    Manage,
}

/// Spec 0246 S13: how many committed patterns `Up` can reach. vim's
/// default `'history'`; the entries are short strings and no
/// measurement stands behind the number.
const SEARCH_HISTORY_MAX: usize = 50;

/// One candidate: a document line in the main pane, a list index in the
/// two side panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SweepCursor {
    Line(LinePos),
    Index(usize),
}

/// Spec 0246 S1: which of a candidate's matches count as stops.
///
/// A candidate is still a *row* — that is the unit of work the slice
/// budget counts, and the identity `next_candidate` enumerates. Match
/// granularity rides alongside it as this bound, which is `Whole` for
/// every row but the origin's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RowBound {
    /// Every match in the candidate is a stop.
    Whole,
    /// No stop here — spec 0246 N4: a side pane's origin entry on the
    /// way out, so that `n` leaves the entry it starts on.
    Nothing,
    /// Only matches whose byte start lies in `lo..hi`. The main pane's
    /// origin row, which S2 visits twice and S3 splits at the caret.
    Starts { lo: usize, hi: usize },
}

impl RowBound {
    fn admits(self, start: usize) -> bool {
        match self {
            Self::Whole => true,
            Self::Nothing => false,
            Self::Starts { lo, hi } => start >= lo && start < hi,
        }
    }
}

/// Spec 0246 S14: an open prompt's walk back through `search_history`.
pub(super) struct SearchBrowse {
    /// Which entry the buffer is showing.
    index: usize,
    /// What the user had typed before the first `Up`, restored by
    /// `Down` past the newest entry.
    draft: String,
}

/// Spec 0235 S2: what a sweep found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SweepHit {
    pub(super) at: SweepCursor,
    /// The match's byte offset in the haystack it was found in — zero
    /// for a path match, which spec 0246 S9 makes a single stop at the
    /// row's start. What a rotation restarts from (S18), and what S3's
    /// bounds are compared against.
    pub(super) start: usize,
    /// The column the caret lands on at `Enter` — the matched
    /// character's for a text match, the row's first non-blank for a
    /// path match (spec 0235 S20).
    pub(super) column: usize,
    /// How many columns the match covers — spec 0235 S14 tints it over
    /// its whole extent, and S12 centers that extent. One for a path
    /// match, which owns a single cell (S22).
    pub(super) width: usize,
    /// Spec 0235 S22: the pattern matched the row's positional path,
    /// which is not on screen, rather than its text.
    pub(super) on_path: bool,
    /// Spec 0274 S11: the match's other end — the row it stops on and
    /// the column one past its last character — when that is not this
    /// row.
    ///
    /// `None` is the single-row case, which is every hit a pattern that
    /// cannot match a `\n` can produce and which `width` already
    /// describes. `width` keeps its meaning either way: it is what the
    /// match covers of *this* row, so the centering and the first row's
    /// tint read it unchanged.
    pub(super) end: Option<(LinePos, usize)>,
}

/// Spec 0235 S6: where a `/`/`?` prompt was opened from — what every
/// keystroke searches again from, and what `Esc` puts back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SearchOrigin {
    pub(super) scope: SearchScope,
    scroll: PaneScroll,
    pan: usize,
    at: SweepCursor,
    /// Spec 0246 S8: the caret's byte offset within `at`'s row, which
    /// S3 splits that row at. Zero for a side pane, whose bounds are
    /// `Nothing`/`Whole` and need no offset.
    column: usize,
}

/// Spec 0235 S2: a search in progress.
pub(super) struct SearchSweep {
    pub(super) pattern: SearchPattern,
    dir: SearchDir,
    origin: SearchOrigin,
    /// The next candidate to test, and which of its matches count
    /// (spec 0246 S1). `None` once the walk has finished, either by
    /// finding a match or by seeing the whole document.
    at: Option<(SweepCursor, RowBound)>,
    /// Where `at`'s line begins in its owner's text — spec 0222 S4's
    /// byte cursor, carried rather than re-derived.
    ///
    /// A packed run's elements are all lines of *one* node (spec 0216
    /// S22), and `line_offset` counts newlines from that node's start,
    /// so asking it per candidate makes a walk across a run quadratic
    /// in the run's length: a 32 000-element run took 9.2 s to sweep
    /// and a 300 000-element one would have taken a quarter of an hour,
    /// which is a prompt that never answers rather than a slow one.
    /// Stepping to the neighboring newline instead is linear, and the
    /// draw path has been doing it since 0222.
    ///
    /// Zero and unread for the two side panes, whose candidates are
    /// list entries rather than lines.
    offset: usize,
    pub(super) found: Option<SweepHit>,
    /// Whether the walk ran out while the bake still owed the document
    /// text, which makes its miss provisional rather than an answer.
    ///
    /// Recorded when the walk ends rather than derived on demand: once
    /// the bake finishes, "is the document complete *now*" can no
    /// longer tell a miss that was always final from one that was taken
    /// too early, and only the second is worth asking again.
    provisional: bool,
    /// Candidates left before the whole document has been seen — the
    /// wrap budget, decremented per candidate.
    remaining: usize,
    /// Spec 0274 S9: the segments a multi-line pattern still has to
    /// scan, with the half of each that counts, in S14's order. Empty
    /// for every pattern that takes 0273's per-row walk, which uses
    /// `at` instead.
    ///
    /// **Frozen at construction.** It is the segmentation as it stood
    /// when the search began; segments the bake creates or joins
    /// afterwards do not enter it, because scanning a joined segment
    /// means scanning both its sides again and the bake joins one every
    /// few tens of milliseconds. S15's second pass is what covers the
    /// material a join makes reachable.
    segments: VecDeque<(Segment, RowBound)>,
    /// Spec 0274 S9: the segment a worker thread is scanning right now.
    ///
    /// Dropping it aborts and joins, so a sweep that is replaced or
    /// abandoned needs no shutdown of its own.
    scan: Option<SegmentScan>,
    /// Spec 0235 S21.
    path: PathScratch,
    /// Spec 0277 S6: the place, in the tally's numbering, of the match
    /// this sweep is *leaving*.
    ///
    /// Decided at departure rather than reconstructed at arrival: at
    /// the moment of departure the displayed hit is still the displayed
    /// one and the caret has not moved yet, so "am I leaving match *k*"
    /// is a question about the state the reader is looking at rather
    /// than a guess about where they ended up. `None` when the search
    /// did not start from the match the ordinal describes, which is
    /// what sends the tally to S7's prefix walk.
    from_ordinal: Option<usize>,
}

impl SearchSweep {
    /// Whether the walk is over. A finished sweep is *kept* rather than
    /// dropped: the prompt still has to draw its answer, and `Enter`
    /// still has to commit it.
    pub(super) fn is_finished(&self) -> bool {
        self.at.is_none() && self.segments.is_empty() && self.scan.is_none()
    }
}

/// Spec 0277 S1: how many matches the document holds, and which of them
/// the reader is on.
///
/// **Two scalars, not a list of positions.** `total` and `ordinal` are
/// facts with two different lifetimes, and keeping them apart is what
/// makes the rest of this small: the total survives every movement of
/// the reader, and only the ordinal has to keep up with it. Remembering
/// every match's position instead would make the ordinal a lookup, at
/// the price of a cap — a one-character pattern over a five-million-row
/// document asks for hundreds of megabytes — and the cap would land on
/// exactly the documents the feature exists for.
pub(super) struct SearchTally {
    /// Spec 0277 S11: what the two answers are about. Any of the three
    /// changing drops both and starts a new walk; a change in the
    /// *displayed hit* touches only the ordinal.
    scope: SearchScope,
    text: String,
    version: u64,
    /// `text` compiled, shared with `render`'s own cache — after spec
    /// 0273 a compile is a real cost.
    pattern: Rc<SearchPattern>,
    /// The walk in flight, or `None` once it has closed.
    walk: Option<TallyWalk>,
    /// Spec 0277 S5: published together, when the walk closes. An
    /// ordinal without a total cannot wrap and has nothing to be drawn
    /// beside, so there is no state in which one of the two is useful
    /// alone.
    total: Option<usize>,
    ordinal: Option<usize>,
    /// The hit `ordinal` describes. Spec 0277 S6 re-points this as the
    /// reader steps, rather than deriving the ordinal afresh.
    of: Option<(SweepCursor, usize)>,
}

/// One counting pass over the document's candidates.
///
/// The sweep's own walk with the early exit removed and every match of a
/// candidate taken instead of the first: same candidates, same
/// enumeration, and `RowBound::Whole` throughout, since a tally has no
/// origin to split (spec 0277 S4). That reuse is the invariant the pair
/// rests on — the tally counts exactly the stops of one full cycle of
/// `n`, and a folded subtree contributes candidates to neither walk.
struct TallyWalk {
    /// The next candidate, `None` once the walk has closed.
    at: Option<SweepCursor>,
    /// Where `at`'s line begins in its owner's text — spec 0272's
    /// carried byte cursor, for the same reason: asking per candidate
    /// makes a walk across a packed run quadratic in the run's length.
    offset: usize,
    /// Candidates left before the whole document has been seen. One per
    /// candidate and no `+ 1`: a tally starts at the document's first
    /// candidate rather than at an origin, so nothing is visited twice.
    remaining: usize,
    /// Matches counted so far.
    count: usize,
    /// The displayed hit the ordinal is being derived for, as the
    /// candidate it sits in and the byte its stop is placed at.
    target: (SweepCursor, usize),
    /// Its place, once the walk has reached it.
    ordinal: Option<usize>,
    /// Spec 0277 S7: stop at `target` rather than at the end of the
    /// document. The total is still valid, so the only missing fact is
    /// how many matches lie before the displayed hit.
    prefix: bool,
    /// Spec 0235 S21.
    path: PathScratch,
}

impl App {
    /// Which pane the prompt belongs to, by the same focus test the
    /// `Enter` arm has always used.
    pub(super) fn search_scope(&self) -> SearchScope {
        if self.override_focus {
            SearchScope::Override
        } else if self.manage_open && self.manage_focus {
            SearchScope::Manage
        } else {
            SearchScope::Main
        }
    }

    /// Spec 0235 S6: this pane's current position and view.
    pub(super) fn search_origin_for(&self, scope: SearchScope) -> SearchOrigin {
        let (scroll, pan, at, column) = match scope {
            SearchScope::Main => {
                let pos = LinePos {
                    node: self.cursor,
                    line_in_node: self.cursor_line_in_node,
                };
                // Spec 0246 S8: `cursor_column` counts characters and
                // the sweep's bounds count bytes, so the conversion
                // happens once, here, rather than per candidate.
                let text = self.line_text(pos);
                let column = text
                    .char_indices()
                    .nth(self.cursor_column)
                    .map_or(text.len(), |(i, _)| i);
                (self.scroll, self.pan_offset, SweepCursor::Line(pos), column)
            }
            SearchScope::Override => (
                self.override_scroll,
                self.override_pan_offset,
                SweepCursor::Index(self.override_highlight),
                0,
            ),
            SearchScope::Manage => (
                self.manage_scroll,
                self.manage_pan_offset,
                SweepCursor::Index(self.manage_highlight),
                0,
            ),
        };
        SearchOrigin {
            scope,
            scroll,
            pan,
            at,
            column,
        }
    }

    /// Spec 0235 S11: put the view back where the prompt found it. The
    /// position itself needs no restoring — by S8 a sweep never moved
    /// it.
    fn restore_search_origin(&mut self, origin: SearchOrigin) {
        match origin.scope {
            SearchScope::Main => {
                self.scroll = origin.scroll;
                self.pan_offset = origin.pan;
            }
            SearchScope::Override => {
                self.override_scroll = origin.scroll;
                self.override_pan_offset = origin.pan;
            }
            SearchScope::Manage => {
                self.manage_scroll = origin.scroll;
                self.manage_pan_offset = origin.pan;
            }
        }
    }

    /// How many candidates a full wrap covers.
    fn search_candidate_count(&self, scope: SearchScope) -> usize {
        match scope {
            SearchScope::Main => self.total_lines(),
            SearchScope::Override => self.override_candidates.len(),
            SearchScope::Manage => self.overrides.entries().len(),
        }
    }

    /// The candidate after `at` in `dir`, wrapping at the ends.
    fn next_candidate(&self, at: SweepCursor, dir: SearchDir, n: usize) -> Option<SweepCursor> {
        match at {
            SweepCursor::Line(pos) => match dir {
                SearchDir::Forward => self.next_line(pos).or_else(|| self.first_line()),
                SearchDir::Backward => self.prev_line(pos).or_else(|| self.last_line()),
            }
            .map(SweepCursor::Line),
            SweepCursor::Index(i) => (n > 0).then(|| {
                SweepCursor::Index(match dir {
                    SearchDir::Forward => (i + 1) % n,
                    SearchDir::Backward => (i + n - 1) % n,
                })
            }),
        }
    }

    /// A sweep of `pattern` over `origin`'s pane.
    ///
    /// Spec 0246 S2: it starts *at* the origin's own row and budgets one
    /// candidate more than the pane has, so that row is visited twice —
    /// once for the matches past the caret and once, at the end of the
    /// wrap, for the ones at or before it. The two halves partition the
    /// row, which is what makes a full cycle visit every match exactly
    /// once and land back where it started rather than report "not
    /// found".
    ///
    /// `None` for an empty pattern or an empty pane, neither of which
    /// has anything to walk.
    pub(super) fn begin_search_sweep(
        &self,
        pattern: &str,
        dir: SearchDir,
        origin: SearchOrigin,
    ) -> Option<SearchSweep> {
        let n = self.search_candidate_count(origin.scope);
        if pattern.is_empty() || n == 0 {
            return None;
        }
        // Spec 0273 S8: a pattern that does not compile is not a
        // search. `foo(` is what `foo(bar)` looks like halfway through
        // typing it.
        let pattern = SearchPattern::new(pattern).ok()?;
        // Spec 0274 S1: a pattern that can match a `\n` searches the
        // rows joined, which is a different walk over a different
        // haystack. Everything else keeps 0273's row-at-a-time one.
        let segments = match pattern {
            SearchPattern::Multi(_) => self.segment_queue(origin, dir),
            _ => VecDeque::new(),
        };
        Some(SearchSweep {
            pattern,
            dir,
            origin,
            at: segments
                .is_empty()
                .then(|| (origin.at, origin_bound(origin, dir, Visit::Out))),
            offset: match origin.at {
                SweepCursor::Line(pos) => self.line_offset(pos),
                SweepCursor::Index(_) => 0,
            },
            found: None,
            provisional: false,
            remaining: n + 1,
            segments,
            scan: None,
            path: PathScratch::default(),
            from_ordinal: None,
        })
    }

    /// Spec 0274 S14: the segments a multi-line sweep will scan, in the
    /// order it will scan them.
    ///
    /// The forward walk takes the origin's segment from the caret to its
    /// end, then every following segment whole, wraps, then every
    /// preceding one whole, and closes on the head of the origin's up to
    /// and including the caret. Backward is the mirror. That is 0246
    /// S2's two-visit partition of the origin, stated over segment bytes
    /// rather than row bytes, so a full cycle still visits every match
    /// exactly once and lands back where it started.
    ///
    /// Empty for a scope that has no document — a side pane keeps the
    /// per-row walk unconditionally (N7).
    fn segment_queue(&self, origin: SearchOrigin, dir: SearchDir) -> VecDeque<(Segment, RowBound)> {
        let SweepCursor::Line(pos) = origin.at else {
            return VecDeque::new();
        };
        let (segments, stops) = self.search_segments();
        let Some(last) = segments.len().checked_sub(1) else {
            return VecDeque::new();
        };
        let place = self.place_of(pos);
        let k = self.segment_index_of(place, &stops).min(last);
        // The caret's byte in its segment: where its chunk begins, plus
        // where its row begins in that chunk — nonzero only inside a
        // packed run — plus the caret's own byte column.
        let caret = self.segment_byte_of(segments[k], place).unwrap_or(0)
            + self.line_offset(pos)
            + origin.column;

        let mut queue = VecDeque::with_capacity(segments.len() + 1);
        queue.push_back((segments[k], split_at(caret, dir, Visit::Out)));
        let rest: Vec<usize> = match dir {
            SearchDir::Forward => (k + 1..segments.len()).chain(0..k).collect(),
            SearchDir::Backward => (0..k).rev().chain((k + 1..segments.len()).rev()).collect(),
        };
        queue.extend(rest.into_iter().map(|i| (segments[i], RowBound::Whole)));
        queue.push_back((segments[k], split_at(caret, dir, Visit::Back)));
        queue
    }

    /// Whether one candidate matches, and where.
    ///
    /// Spec 0273 S5: a main-pane line has *one* haystack, chosen by the
    /// pattern's shape — the owning node's positional path when the
    /// pattern looks like one, and the rendered row text otherwise.
    /// Never both, which is what deletes spec 0246 S9's "the path is the
    /// row's stop only when its text has no match at all" guard and the
    /// extra scan that went with it.
    ///
    /// Spec 0246 S1: `bound` says which of the candidate's matches are
    /// stops — everything but the origin's own row passes `Whole`.
    fn sweep_test(
        &self,
        sweep: &mut SearchSweep,
        at: SweepCursor,
        bound: RowBound,
    ) -> Option<SweepHit> {
        let haystack: Cow<'_, str> = match (sweep.origin.scope, at) {
            (SearchScope::Main, SweepCursor::Line(pos)) => {
                // A closing `}` draws no content of its own, and a
                // search has never matched one.
                if self.is_footer(pos) {
                    return None;
                }
                if sweep.pattern.is_path() {
                    // Spec 0273 S4: a node's own lines all carry the
                    // same path, so only the first of them is a
                    // candidate — otherwise a three-line node is three
                    // consecutive stops on one answer.
                    if pos.line_in_node != 0 {
                        return None;
                    }
                    let text = self.line_text_at(pos, sweep.offset);
                    let indent = text.len() - text.trim_start().len();
                    drop(text);
                    // The stop sits where its caret lands, so a bound
                    // that has already passed that offset excludes it
                    // and a full cycle still visits it once.
                    if !bound.admits(indent) {
                        return None;
                    }
                    self.write_path_segments(&mut sweep.path, pos.node);
                    return sweep
                        .pattern
                        .matches_path(sweep.path.segments())
                        .then_some(SweepHit {
                            at,
                            start: indent,
                            column: indent,
                            width: 1,
                            on_path: true,
                            end: None,
                        });
                }
                let text = self.line_text_at(pos, sweep.offset);
                // The row's first non-blank — the floor every stop's
                // position is measured against (spec 0246 S3a).
                let indent = text.len() - text.trim_start().len();
                let range = pick_match(&sweep.pattern, &text, indent, sweep.dir, bound)?;
                return Some(SweepHit {
                    at,
                    start: range.start.max(indent),
                    column: text[..range.start].chars().count(),
                    width: text[range].chars().count(),
                    on_path: false,
                    end: None,
                });
            }
            // Spec 0235 S23: the side panes list FQDNs, not nodes, so
            // they have one haystack and no path rule.
            (SearchScope::Override, SweepCursor::Index(i)) => {
                Cow::Borrowed(self.override_candidates[i].0.as_str())
            }
            (SearchScope::Manage, SweepCursor::Index(i)) => Cow::Owned(self.manage_search_text(i)),
            // A scope is fixed at construction and picks the cursor
            // shape with it, so the remaining pairs do not occur.
            _ => return None,
        };
        // Spec 0246 N4: a side pane's entry is a whole-row highlight, so
        // it stays one stop — its origin is excluded by `Nothing` on the
        // way out and admitted whole on the way back, and no `Starts`
        // bound is ever built for an index cursor.
        if bound == RowBound::Nothing {
            return None;
        }
        sweep.pattern.find_range(&haystack).map(|range| SweepHit {
            at,
            // Spec 0246 N4: never read back — a side pane's stops are
            // its entries, and only a `Nothing` bound ever skips one.
            start: range.start,
            column: haystack[..range.start].chars().count(),
            width: haystack[range].chars().count(),
            on_path: false,
            end: None,
        })
    }

    /// Visit at most `budget` candidates, stopping early on a match or
    /// at the end of the walk.
    fn advance_sweep(&self, sweep: &mut SearchSweep, budget: usize) {
        let n = self.search_candidate_count(sweep.origin.scope);
        for _ in 0..budget {
            let Some((at, bound)) = sweep.at else { return };
            if let Some(hit) = self.sweep_test(sweep, at, bound) {
                sweep.found = Some(hit);
                sweep.at = None;
                return;
            }
            sweep.remaining -= 1;
            let next = (sweep.remaining > 0)
                .then(|| self.next_candidate(at, sweep.dir, n))
                .flatten();
            // Spec 0246 S2: `next_candidate` is a bijection over the
            // candidates, so the origin comes round again exactly once —
            // on the last of the `n + 1` visits — and that is the one
            // that takes the closing half of the split.
            let origin_at = sweep.origin.at;
            let closing = origin_bound(sweep.origin, sweep.dir, Visit::Back);
            if let (SweepCursor::Line(from), Some(SweepCursor::Line(to))) = (at, next) {
                sweep.offset = self.stepped_offset(from, sweep.offset, to);
            }
            sweep.at = next.map(|next| {
                let bound = if next == origin_at {
                    closing
                } else {
                    RowBound::Whole
                };
                (next, bound)
            });
        }
    }

    /// Spec 0274 S9/S10: one step of a multi-line sweep — collect the
    /// segment a worker has finished, or hand out the next one.
    ///
    /// **The segment is the unit of work**, not the slice `budget` the
    /// per-row walk counts: a cross-row engine's state is its own, so
    /// there is nowhere to stop in the middle of one. The queue is what
    /// bounds the stall instead, and it is self-limiting — segments are
    /// many and small exactly while the bake still has the most to do.
    ///
    /// `offload` is false for the callers that must answer before they
    /// return; they scan on this thread, where a worker's report would
    /// have nobody listening for it.
    fn advance_segment_sweep(&self, sweep: &mut SearchSweep, offload: bool) -> SweepStep {
        if let Some(scan) = sweep.scan.take() {
            let Some(found) = scan.collect() else {
                sweep.scan = Some(scan);
                return SweepStep::Waiting;
            };
            let seg = scan.seg;
            // Dropped here: the thread is already done, so the join in
            // `SegmentScan`'s destructor returns at once.
            drop(scan);
            if let Some(span) = found {
                self.take_segment_hit(sweep, seg, span);
            }
            // Spec 0274 S10: the yield, and it has to be `Idle` rather
            // than `Progressed` — `Progressed` `continue`s past
            // discard, bake and read-ahead, so this is the pass that
            // lets each of those three take one step.
            return SweepStep::Idle;
        }
        let Some((seg, bound)) = sweep.segments.pop_front() else {
            return SweepStep::Idle;
        };
        let (SearchPattern::Multi(re), Some((lo, hi))) = (&sweep.pattern, bound_span(bound)) else {
            return SweepStep::Progressed;
        };
        if offload {
            if let Some(scan) = self.spawn_segment_scan(re, seg, bound, sweep.dir, (lo, hi)) {
                sweep.scan = Some(scan);
                return SweepStep::Waiting;
            }
        }
        // Nowhere to report to, so the scan runs here. Slower for the
        // reader, but it is what a headless session and the tests get,
        // and it is the walk stage 2 shipped.
        if let Some(span) = self.scan_segment_inline(re, seg, sweep.dir, lo, hi) {
            self.take_segment_hit(sweep, seg, span);
        }
        SweepStep::Progressed
    }

    /// A segment's answer, read as the sweep's. A hit ends the walk:
    /// the queue is in S14's order, so the first segment to answer is
    /// the nearest one.
    fn take_segment_hit(&self, sweep: &mut SearchSweep, seg: Segment, span: Range<usize>) {
        if let Some(hit) = self.hit_from_span(seg, span) {
            sweep.found = Some(hit);
            sweep.segments.clear();
        }
    }

    /// Spec 0274 S9: run a sweep to its end on this thread.
    ///
    /// The two non-incremental callers — `n`/`N` and a committed
    /// prompt — have no loop to interleave with and have already told
    /// the reader the wait is the answer's cost, so a multi-line sweep
    /// scans its segments here rather than handing them out.
    fn drain_sweep(&self, sweep: &mut SearchSweep) {
        // A scan the prompt had already handed out is recalled rather
        // than waited on: dropping it aborts and joins (S7), and its
        // segment goes back at the head of the queue to be redone here.
        // Spinning on `collect` instead would burn the very core the
        // scan is running on.
        if let Some(scan) = sweep.scan.take() {
            sweep.segments.push_front((scan.seg, scan.bound));
        }
        while !sweep.is_finished() {
            if sweep.segments.is_empty() {
                self.advance_sweep(sweep, usize::MAX);
            } else {
                self.advance_segment_sweep(sweep, false);
            }
        }
    }

    /// Spec 0274 S8: end any segment scan in flight so that the caller
    /// may write the document.
    ///
    /// The segment goes back at the head of the queue: a sweep that
    /// silently dropped one would report a miss over bytes it never
    /// looked at. By S5 a bake step cannot have changed the interior of
    /// a segment, so redoing one is redoing the same work.
    ///
    /// Called from [`App::tree_mut`] and [`App::node_text_mut`] rather
    /// than from the writers themselves, which is what makes it
    /// impossible to forget.
    pub(super) fn halt_search_scan(&mut self) {
        let Some(sweep) = self.search_sweep.as_mut() else {
            return;
        };
        if let Some(scan) = sweep.scan.take() {
            sweep.segments.push_front((scan.seg, scan.bound));
        }
    }

    /// Spec 0274 S10: whether the frozen queue still owes a segment.
    ///
    /// `search_sweep_step` reports `Idle` when it collects a verdict, so
    /// that discard, bake and read-ahead each get a step. That is a
    /// yield and not an answer, and `run_loop` reads this to tell the
    /// two apart before it decides to sleep.
    pub(super) fn search_segment_pending(&self) -> bool {
        self.search_sweep
            .as_ref()
            .is_some_and(|sweep| !sweep.segments.is_empty())
    }

    /// A byte span inside `seg`, read as a stop.
    ///
    /// Spec 0274 S11: two positions rather than a row and a width. The
    /// stop itself is still the row the match *starts* on — that is
    /// where the sweep is, where the view centers and where a rotation
    /// resumes — and the second position says how far past it the match
    /// runs.
    fn hit_from_span(&self, seg: Segment, span: Range<usize>) -> Option<SweepHit> {
        let (pos, row) = self.locate_in_segment(seg, span.start)?;
        let text = self.line_text(pos);
        let start = span.start - row.start;
        // Its own row's share of the match, which is all of it when the
        // two ends are on the same row and the rest of the row when
        // they are not.
        let end = span.end.min(row.end) - row.start;
        // The row's first non-blank, which spec 0246 S3a makes the floor
        // every stop's position is measured against.
        let indent = text.len() - text.trim_start().len();
        Some(SweepHit {
            at: SweepCursor::Line(pos),
            start: start.max(indent),
            column: text[..start].chars().count(),
            width: text[start..end].chars().count(),
            on_path: false,
            end: self.span_end_position(seg, span.end, pos),
        })
    }

    /// Spec 0274 S11's second position — the row byte `at` falls on and
    /// the column one past it — or `None` when that is `from`'s own row
    /// and the hit is the single-row one every other caller expects.
    ///
    /// Located rather than carried along: `locate_in_segment` is a walk
    /// from the segment's start, and paying for it twice happens once
    /// per *found* match rather than once per candidate.
    fn span_end_position(
        &self,
        seg: Segment,
        at: usize,
        from: LinePos,
    ) -> Option<(LinePos, usize)> {
        let (pos, row) = self.locate_in_segment(seg, at)?;
        if pos == from {
            return None;
        }
        let text = self.line_text(pos);
        Some((pos, text[..at - row.start].chars().count()))
    }

    /// Where `to`'s line begins in its owner's text, given that `from`'s
    /// began at `offset`.
    ///
    /// The walk's common case is the next or previous line of the same
    /// node, and that is the case worth having: both are one newline
    /// away, so both are found by scanning from `offset` rather than
    /// from the node's start. Every other step lands on a line 0 or on
    /// a wrap, where `line_offset` is already O(1) or is paid once.
    fn stepped_offset(&self, from: LinePos, offset: usize, to: LinePos) -> usize {
        // A bracketed node's only other line is its derived closing
        // brace, which is not in the text at all (spec 0222 S2).
        if to.node != from.node || self.tree[from.node].is_bracketed() {
            return self.line_offset(to);
        }
        let Some(text) = self.node_text[from.node].as_deref() else {
            return 0;
        };
        if to.line_in_node == from.line_in_node + 1 {
            return match text[offset..].find('\n') {
                Some(at) => offset + at + 1,
                None => text.len(),
            };
        }
        if to.line_in_node + 1 == from.line_in_node {
            // `from` is not line 0, so `offset` is one past the newline
            // that ended `to` — and the newline before *that* one, if
            // there is one, ends the line before `to`.
            return text[..offset - 1].rfind('\n').map_or(0, |at| at + 1);
        }
        self.line_offset(to)
    }

    /// Spec 0235 S2/S3: move the live sweep on by one slice.
    ///
    /// `Idle` means the loop may sleep — no sweep, or one whose walk is
    /// over. Everything else is `Progressed`, which is what keeps
    /// `run_loop` off `recv_timeout` for as long as an answer is owed.
    pub(super) fn search_sweep_step(&mut self) -> SweepStep {
        let Some(mut sweep) = self.search_sweep.take() else {
            return SweepStep::Idle;
        };
        if sweep.is_finished() {
            // A miss taken before the bake had finished was never an
            // answer, and nothing else would ever revise it — the
            // sweep is kept, not dropped, so without this the prompt
            // would still be reporting the smaller document long after
            // the rest of it had arrived. The bake runs only while
            // this returns `Idle` (spec 0255 S5's ordering), so the
            // question is asked again exactly once, at the first step
            // after the last subtree lands.
            let stale = sweep.provisional && self.search_miss_is_conclusive(sweep.origin.scope);
            self.search_sweep = Some(sweep);
            if stale {
                self.restart_search_sweep();
                return SweepStep::Progressed;
            }
            return SweepStep::Idle;
        }
        let before = sweep.found;
        // Spec 0274 S10: a multi-line sweep walks segments instead of
        // rows, and one segment is one worker's task, so its step
        // reports for itself whether it handed work out, waited or
        // yielded.
        let step = if sweep.segments.is_empty() && sweep.scan.is_none() {
            self.advance_sweep(&mut sweep, SEARCH_SWEEP_SLICE);
            SweepStep::Progressed
        } else {
            self.advance_segment_sweep(&mut sweep, true)
        };
        let found_changed = sweep.found != before;
        // Spec 0235 S5: the walk finishing with nothing is the *other*
        // change to the result — it is what turns "still looking" into
        // "not there", which S10 draws.
        let exhausted = sweep.is_finished() && sweep.found.is_none();
        if exhausted {
            sweep.provisional = !self.search_miss_is_conclusive(sweep.origin.scope);
        }
        let hit = sweep.found;
        self.search_sweep = Some(sweep);
        if found_changed {
            self.track_search_tally();
            if let Some(hit) = hit {
                self.show_sweep_hit(hit);
            }
        }
        self.search_dirty |= found_changed || exhausted;
        step
    }

    /// Spec 0277: what a tally would be about — the pane, the pattern,
    /// the pattern compiled, and the hit whose place the ordinal names —
    /// or `None` when there is nothing to count.
    ///
    /// Four of the spec's limits live here, because all four are the
    /// same question asked once a step:
    ///
    /// - S12, the tally exists only while the matches are tinted;
    /// - N3, no `0 of 0` — a miss is already reported on this row by
    ///   the pattern's own color, and a sweep still walking has no hit
    ///   for the ordinal to be about;
    /// - N2, no total over a document the bake is still growing;
    /// - N1, no count for a cross-row pattern, whose unit of work is a
    ///   segment on a worker thread rather than a row on this one.
    fn tally_subject(&self) -> Option<(SearchScope, String, Rc<SearchPattern>, SweepHit)> {
        if !self.search_highlight {
            return None;
        }
        let sweep = self.search_sweep.as_ref()?;
        let hit = sweep.found?;
        let scope = sweep.origin.scope;
        if !self.search_miss_is_conclusive(scope) {
            return None;
        }
        let text = self.tally_pattern_text(scope)?;
        let pattern = self.compiled_pattern(&text)?;
        if matches!(*pattern, SearchPattern::Multi(_)) {
            return None;
        }
        Some((scope, text, pattern, hit))
    }

    /// The *sweep's* pattern, read the way spec 0235 S15's highlight
    /// reads it but keyed on the sweep's own scope rather than on
    /// whichever pane happens to have focus now.
    fn tally_pattern_text(&self, scope: SearchScope) -> Option<String> {
        match &self.command_buffer {
            Some(buf)
                if matches!(self.command_kind, CommandLineKind::Search { .. })
                    && !buf.is_empty() =>
            {
                Some(buf.clone())
            }
            _ => self.last_search_for(scope).map(|(_, p)| p.clone()),
        }
    }

    /// Spec 0277 S3: a walk forward from the document's first candidate,
    /// whatever the search's direction.
    ///
    /// "Counting from the beginning of the document" is the whole
    /// meaning of the first number, so a backward search's `n`
    /// *decrements* it.
    fn begin_tally_walk(
        &self,
        scope: SearchScope,
        target: (SweepCursor, usize),
        prefix: bool,
    ) -> TallyWalk {
        let at = match scope {
            SearchScope::Main => self.first_line().map(SweepCursor::Line),
            _ => (self.search_candidate_count(scope) > 0).then_some(SweepCursor::Index(0)),
        };
        TallyWalk {
            offset: match at {
                Some(SweepCursor::Line(pos)) => self.line_offset(pos),
                _ => 0,
            },
            at,
            remaining: self.search_candidate_count(scope),
            count: 0,
            target,
            ordinal: None,
            prefix,
            path: PathScratch::default(),
        }
    }

    /// Spec 0277 S2: visit at most `budget` candidates, counting.
    ///
    /// The walk is uncapped and exact. `SEARCH_SWEEP_SLICE`'s doc
    /// comment records a full sweep of googleapis.desc's 5 281 124
    /// lines at 647–961 ms, converged one slice at a time, and this is
    /// that same candidate walk; roughly a second on the largest corpus
    /// in the project, for a job nothing is waiting on.
    fn advance_tally(&self, tally: &mut SearchTally, budget: usize) {
        let scope = tally.scope;
        let pattern = &*tally.pattern;
        let Some(walk) = tally.walk.as_mut() else {
            return;
        };
        let n = self.search_candidate_count(scope);
        for _ in 0..budget {
            let Some(at) = walk.at else { return };
            let (stops, place) =
                self.count_stops(&mut walk.path, pattern, scope, at, walk.offset, walk.target);
            if let Some(k) = place {
                // Spec 0277 S5: the ordinal comes out of the same walk,
                // so the first `27 of 42` costs no more than the `42`.
                walk.ordinal = Some(walk.count + k);
                // Spec 0277 S7: a prefix walk wanted exactly this and
                // has no total left to finish.
                if walk.prefix {
                    walk.at = None;
                    return;
                }
            }
            walk.count += stops;
            walk.remaining -= 1;
            let next = (walk.remaining > 0)
                .then(|| self.next_candidate(at, SearchDir::Forward, n))
                .flatten();
            if let (SweepCursor::Line(from), Some(SweepCursor::Line(to))) = (at, next) {
                walk.offset = self.stepped_offset(from, walk.offset, to);
            }
            walk.at = next;
        }
    }

    /// Spec 0277 S4: how many stops one candidate holds, and — when the
    /// walk's target is among them — which of them it is.
    ///
    /// Exactly [`App::sweep_test`]'s haystack choice, with
    /// [`pick_match`]'s single answer widened to the whole set. Reusing
    /// the choice rather than writing a second matching rule beside the
    /// first is what makes the count *be* the number of stops in one
    /// full cycle of `n`: every match in every candidate, one stop for
    /// a row the pattern matched on its path (spec 0273 S5), and
    /// nothing for a footer row.
    fn count_stops(
        &self,
        path: &mut PathScratch,
        pattern: &SearchPattern,
        scope: SearchScope,
        at: SweepCursor,
        offset: usize,
        target: (SweepCursor, usize),
    ) -> (usize, Option<usize>) {
        // The target is somewhere else entirely unless it is here.
        let here = |k: usize| (target.0 == at).then_some(k);
        let haystack: Cow<'_, str> = match (scope, at) {
            (SearchScope::Main, SweepCursor::Line(pos)) => {
                // A closing `}` draws no content of its own, and a
                // search has never matched one.
                if self.is_footer(pos) {
                    return (0, None);
                }
                if pattern.is_path() {
                    // Spec 0273 S4: a node's own lines all carry the
                    // same path, so only the first of them is a
                    // candidate.
                    if pos.line_in_node != 0 {
                        return (0, None);
                    }
                    self.write_path_segments(path, pos.node);
                    if !pattern.matches_path(path.segments()) {
                        return (0, None);
                    }
                    // Spec 0246 S9: one stop, at the row's start.
                    return (1, here(1));
                }
                let text = self.line_text_at(pos, offset);
                // The row's first non-blank — the floor every stop's
                // position is measured against (spec 0246 S3a).
                let indent = text.len() - text.trim_start().len();
                let mut count = 0;
                let mut place = None;
                let mut last = None;
                let mut from = 0;
                while from <= text.len() {
                    let Some(range) = pattern.find_range_from(&text, from) else {
                        break;
                    };
                    // Spec 0246 S5: one character past the match's
                    // *start*, not its end, so overlapping matches are
                    // separate stops — and so an empty match (`^`,
                    // `x*`) terminates the scan.
                    from =
                        range.start + text[range.start..].chars().next().map_or(1, char::len_utf8);
                    let start = range.start.max(indent);
                    // Spec 0246 S3a again: several matches inside one
                    // indentation are the single stop the caret
                    // collapses them to, and counting them separately
                    // would make a total `n` could never walk.
                    if last == Some(start) {
                        continue;
                    }
                    last = Some(start);
                    count += 1;
                    if target.1 == start {
                        place = here(count);
                    }
                }
                return (count, place);
            }
            // Spec 0235 S23: the side panes list FQDNs, not nodes, so
            // they have one haystack and no path rule.
            (SearchScope::Override, SweepCursor::Index(i)) => {
                Cow::Borrowed(self.override_candidates[i].0.as_str())
            }
            (SearchScope::Manage, SweepCursor::Index(i)) => Cow::Owned(self.manage_search_text(i)),
            // A scope picks its cursor shape, so the remaining pairs do
            // not occur.
            _ => return (0, None),
        };
        // Spec 0246 N4: a side pane's entry is a whole-row highlight, so
        // it is one stop however many times the pattern occurs in it.
        match pattern.find_range(&haystack) {
            Some(_) => (1, here(1)),
            None => (0, None),
        }
    }

    /// Spec 0277 S9: one slice of the tally's walk.
    ///
    /// Its own step in `run_loop`'s idle arm, and it runs **last** —
    /// after the sweep, the discard, the bake and the read-ahead. It is
    /// the only one of the five jobs there that nothing on screen is
    /// waiting for. The slice is `SEARCH_SWEEP_SLICE` candidates, like
    /// the sweep's, for the same reason and with the same effect on key
    /// latency.
    pub(super) fn search_tally_step(&mut self) -> SweepStep {
        let Some((scope, text, pattern, hit)) = self.tally_subject() else {
            self.drop_tally();
            return SweepStep::Idle;
        };
        let version = self.structural_version;
        let stale = match &self.search_tally {
            Some(tally) => tally.scope != scope || tally.text != text || tally.version != version,
            None => true,
        };
        if stale {
            // Spec 0277 S11: a new key is a new tally, both facts
            // dropped.
            self.drop_tally();
            let walk = self.begin_tally_walk(scope, (hit.at, hit.start), false);
            self.search_tally = Some(SearchTally {
                scope,
                text,
                version,
                pattern,
                walk: Some(walk),
                total: None,
                ordinal: None,
                of: None,
            });
            return SweepStep::Progressed;
        }
        let mut tally = self.search_tally.take().expect("not stale, so present");
        self.follow_tally_hit(&mut tally, hit);
        if tally.walk.is_none() {
            self.search_tally = Some(tally);
            return SweepStep::Idle;
        }
        self.advance_tally(&mut tally, SEARCH_SWEEP_SLICE);
        if tally.walk.as_ref().is_some_and(|walk| walk.at.is_none()) {
            let walk = tally.walk.take().expect("just tested");
            // Spec 0277 S5: both facts published together. A prefix
            // walk (S7) saw only part of the document and so revises
            // only the ordinal.
            if !walk.prefix {
                tally.total = Some(walk.count);
            }
            tally.ordinal = walk.ordinal;
            tally.of = Some(walk.target);
            // Spec 0277 S10: a finished walk owes exactly one repaint.
            // `may_sleep_indefinitely` has no tally term and must not
            // grow one — without this the number would first appear on
            // the reader's next keystroke.
            self.search_dirty = true;
        }
        self.search_tally = Some(tally);
        SweepStep::Progressed
    }

    /// Spec 0277 S6: the displayed hit has just changed, so the ordinal
    /// must too.
    ///
    /// Called where the change happens rather than only from
    /// [`App::search_tally_step`], because the frame drawing the new
    /// match goes out before `run_loop` next reaches its idle arm — an
    /// ordinal that caught up a frame later would be a number the
    /// reader watches lag behind the match it counts.
    pub(super) fn track_search_tally(&mut self) {
        let Some(mut tally) = self.search_tally.take() else {
            return;
        };
        if let Some(hit) = self.search_sweep.as_ref().and_then(|sweep| sweep.found) {
            self.follow_tally_hit(&mut tally, hit);
        }
        self.search_tally = Some(tally);
    }

    /// Forget the tally, and owe the frame that erases a number the
    /// reader can see.
    fn drop_tally(&mut self) {
        if let Some(old) = self.search_tally.take() {
            self.search_dirty |= old.total.is_some();
        }
    }

    /// Spec 0277 S6/S7: keep the ordinal on the hit that is displayed.
    ///
    /// The sweep that landed carries the ordinal of the match it left,
    /// so stepping it is `± 1` and a wrap at `total`. A sweep that
    /// carried none — the reader moved off the match, then pressed `n`
    /// — leaves the ordinal unknown and starts S7's prefix walk, which
    /// stops at the hit rather than running to the end: the total is
    /// still valid, so the only missing fact is how many matches lie
    /// before it.
    fn follow_tally_hit(&self, tally: &mut SearchTally, hit: SweepHit) {
        let now = (hit.at, hit.start);
        if let Some(walk) = &tally.walk {
            // A walk in flight is already looking for a hit; if that is
            // no longer the one on screen it is looking for the wrong
            // one, and there is no ordinal yet to step from.
            if walk.target != now {
                let prefix = tally.total.is_some();
                tally.walk = Some(self.begin_tally_walk(tally.scope, now, prefix));
            }
            return;
        }
        if tally.of == Some(now) {
            return;
        }
        tally.of = Some(now);
        let sweep = self.search_sweep.as_ref();
        match (sweep.and_then(|sweep| sweep.from_ordinal), tally.total) {
            (Some(from), Some(total)) if total > 0 => {
                let dir = sweep.map_or(SearchDir::Forward, |sweep| sweep.dir);
                tally.ordinal = Some(match dir {
                    SearchDir::Forward => from % total + 1,
                    SearchDir::Backward => (from + total - 2) % total + 1,
                });
            }
            _ => {
                tally.ordinal = None;
                tally.walk = Some(self.begin_tally_walk(tally.scope, now, true));
            }
        }
    }

    /// Spec 0277 S6: the ordinal a departing sweep carries — the place
    /// of the match it is leaving, and only when the tally is in fact
    /// describing that match.
    fn departing_ordinal(&self, from: SweepCursor, start: usize) -> Option<usize> {
        let tally = self.search_tally.as_ref()?;
        (tally.of == Some((from, start)))
            .then_some(tally.ordinal)
            .flatten()
    }

    /// Spec 0277 S6: the ordinal `n`/`N` carries out — the displayed
    /// hit's, when the origin lies **within its extent**.
    ///
    /// Not `origin.column == hit.start`: spec 0246 S3 leaves the caret
    /// on the match's first character but spec 0276 S5 deliberately
    /// leaves it on the *last*, and an equality test would make an `n`
    /// straight after an `Esc`-accepted find look like a jump. The
    /// extent covers both landings, and every caret nudge inside the
    /// match besides.
    fn caret_ordinal(&self, scope: SearchScope) -> Option<usize> {
        let hit = self.search_sweep.as_ref()?.found?;
        let inside = match (scope, hit.at) {
            (SearchScope::Main, SweepCursor::Line(pos)) => {
                pos.node == self.cursor
                    && pos.line_in_node == self.cursor_line_in_node
                    && self.cursor_column >= hit.column
                    && self.cursor_column < hit.column + hit.width.max(1)
            }
            // A side pane's stop is its whole entry, so standing on the
            // entry is standing in the match.
            (SearchScope::Override, SweepCursor::Index(i)) => i == self.override_highlight,
            (SearchScope::Manage, SweepCursor::Index(i)) => i == self.manage_highlight,
            _ => false,
        };
        inside
            .then(|| self.departing_ordinal(hit.at, hit.start))
            .flatten()
    }

    /// Spec 0277 S8: `27 of 42`, or `? of 42` while S7's walk is
    /// running. `None` — nothing drawn at all — until the total is
    /// known.
    ///
    /// The placeholder rather than dropping the whole field: the field
    /// keeps its width, so a reader stepping matches on a large
    /// document does not watch it appear and vanish, and the `?` says
    /// which of the two facts is the missing one.
    ///
    /// S11's key is re-checked here rather than trusted, because the
    /// walk that re-keys it is the idle arm's last job (S9) and the
    /// frame is drawn long before the loop reaches it. Without this, the
    /// keystroke that changes the pattern would draw one frame of the
    /// old pattern's total.
    ///
    /// Spec 0278 S2: the search text the command row is showing — an
    /// open search prompt's buffer, or the echo a committed search left
    /// behind — with the prefix character that names it.
    ///
    /// One predicate serving two readers: `render_command_row` draws
    /// what this returns, and spec 0277's count is drawn only where it
    /// returns something. That is the whole of "the pattern and its
    /// count arrive together and leave together".
    ///
    /// A message outranks the echo. A message is news and an echo is a
    /// reminder, and the row is not wide enough for both to matter.
    pub(super) fn search_row_text(&self) -> Option<String> {
        if let Some(buf) = &self.command_buffer {
            let CommandLineKind::Search { dir, find } = self.command_kind else {
                return None;
            };
            return Some(format!("{}{buf}", search_prefix(dir, find)));
        }
        if !self.message.is_empty() {
            return None;
        }
        let (dir, pattern) = self.search_echo.as_ref()?;
        // Not `find`: an accepted find is a committed search like any
        // other (spec 0276 S6), so what it leaves behind is spelled the
        // way `n`/`N` will repeat it.
        Some(format!("{}{pattern}", search_prefix(*dir, false)))
    }

    pub(super) fn search_tally_text(&self) -> Option<String> {
        // Spec 0278 S2: the count is a satellite of the pattern, not of
        // the highlight — it is drawn beside the search text and never
        // left alone on a row that has moved on to something else.
        self.search_row_text()?;
        let tally = self.search_tally.as_ref()?;
        let total = tally.total?;
        let scope = self.search_sweep.as_ref()?.origin.scope;
        if scope != tally.scope
            || self.structural_version != tally.version
            || self.tally_pattern_text(scope).as_deref() != Some(tally.text.as_str())
        {
            return None;
        }
        Some(match tally.ordinal {
            Some(ordinal) => format!("{ordinal} of {total}"),
            None => format!("? of {total}"),
        })
    }

    /// Spec 0235 S8: the whole of a sweep's effect on the pane. It
    /// unfolds what hides its match and asks the next frame to bring it
    /// into view; it does not move the cursor, and so records no
    /// jumplist entry.
    fn show_sweep_hit(&mut self, hit: SweepHit) {
        match hit.at {
            SweepCursor::Line(pos) => self.unfold_ancestors(pos.node),
            // Spec 0276 S9: a side pane tints nothing, so its current
            // match is shown by the highlight or not at all — leave it
            // where it is and a find's `Enter` steps something invisible.
            // Only a find previews: a `/` prompt there can still be
            // abandoned, and this move is not undone.
            SweepCursor::Index(_) => {
                if let CommandLineKind::Search { find: true, .. } = self.command_kind {
                    self.apply_sweep_hit(self.search_scope(), hit);
                }
            }
        }
        self.search_center = true;
    }

    /// Spec 0235 S9: move the cursor (or the side pane's highlight) to a
    /// match. The only place either moves for a search, and so the only
    /// place `record_jump` fires.
    fn apply_sweep_hit(&mut self, scope: SearchScope, hit: SweepHit) {
        match (scope, hit.at) {
            (SearchScope::Main, SweepCursor::Line(pos)) => {
                if pos.node != self.cursor {
                    self.record_jump();
                    self.set_cursor(pos.node);
                    self.unfold_ancestors(pos.node);
                }
                // Spec 0216 S7: a flat node can own several rows — a
                // packed run's elements — so landing on the node is not
                // yet landing on the match. `set_cursor` has just reset
                // this to 0.
                self.cursor_line_in_node = pos.line_in_node;
                if hit.on_path {
                    // Spec 0235 S20: the path is not on screen, so it
                    // has no column of its own to land on; the row it
                    // names is what the match is about.
                    self.reset_caret_column();
                } else {
                    // Clamped, since a match inside the row's own
                    // indentation is left of the leftmost reachable
                    // column.
                    self.cursor_column = hit.column;
                    self.clamp_caret_column();
                    self.desired_column = self.cursor_column;
                    // Spec 0199 S10: a text match at an end of the row
                    // is a coincidence, so it must not arm `h`'s fold or
                    // `l`'s descent.
                    self.caret_anchor = CaretAnchor::Free;
                    self.select_sweep_hit(hit);
                }
            }
            (SearchScope::Override, SweepCursor::Index(i)) => {
                self.override_highlight = i;
                // Preview the landing row, as arrow-key movement does.
                self.preview_override_highlight();
            }
            (SearchScope::Manage, SweepCursor::Index(i)) => self.set_manage_highlight(i),
            _ => {}
        }
    }

    /// Spec 0274 S12: leave a match that crosses a row selected.
    ///
    /// The caret has just landed on the match's first character and
    /// **stays there**; the anchor goes to its last. Spec 0242 makes the
    /// caret the selection's moving end, so this is the model used
    /// rather than worked around — the renderer, `Ctrl-c` and
    /// `selected_columns` learn nothing, and `selection_span` orders the
    /// two ends for itself.
    ///
    /// Which end holds the caret is not a free choice, though, and the
    /// obvious one is wrong. A search leaves the caret **where the match
    /// begins**, whatever engine found it, because spec 0246 S3 splits
    /// the origin at the caret and takes the two halves to partition it:
    /// park the caret at the match's *end* and the match the reader is
    /// standing on still starts before the split, so a backward search
    /// finds it again, and `N` after `/` never moves.
    ///
    /// **The anchor sits on the last character, not one past it**: 0242
    /// S1's block caret rests *on* a cell, and a span whose end column
    /// is exclusive is one past the cell the reader can see.
    ///
    /// A single-row hit selects nothing. Its behavior is spec 0246's and
    /// does not change; a selection there would be a cue the reader did
    /// not ask for, over text one glance already takes in.
    fn select_sweep_hit(&mut self, hit: SweepHit) {
        let Some((end, column)) = hit.end else {
            return;
        };
        self.select_anchor = Some(CursorPos {
            node: end.node,
            line_in_node: end.line_in_node,
            column: column.saturating_sub(1),
        });
        self.select_engaged = true;
    }

    /// The pane's own last committed `(direction, pattern)`.
    fn last_search_for(&self, scope: SearchScope) -> Option<&(SearchDir, String)> {
        match scope {
            SearchScope::Main => self.last_search.as_ref(),
            SearchScope::Override => self.last_override_search.as_ref(),
            SearchScope::Manage => self.last_manage_search.as_ref(),
        }
    }

    fn set_last_search_for(&mut self, scope: SearchScope, last: (SearchDir, String)) {
        *match scope {
            SearchScope::Main => &mut self.last_search,
            SearchScope::Override => &mut self.last_override_search,
            SearchScope::Manage => &mut self.last_manage_search,
        } = Some(last);
    }

    /// Spec 0276 S2: `F`/`B` — a search prompt that steps rather than
    /// commits, pre-filled with the focused pane's last pattern.
    ///
    /// The sweep starts here rather than at the first edit, which is the
    /// one thing `open_command_line` cannot do for it: `/` opens on an
    /// empty buffer and has nothing to look for, while a find opens
    /// already owing an answer.
    pub(super) fn open_find(&mut self, dir: SearchDir) {
        let prefill = self
            .last_search_for(self.search_scope())
            .map(|(_, pattern)| pattern.clone())
            .unwrap_or_default();
        self.open_command_line(CommandLineKind::Search { dir, find: true }, prefill);
        self.restart_search_sweep();
    }

    /// Spec 0276 S5: `Esc` at a find prompt accepts the match on screen.
    ///
    /// The displayed hit is applied, rather than the pattern being
    /// searched again: `Enter` may have rotated several matches past the
    /// first, and a fresh sweep from the prompt's origin would land back
    /// on that first one.
    ///
    /// Nothing displayed is nothing to accept (S7) — that is `/`'s `Esc`
    /// unchanged, view restored and position untouched.
    pub(super) fn accept_find(&mut self, dir: SearchDir) {
        let pattern = self.command_buffer.take().unwrap_or_default();
        self.command_cursor = 0;
        self.search_browse = None;
        let scope = self
            .search_origin
            .map_or_else(|| self.search_scope(), |origin| origin.scope);
        let Some(hit) = self.search_sweep.as_ref().and_then(|sweep| sweep.found) else {
            self.cancel_search();
            return;
        };
        // Accepted, so the origin is not restored — but it must still be
        // dropped, or the next `Esc` would put this view back.
        self.search_origin = None;
        // Spec 0276 S6: an accepted find is a committed search.
        self.push_search_history(&pattern);
        self.echo_search(dir, &pattern);
        self.set_last_search_for(scope, (dir, pattern));
        self.apply_sweep_hit(scope, hit);
        // Spec 0276 N3, amended: `apply_sweep_hit` has left the caret on
        // the match's first character, which is where an accepted find
        // now leaves it — the same landing a `/` commit makes. All that
        // remains of S5's second half is refusing the selection spec
        // 0274 S12 leaves behind for that commit: a find keeps its
        // highlight, so the match's extent is already on screen and a
        // selection would be a cue nobody asked for.
        if scope == SearchScope::Main {
            self.clear_selection();
        }
    }

    /// Spec 0235 S6: open a `/`/`?` prompt's search state — the origin
    /// every keystroke will search again from, and the highlight that
    /// outlives the prompt (S15).
    pub(super) fn start_search_prompt(&mut self) {
        self.search_origin = Some(self.search_origin_for(self.search_scope()));
        self.search_sweep = None;
        self.search_highlight = true;
        self.search_dirty = true;
        // Spec 0246 S16: no browse survives a prompt.
        self.search_browse = None;
    }

    /// Spec 0246 S17/S18: show the next match in `dir` — absolute
    /// document directions, whichever way the prompt itself points.
    ///
    /// A fresh sweep from where the displayed match sits, which S2's
    /// two-visit walk then steps off and cycles back to, so a
    /// single-match pattern rotates to itself. `search_origin` is
    /// deliberately untouched (S19): it is still what `Esc` restores and
    /// what the next edit re-searches from.
    pub(super) fn rotate_search_match(&mut self, dir: SearchDir) {
        let Some(origin) = self.search_origin else {
            return;
        };
        // Spec 0246 S21: nothing displayed — the sweep is still walking,
        // or it finished having missed — is nothing to rotate from.
        //
        // Spec 0274 S16 relaxes the second half of that. A *provisional*
        // miss is precisely the state in which asking again is the point:
        // the frozen queue saw the document the bake had managed at the
        // time, and the reader pressing the key is asking about the
        // document as it stands now. It rotates from `search_origin`,
        // there being no displayed match to step off. A *conclusive*
        // miss and a still-walking sweep are still no-ops.
        // Spec 0277 S6: a rotation is built from the displayed hit by
        // construction, so it carries that hit's ordinal whenever there
        // is one. This is the find prompt's `Enter` (spec 0276 S4) and
        // `Ctrl-←`/`Ctrl-→`.
        let (from, ordinal) = match self.search_sweep.as_ref().map(|s| (s.found, s.provisional)) {
            Some((Some(hit), _)) => (
                SearchOrigin {
                    at: hit.at,
                    column: hit.start,
                    ..origin
                },
                self.departing_ordinal(hit.at, hit.start),
            ),
            // `provisional` is set only where the walk ran out, so this
            // arm cannot catch a sweep that is still going.
            Some((None, true)) => (origin, None),
            _ => return,
        };
        let pattern = self.command_buffer.clone().unwrap_or_default();
        let Some(mut sweep) = self.begin_search_sweep(&pattern, dir, from) else {
            return;
        };
        sweep.from_ordinal = ordinal;
        // Spec 0246 S22: walked by `run_loop`'s idle arm like any other,
        // so a rotation over a large document does not block the key.
        self.search_sweep = Some(sweep);
        self.search_dirty = true;
    }

    /// Spec 0246 S12: remember a committed pattern. A repeat moves to
    /// the end rather than being stored twice — a history of one pattern
    /// typed five times is a history of one pattern.
    fn push_search_history(&mut self, pattern: &str) {
        if pattern.is_empty() {
            return;
        }
        if let Some(i) = self.search_history.iter().position(|p| p == pattern) {
            self.search_history.remove(i);
        }
        self.search_history.push(pattern.to_string());
        if self.search_history.len() > SEARCH_HISTORY_MAX {
            self.search_history.remove(0);
        }
    }

    /// Spec 0246 S14/S15: `Up` (`back`) and `Down` through
    /// `search_history`.
    ///
    /// Neither end wraps, and neither reports why it did nothing: a
    /// prompt row is the pattern's, not a place for a message.
    pub(super) fn browse_search_history(&mut self, back: bool) {
        let next = match (&self.search_browse, back) {
            (None, true) if self.search_history.is_empty() => return,
            (None, true) => Some(self.search_history.len() - 1),
            (None, false) => return,
            (Some(b), true) if b.index == 0 => return,
            (Some(b), true) => Some(b.index - 1),
            // Past the newest entry is back to the user's own text.
            (Some(b), false) if b.index + 1 >= self.search_history.len() => None,
            (Some(b), false) => Some(b.index + 1),
        };
        let draft = match self.search_browse.take() {
            Some(b) => b.draft,
            None => self.command_buffer.clone().unwrap_or_default(),
        };
        let text = match next {
            Some(i) => self.search_history[i].clone(),
            None => draft.clone(),
        };
        self.command_cursor = text.chars().count();
        self.command_buffer = Some(text);
        // Spec 0246 S15: a recall *is* a pattern change, so the sweep
        // restarts from the prompt's origin like any edit — and since
        // that clears the browse (S16), the new state is set after it.
        self.restart_search_sweep();
        self.search_browse = next.map(|index| SearchBrowse { index, draft });
    }

    /// Spec 0235 S7: any change to the pattern replaces the sweep —
    /// unconditionally, with no exception for "the pattern only grew".
    /// That symmetry is the entire reason `Backspace` reads as an undo.
    pub(super) fn restart_search_sweep(&mut self) {
        // Spec 0246 S16: the one place the browse ends, and it is on the
        // path of every editing key — so any edit ends it.
        self.search_browse = None;
        let (Some(origin), CommandLineKind::Search { dir, .. }) =
            (self.search_origin, self.command_kind)
        else {
            return;
        };
        self.restore_search_origin(origin);
        let pattern = self.command_buffer.clone().unwrap_or_default();
        self.search_sweep = self.begin_search_sweep(&pattern, dir, origin);
        self.search_dirty = true;
    }

    /// Spec 0235 S9: `Enter`. An unfinished sweep is run to completion
    /// first — the user has stopped typing and asked for the answer, so
    /// the wait is now the answer's cost rather than a keystroke's.
    ///
    /// A sweep is live exactly when the buffer held a pattern; the vim
    /// convention of an empty `/` re-using the last one therefore
    /// arrives here with none, and gets a fresh walk from the same
    /// origin.
    pub(super) fn commit_search(&mut self, dir: SearchDir, pattern: &str) {
        // Spec 0246 S12: here rather than at the three `Enter` branches,
        // so that every committed pattern is remembered once and `n`/`N`
        // — which repeat rather than commit — remember nothing.
        self.push_search_history(pattern);
        self.search_browse = None;
        let origin = self
            .search_origin
            .take()
            .unwrap_or_else(|| self.search_origin_for(self.search_scope()));
        let sweep = match self.search_sweep.take() {
            Some(sweep) => Some(sweep),
            None => self.begin_search_sweep(pattern, dir, origin),
        };
        let Some(mut sweep) = sweep else {
            self.report_bad_pattern(pattern);
            return;
        };
        self.drain_sweep(&mut sweep);
        self.search_dirty = true;
        let found = sweep.found;
        // Kept rather than dropped: S15's highlight outlives the
        // prompt, and `found` is what tells the strong tint from the
        // muted one.
        self.search_sweep = Some(sweep);
        self.track_search_tally();
        match found {
            Some(hit) => {
                self.apply_sweep_hit(origin.scope, hit);
                self.echo_search(dir, pattern);
            }
            // Spec 0235 S10: the message returns at `Enter`, and only
            // here — while the prompt was open it shared that row.
            None => self.message = self.not_found(pattern, origin.scope),
        }
    }

    /// Spec 0278 S1: leave the pattern on the command row, the way the
    /// prompt that was just closed had it.
    ///
    /// A hit only. A miss has `not_found` to say, which is the same
    /// fact told at more length, and spec 0277 N3 already refuses the
    /// count that would sit beside it.
    fn echo_search(&mut self, dir: SearchDir, pattern: &str) {
        if !pattern.is_empty() {
            self.search_echo = Some((dir, pattern.to_string()));
        }
    }

    /// Spec 0273 S8: why a committed pattern started no sweep, when the
    /// reason is the pattern itself.
    ///
    /// Only here and at the other non-incremental caller — while the
    /// prompt is open the reader is mid-word, and a diagnostic per
    /// keystroke would be shouting at a half-typed `foo(`. An empty
    /// pattern and an empty pane also reach this, and both compile, so
    /// both stay silent.
    fn report_bad_pattern(&mut self, pattern: &str) {
        if let Err(why) = SearchPattern::new(pattern) {
            self.message = format!("bad pattern: {why}");
        }
    }

    /// Whether a miss is entitled to be reported as one.
    ///
    /// A folded region has no text, so the sweep never looked at it —
    /// while the bake still owes bodies, "not found" is a claim about
    /// the document the search was allowed to see and not about the
    /// document. The two side panes search lists that have nothing to
    /// do with the blob and are never qualified.
    ///
    /// One predicate for both halves of the report: the message
    /// [`App::not_found`] writes at `Enter`, and the color the prompt
    /// tints the pattern while it is still open.
    pub(super) fn search_miss_is_conclusive(&self, scope: SearchScope) -> bool {
        scope != SearchScope::Main || self.auto_folded.is_empty()
    }

    /// Spec 0249 S13: "not found" is a claim about the whole document,
    /// and during a bake it is not one the search can make.
    ///
    /// A folded region has no text, so the sweep never looked at it. The
    /// caveat rides on the *miss* and not on a hit: a hit is a real row
    /// at a real position and needs no qualification, while a miss is
    /// the one answer an unbaked remainder can falsify.
    ///
    /// The count is of subtrees rather than rows because that is what is
    /// known — a stop's body has not been rendered, so nobody can say
    /// how many rows it stands for. Only the main pane is qualified;
    /// the two side panes search lists that have nothing to do with the
    /// document.
    pub(super) fn not_found(&self, pattern: &str, scope: SearchScope) -> String {
        let base = format!("pattern not found: {pattern}");
        if self.search_miss_is_conclusive(scope) {
            return base;
        }
        let n = self.auto_folded.len();
        let plural = if n == 1 { "subtree" } else { "subtrees" };
        format!("{base} ({n} {plural} not yet baked)")
    }

    /// `n`/`N`, and every non-incremental caller: one sweep of `scope`,
    /// run to the end, applied.
    pub(super) fn run_search(&mut self, scope: SearchScope, dir: SearchDir, pattern: &str) {
        let origin = self.search_origin_for(scope);
        // Spec 0277 S6: `n`/`N` sweep from the caret, so they leave the
        // displayed match exactly when the caret is standing in it —
        // read before the sweep that displayed it is dropped.
        let ordinal = self.caret_ordinal(scope);
        self.search_origin = None;
        self.search_sweep = None;
        let Some(mut sweep) = self.begin_search_sweep(pattern, dir, origin) else {
            self.report_bad_pattern(pattern);
            return;
        };
        sweep.from_ordinal = ordinal;
        self.drain_sweep(&mut sweep);
        self.search_dirty = true;
        self.search_highlight = true;
        let found = sweep.found;
        self.search_sweep = Some(sweep);
        self.track_search_tally();
        match found {
            Some(hit) => {
                self.apply_sweep_hit(scope, hit);
                self.echo_search(dir, pattern);
            }
            None => self.message = self.not_found(pattern, scope),
        }
    }

    /// Spec 0235 S11: `Esc` (and `Backspace` on an empty buffer, which
    /// vim treats the same way) drops the sweep and puts the view back.
    pub(super) fn cancel_search(&mut self) {
        if let Some(origin) = self.search_origin.take() {
            self.restore_search_origin(origin);
        }
        // Spec 0246 S16.
        self.search_browse = None;
        self.clear_search_highlight();
    }

    /// Spec 0235 S15/N5: `Esc` outside the prompt — vim's
    /// `:nohlsearch`, with no view to restore because there was no
    /// prompt to open one.
    pub(super) fn clear_search_highlight(&mut self) {
        self.search_sweep = None;
        self.search_highlight = false;
        self.search_dirty = true;
        // Spec 0277 S12: a tally exists only while the matches are
        // tinted, and this is where they stop being.
        self.search_tally = None;
    }

    /// Spec 0235 S15: the pattern this frame tints, or `None` when
    /// nothing is highlighted.
    ///
    /// The live `command_buffer` while a `/`/`?` prompt is open — that
    /// is what makes the highlight track the typing — and the focused
    /// pane's `last_*_search` once it has closed.
    pub(super) fn search_highlight_pattern(&self) -> Option<Rc<SearchPattern>> {
        if !self.search_highlight {
            return None;
        }
        if let (Some(buf), CommandLineKind::Search { .. }) =
            (&self.command_buffer, self.command_kind)
        {
            return (!buf.is_empty())
                .then(|| self.compiled_pattern(buf))
                .flatten();
        }
        let last = match self.search_scope() {
            SearchScope::Main => &self.last_search,
            SearchScope::Override => &self.last_override_search,
            SearchScope::Manage => &self.last_manage_search,
        };
        let pattern = last.as_ref().map(|(_, p)| p.clone())?;
        self.compiled_pattern(&pattern)
    }

    /// Spec 0273 S10: `text` compiled, reusing the last compile when the
    /// text has not changed.
    ///
    /// The caller is `render`, once a frame, asking about the same
    /// pattern until the reader types — and after spec 0273 a compile is
    /// a real cost rather than a `String` clone. `None` for a pattern
    /// that does not compile, which S8 makes silent while the prompt is
    /// open.
    fn compiled_pattern(&self, text: &str) -> Option<Rc<SearchPattern>> {
        let mut slot = self.search_compiled.borrow_mut();
        if let Some((cached, pattern)) = slot.as_ref() {
            if cached == text {
                return Some(Rc::clone(pattern));
            }
        }
        let pattern = Rc::new(SearchPattern::new(text).ok()?);
        *slot = Some((text.to_string(), Rc::clone(&pattern)));
        Some(pattern)
    }

    /// Spec 0235 S14/S22: the one match drawn in `search_current_style`,
    /// as a document line, a caret-track column, its width in columns,
    /// and whether it matched the row's path rather than its text.
    ///
    /// `None` in a side pane, whose matches are whole rows.
    pub(super) fn search_current_cell(&self) -> Option<(usize, usize, usize, bool)> {
        let hit = self.search_sweep.as_ref()?.found?;
        let SweepCursor::Line(pos) = hit.at else {
            return None;
        };
        let line = self.absolute_start(pos.node) + pos.line_in_node as usize;
        Some((line, hit.column, hit.width, hit.on_path))
    }

    /// Spec 0274 S13: the current hit's whole extent, shaped as a
    /// selection is — two absolute lines and two columns, the end
    /// exclusive — so that `selected_columns` cuts it per row.
    ///
    /// This is what a pattern that may cross a row tints in
    /// `search_current_style`, in place of `render`'s occurrence loop —
    /// that loop is a per-row construction, and it cannot describe a
    /// hit two rows tall. A single-row hit of such a pattern comes back
    /// here too, so the current hit is drawn by one rule either way.
    ///
    /// The *other* occurrences are `multi_row_occurrences`'s answer.
    pub(super) fn search_hit_span(&self) -> Option<SelectionSpan> {
        let hit = self.search_sweep.as_ref()?.found?;
        let SweepCursor::Line(pos) = hit.at else {
            return None;
        };
        let lo = self.absolute_start(pos.node) + pos.line_in_node as usize;
        match hit.end {
            Some((end, column)) => Some((
                lo,
                hit.column,
                self.absolute_start(end.node) + end.line_in_node as usize,
                column,
            )),
            None => Some((lo, hit.column, lo, hit.column + hit.width)),
        }
    }

    /// Spec 0274 S13: every occurrence of a cross-row pattern the drawn
    /// window can be shown to hold, as character columns per window
    /// row.
    ///
    /// Single-row search tints every occurrence on screen, not only the
    /// one the caret is on, and a cross-row pattern has no reason to
    /// behave differently. It cannot use `render`'s per-row loop,
    /// though: a match two rows tall is not a range of any one row's
    /// text.
    ///
    /// The window is not one haystack either. Two rows drawn one above
    /// the other are a `\n` apart in the document only when they are
    /// consecutive there as well, so the window is cut into runs of
    /// document-adjacent rows and each run is matched on its own. That
    /// is the same cut the sweep makes into segments, reached from the
    /// drawn side.
    ///
    /// The first run reaches `SEARCH_HIGHLIGHT_LEAD` rows above the
    /// window, so that a match beginning just off the top still tints
    /// the part of itself that is on screen. Those extra rows carry no
    /// tint of their own and are dropped before returning. A match
    /// beginning further up than that is still missed — bounding the
    /// scan is what keeps it a per-frame cost at all (spec 0272).
    pub(super) fn multi_row_occurrences(
        &self,
        pattern: &SearchPattern,
        first_row: usize,
        window: &[DisplayRow],
    ) -> Vec<Vec<Range<usize>>> {
        let lead = first_row.min(SEARCH_HIGHLIGHT_LEAD);
        let mut rows = self.build_window(first_row - lead, lead);
        rows.extend_from_slice(window);
        let mut out = vec![Vec::new(); rows.len()];
        let mut start = 0;
        while start < rows.len() {
            let mut end = start + 1;
            while end < rows.len() && self.rows_are_adjacent(rows[end - 1], rows[end]) {
                end += 1;
            }
            self.scan_row_run(pattern, &rows[start..end], &mut out[start..end]);
            start = end;
        }
        out.drain(..lead);
        out
    }

    /// Whether two rows drawn one above the other are also one line
    /// apart in the document, so that the `\n` between them is one a
    /// pattern may match.
    ///
    /// A user fold shows up as a gap in the numbering all by itself,
    /// since an absolute line counts the rows a fold hides. An unbaked
    /// stop does not: it draws its header and its footer on consecutive
    /// lines with its whole subtree still missing between them, so that
    /// seam has to be named. It is the seam the sweep starts a new
    /// segment at.
    fn rows_are_adjacent(&self, prev: DisplayRow, next: DisplayRow) -> bool {
        let (DisplayRow::Committed(prev), DisplayRow::Committed(next)) = (prev, next) else {
            return false;
        };
        if next.line != prev.line + 1 {
            return false;
        }
        // `prev` is an unbaked stop's header: its subtree is missing
        // between it and the footer drawn underneath it.
        !self.auto_folded.contains(&prev.pos.node) || self.is_footer(prev.pos)
    }

    /// One run of document-adjacent rows, joined by the `\n` that
    /// separates them, matched, and each match cut back into the rows it
    /// covers.
    fn scan_row_run(
        &self,
        pattern: &SearchPattern,
        rows: &[DisplayRow],
        out: &mut [Vec<Range<usize>>],
    ) {
        let mut haystack = String::new();
        let mut starts: Vec<usize> = Vec::with_capacity(rows.len());
        for &row in rows {
            if !starts.is_empty() {
                haystack.push('\n');
            }
            starts.push(haystack.len());
            haystack.push_str(&self.row_text(row));
        }
        let mut at = 0;
        while at <= haystack.len() {
            let Some(found) = pattern.find_range_from(&haystack, at) else {
                break;
            };
            // Stepping past the match's *start* rather than its end,
            // as the single-row loop does, is what keeps overlapping
            // occurrences honest.
            at = found.start
                + haystack[found.start..]
                    .chars()
                    .next()
                    .map_or(1, char::len_utf8);
            for (i, &row_start) in starts.iter().enumerate() {
                // One before the next row's start is that row's `\n`.
                let row_end = starts.get(i + 1).map_or(haystack.len(), |&next| next - 1);
                let lo = found.start.max(row_start);
                let hi = found.end.min(row_end);
                if lo >= hi {
                    continue;
                }
                let text = &haystack[row_start..row_end];
                out[i].push(
                    text[..lo - row_start].chars().count()..text[..hi - row_start].chars().count(),
                );
            }
        }
    }

    /// Spec 0235 S12/S13: bring the live sweep's match into view, once,
    /// from `render` — the only place the pane's height and width are
    /// known.
    ///
    /// Not `clamp_scroll_to_visible`'s minimum nudge: an axis the match
    /// already fits in does not move at all, and an axis it does not
    /// centers it. That pair is what makes a walk across nearby matches
    /// readable — a minimum nudge would put every match on the same
    /// edge row, and an unconditional recenter would make the text swim
    /// under matches that were already on screen.
    pub(super) fn center_search_match(&mut self, pane: Rect) {
        if !std::mem::take(&mut self.search_center) {
            return;
        }
        let Some(hit) = self.search_sweep.as_ref().and_then(|s| s.found) else {
            return;
        };
        match hit.at {
            SweepCursor::Line(pos) => {
                let line = self.absolute_start(pos.node) + pos.line_in_node as usize;
                let Some(row) = self.visible_row_of_line(line) else {
                    return;
                };
                self.center_row(row, pane.height as isize);
                self.center_columns(hit.column, hit.width, pane.width as usize);
            }
            SweepCursor::Index(i) => {
                let (scroll, height) = match self.search_scope() {
                    SearchScope::Override => (&mut self.override_scroll, self.override_list_height),
                    _ => (&mut self.manage_scroll, self.manage_list_height),
                };
                let top = scroll.top(&FLAT_ROWS);
                let i = i as isize;
                if height > 0 && (i < top || i >= top + height as isize) {
                    // Never above the first row: centering brings a match
                    // into view, and blank rows above it would be room
                    // spent on nothing.
                    scroll.set_top((i - (height / 2) as isize).max(0), &FLAT_ROWS);
                }
            }
        }
    }

    /// The vertical half of S12, in terminal rows so that a wire row
    /// (spec 0225 S8) counts as the half of a line it is.
    fn center_row(&mut self, row: usize, pane_height: isize) {
        let heights = self.row_heights();
        let line = heights.height(row) as isize;
        if pane_height < line {
            return;
        }
        let pos = heights.offset(row) as isize;
        let top = self.scroll_top();
        if pos >= top && pos + line <= top + pane_height {
            return;
        }
        let last = (heights.offset(self.composed_row_count()) as isize - pane_height).max(0);
        self.set_scroll_top((pos - (pane_height - line) / 2).clamp(0, last));
    }

    /// The horizontal half of S12. A match wider than the pane centers
    /// on its start rather than on a midpoint that would leave the
    /// match's beginning off screen.
    fn center_columns(&mut self, column: usize, width: usize, pane_width: usize) {
        let usable = pane_width.saturating_sub(render::FOLD_FIELD_WIDTH);
        if usable == 0 {
            return;
        }
        if column >= self.pan_offset && column + width <= self.pan_offset + usable {
            return;
        }
        let target = column.saturating_sub(usable.saturating_sub(width) / 2);
        self.pan_offset = target.min(self.max_pan_offset());
    }

    /// Spec 0235 S5: whether a sweep's *result* changed since the last
    /// frame — not whether it did work, which happens hundreds of times
    /// a second and would draw as often.
    pub(super) fn take_search_dirty(&mut self) -> bool {
        std::mem::take(&mut self.search_dirty)
    }

    /// [`Self::take_search_dirty`] without taking it — what `run_loop`
    /// asks before it decides to sleep, since a frame already owed is a
    /// reason not to.
    pub(super) fn search_frame_owed(&self) -> bool {
        self.search_dirty
    }
}

/// Which of the origin row's two visits (spec 0246 S2) a bound is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Visit {
    /// The first candidate of the walk.
    Out,
    /// The last one, after the wrap.
    Back,
}

/// Spec 0276 S3: the character that names a search on the command row.
///
/// Punctuation rather than the `F`/`B` that opened a find: the buffer
/// arrives pre-filled, so a letter prefix would render `Ffoo` and read
/// as a typo.
fn search_prefix(dir: SearchDir, find: bool) -> char {
    match (dir, find) {
        (SearchDir::Forward, false) => '/',
        (SearchDir::Backward, false) => '?',
        (SearchDir::Forward, true) => '>',
        (SearchDir::Backward, true) => '<',
    }
}

/// Spec 0246 S3: the origin row's half, split at the caret.
///
/// The two halves are complementary, which is the property the whole
/// scheme rests on: every match of the row falls in exactly one of them,
/// so a full cycle sees each once. Strictness at the caret is what makes
/// `n` leave the match it is standing on.
///
/// The exclusive bounds are stated as `column + 1` rather than as the
/// next character boundary: match starts are boundaries, so `start >=
/// c + 1` is `start > c` and `start < c + 1` is `start <= c`, and
/// `find_range_from` rounds a mid-character `lo` up for itself.
fn origin_bound(origin: SearchOrigin, dir: SearchDir, visit: Visit) -> RowBound {
    match (origin.at, dir, visit) {
        // Spec 0246 N4: a side pane counts entries, not matches.
        (SweepCursor::Index(_), _, Visit::Out) => RowBound::Nothing,
        (SweepCursor::Index(_), _, Visit::Back) => RowBound::Whole,
        _ => split_at(origin.column, dir, visit),
    }
}

/// The two complementary halves of a haystack split at `c`, which spec
/// 0274 S14 asks for over a segment's bytes in exactly the shape 0246
/// S3 asks for over a row's.
fn split_at(c: usize, dir: SearchDir, visit: Visit) -> RowBound {
    match (dir, visit) {
        (SearchDir::Forward, Visit::Out) => RowBound::Starts {
            lo: c + 1,
            hi: usize::MAX,
        },
        (SearchDir::Forward, Visit::Back) => RowBound::Starts { lo: 0, hi: c + 1 },
        (SearchDir::Backward, Visit::Out) => RowBound::Starts { lo: 0, hi: c },
        (SearchDir::Backward, Visit::Back) => RowBound::Starts {
            lo: c,
            hi: usize::MAX,
        },
    }
}

/// The byte window a [`RowBound`] names inside a segment, or `None` for
/// the bound that admits nothing.
///
/// Spec 0246 S4's two halves, read over a segment rather than a row.
/// Unlike `pick_match` the comparison is against the match's *true*
/// offset rather than 0246 S3a's floored one — the floor is a row's, and
/// a segment holds many. The two halves still partition the segment,
/// which is the only property the wrap rests on.
fn bound_span(bound: RowBound) -> Option<(usize, usize)> {
    match bound {
        RowBound::Nothing => None,
        RowBound::Whole => Some((0, usize::MAX)),
        RowBound::Starts { lo, hi } => Some((lo, hi)),
    }
}

/// Spec 0246 S4: the match a candidate stops at — the first eligible
/// start going forward, the **last** going backward.
///
/// Taking the last is not symmetry for its own sake: a backward search
/// arriving at a row must land on its rightmost match, or it lands right
/// of where the user is looking. It costs a scan of the row's matches,
/// which the forward direction does not pay.
///
/// Spec 0246 S5: the scan resumes one character past a match's *start*,
/// not past its end, so overlapping matches (`aa` in `aaa`) are separate
/// stops — vim's rule, and the only one under which S3's two halves
/// partition the row.
///
/// Spec 0246 S3a: a stop is placed at `max(start, floor)`, `floor` being
/// the row's first non-blank. The bounds come from a caret, and spec
/// 0194 S3 keeps a caret out of the indentation, so a match reaching
/// back into it has to be compared at the column the caret will actually
/// occupy — otherwise the row splits *ahead* of the stop the search just
/// landed on, and the next `n` finds it again, forever. The cost is that
/// several matches inside one indentation collapse to a single stop,
/// which is the same thing the caret does to them.
fn pick_match(
    pattern: &SearchPattern,
    haystack: &str,
    floor: usize,
    dir: SearchDir,
    bound: RowBound,
) -> Option<Range<usize>> {
    let (lo, hi) = match bound {
        RowBound::Nothing => return None,
        RowBound::Whole => (0, usize::MAX),
        RowBound::Starts { lo, hi } => (lo, hi),
    };
    // Where the lower bound first admits something. `max(start, floor)`
    // rises with `start`, so below this offset nothing can qualify and
    // above it everything does.
    let from = if floor >= lo { 0 } else { lo };
    if dir == SearchDir::Forward {
        return pattern
            .find_range_from(haystack, from)
            .filter(|r| r.start.max(floor) < hi);
    }
    let mut best = None;
    let mut from = from;
    // Spec 0273 S6 lets a pattern match nothing at all (`^`, `x*`), so
    // the scan is bounded by the haystack as well as by the matches:
    // `from` steps past a match's start by at least one byte, and an
    // empty match at the end would otherwise be re-found forever —
    // `find_range_from` clamps `from` back down to the length.
    while from <= haystack.len() {
        let Some(range) = pattern.find_range_from(haystack, from) else {
            break;
        };
        if range.start.max(floor) >= hi {
            break;
        }
        from = range.start
            + haystack[range.start..]
                .chars()
                .next()
                .map_or(1, char::len_utf8);
        best = Some(range);
    }
    best
}
