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
}

impl SearchSweep {
    /// Whether the walk is over. A finished sweep is *kept* rather than
    /// dropped: the prompt still has to draw its answer, and `Enter`
    /// still has to commit it.
    pub(super) fn is_finished(&self) -> bool {
        self.at.is_none() && self.segments.is_empty() && self.scan.is_none()
    }
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
    /// Spec 0274 S11's extent lands in a later stage; for now the hit is
    /// reported over the part of its first row that it covers, which is
    /// the whole of it whenever the match does not cross a row.
    fn hit_from_span(&self, seg: Segment, span: Range<usize>) -> Option<SweepHit> {
        let (pos, row) = self.locate_in_segment(seg, span.start)?;
        let text = self.line_text(pos);
        let start = span.start - row.start;
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
        })
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
            if let Some(hit) = hit {
                self.show_sweep_hit(hit);
            }
        }
        self.search_dirty |= found_changed || exhausted;
        step
    }

    /// Spec 0235 S8: the whole of a sweep's effect on the pane. It
    /// unfolds what hides its match and asks the next frame to bring it
    /// into view; it does not move the cursor, and so records no
    /// jumplist entry.
    fn show_sweep_hit(&mut self, hit: SweepHit) {
        if let SweepCursor::Line(pos) = hit.at {
            self.unfold_ancestors(pos.node);
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
        let from = match self.search_sweep.as_ref().map(|s| (s.found, s.provisional)) {
            Some((Some(hit), _)) => SearchOrigin {
                at: hit.at,
                column: hit.start,
                ..origin
            },
            // `provisional` is set only where the walk ran out, so this
            // arm cannot catch a sweep that is still going.
            Some((None, true)) => origin,
            _ => return,
        };
        let pattern = self.command_buffer.clone().unwrap_or_default();
        let Some(sweep) = self.begin_search_sweep(&pattern, dir, from) else {
            return;
        };
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
        let (Some(origin), CommandLineKind::Search(dir)) = (self.search_origin, self.command_kind)
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
        match found {
            Some(hit) => self.apply_sweep_hit(origin.scope, hit),
            // Spec 0235 S10: the message returns at `Enter`, and only
            // here — while the prompt was open it shared that row.
            None => self.message = self.not_found(pattern, origin.scope),
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
        self.search_origin = None;
        self.search_sweep = None;
        let Some(mut sweep) = self.begin_search_sweep(pattern, dir, origin) else {
            self.report_bad_pattern(pattern);
            return;
        };
        self.drain_sweep(&mut sweep);
        self.search_dirty = true;
        self.search_highlight = true;
        let found = sweep.found;
        self.search_sweep = Some(sweep);
        match found {
            Some(hit) => self.apply_sweep_hit(scope, hit),
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
        if let (Some(buf), CommandLineKind::Search(_)) = (&self.command_buffer, self.command_kind) {
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
