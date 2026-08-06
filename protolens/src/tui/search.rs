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

use super::navigation::PathScratch;
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

/// What one `search_sweep_step` did — deliberately the same shape as
/// `PrefetchStep`, since `run_loop` treats the two the same way:
/// `Progressed` means do not sleep yet, `Idle` means there is nothing
/// left to do here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SweepStep {
    Progressed,
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
enum RowBound {
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
    pub(super) found: Option<SweepHit>,
    /// Candidates left before the whole document has been seen — the
    /// wrap budget, decremented per candidate.
    remaining: usize,
    /// Spec 0235 S21.
    path: PathScratch,
}

impl SearchSweep {
    /// Whether the walk is over. A finished sweep is *kept* rather than
    /// dropped: the prompt still has to draw its answer, and `Enter`
    /// still has to commit it.
    pub(super) fn is_finished(&self) -> bool {
        self.at.is_none()
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
        Some(SearchSweep {
            pattern: SearchPattern::new(pattern),
            dir,
            origin,
            at: Some((origin.at, origin_bound(origin, dir, Visit::Out))),
            found: None,
            remaining: n + 1,
            path: PathScratch::default(),
        })
    }

    /// Whether one candidate matches, and where.
    ///
    /// Spec 0235 S19: a main-pane line offers *two* haystacks to the
    /// same pattern — its rendered text and its owning node's positional
    /// path — with no shape test and no reserved syntax. The text is
    /// tried first, so that a line matching both ways lands on the
    /// column the user can see (S20).
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
                let text = self.line_text(pos);
                // The row's first non-blank — where a path match lands
                // (S20), and the floor every stop's position is measured
                // against (spec 0246 S3a).
                let indent = text.len() - text.trim_start().len();
                if let Some(range) = pick_match(&sweep.pattern, &text, indent, sweep.dir, bound) {
                    return Some(SweepHit {
                        at,
                        start: range.start.max(indent),
                        column: text[..range.start].chars().count(),
                        width: text[range].chars().count(),
                        on_path: false,
                    });
                }
                // Spec 0246 S9: the path is the row's stop only when the
                // row's *text* has no match at all. A text match the
                // bound merely excluded still means the row belongs to
                // its text — without this test the origin row would
                // offer its text matches and then a path stop as well,
                // and the walk would visit it twice per cycle.
                //
                // The extra scan falls only on a bounded row, and the
                // origin is the only row a sweep ever bounds.
                if bound != RowBound::Whole && sweep.pattern.find_range(&text).is_some() {
                    return None;
                }
                // The stop sits where its caret lands, so a bound that
                // has already passed that offset excludes it and a full
                // cycle still visits it once.
                if !bound.admits(indent) {
                    return None;
                }
                drop(text);
                self.write_positional_path(&mut sweep.path, pos.node);
                // Spec 0235 S19 as amended: the path is matched
                // *anchored*, as if the pattern were `^/a/b`. A path is
                // read from the root down, so an unanchored match on one
                // is nearly always an accident.
                return sweep
                    .pattern
                    .starts_with(sweep.path.as_str())
                    .then_some(SweepHit {
                        at,
                        start: indent,
                        column: indent,
                        width: 1,
                        on_path: true,
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
            self.search_sweep = Some(sweep);
            return SweepStep::Idle;
        }
        let before = sweep.found;
        self.advance_sweep(&mut sweep, SEARCH_SWEEP_SLICE);
        let found_changed = sweep.found != before;
        // Spec 0235 S5: the walk finishing with nothing is the *other*
        // change to the result — it is what turns "still looking" into
        // "not there", which S10 draws.
        let exhausted = sweep.is_finished() && sweep.found.is_none();
        let hit = sweep.found;
        self.search_sweep = Some(sweep);
        if found_changed {
            if let Some(hit) = hit {
                self.show_sweep_hit(hit);
            }
        }
        self.search_dirty |= found_changed || exhausted;
        SweepStep::Progressed
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
        let Some(hit) = self.search_sweep.as_ref().and_then(|s| s.found) else {
            return;
        };
        let pattern = self.command_buffer.clone().unwrap_or_default();
        let from = SearchOrigin {
            at: hit.at,
            column: hit.start,
            ..origin
        };
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
        let Some(mut sweep) = sweep else { return };
        self.advance_sweep(&mut sweep, usize::MAX);
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
            None => self.message = format!("pattern not found: {pattern}"),
        }
    }

    /// `n`/`N`, and every non-incremental caller: one sweep of `scope`,
    /// run to the end, applied.
    pub(super) fn run_search(&mut self, scope: SearchScope, dir: SearchDir, pattern: &str) {
        let origin = self.search_origin_for(scope);
        self.search_origin = None;
        self.search_sweep = None;
        let Some(mut sweep) = self.begin_search_sweep(pattern, dir, origin) else {
            return;
        };
        self.advance_sweep(&mut sweep, usize::MAX);
        self.search_dirty = true;
        self.search_highlight = true;
        let found = sweep.found;
        self.search_sweep = Some(sweep);
        match found {
            Some(hit) => self.apply_sweep_hit(scope, hit),
            None => self.message = format!("pattern not found: {pattern}"),
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
    pub(super) fn search_highlight_pattern(&self) -> Option<SearchPattern> {
        if !self.search_highlight {
            return None;
        }
        if let (Some(buf), CommandLineKind::Search(_)) = (&self.command_buffer, self.command_kind) {
            return (!buf.is_empty()).then(|| SearchPattern::new(buf));
        }
        let last = match self.search_scope() {
            SearchScope::Main => &self.last_search,
            SearchScope::Override => &self.last_override_search,
            SearchScope::Manage => &self.last_manage_search,
        };
        last.as_ref().map(|(_, p)| SearchPattern::new(p))
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
                let top = scroll.top(1);
                let i = i as isize;
                if height > 0 && (i < top || i >= top + height as isize) {
                    // Never above the first row: centering brings a match
                    // into view, and blank rows above it would be room
                    // spent on nothing.
                    scroll.set_top((i - (height / 2) as isize).max(0), 1);
                }
            }
        }
    }

    /// The vertical half of S12, in terminal rows so that a wire row
    /// (spec 0225 S8) counts as the half of a line it is.
    fn center_row(&mut self, row: usize, pane_height: isize) {
        let line = self.row_height() as isize;
        if pane_height < line {
            return;
        }
        let pos = (row * self.row_height()) as isize;
        let top = self.scroll_top();
        if pos >= top && pos + line <= top + pane_height {
            return;
        }
        let last = (self.composed_row_count() as isize * line - pane_height).max(0);
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
    let c = origin.column;
    match (origin.at, dir, visit) {
        // Spec 0246 N4: a side pane counts entries, not matches.
        (SweepCursor::Index(_), _, Visit::Out) => RowBound::Nothing,
        (SweepCursor::Index(_), _, Visit::Back) => RowBound::Whole,
        (_, SearchDir::Forward, Visit::Out) => RowBound::Starts {
            lo: c + 1,
            hi: usize::MAX,
        },
        (_, SearchDir::Forward, Visit::Back) => RowBound::Starts { lo: 0, hi: c + 1 },
        (_, SearchDir::Backward, Visit::Out) => RowBound::Starts { lo: 0, hi: c },
        (_, SearchDir::Backward, Visit::Back) => RowBound::Starts {
            lo: c,
            hi: usize::MAX,
        },
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
    while let Some(range) = pattern.find_range_from(haystack, from) {
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
