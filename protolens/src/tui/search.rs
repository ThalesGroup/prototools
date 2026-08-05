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

/// One candidate: a document line in the main pane, a list index in the
/// two side panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SweepCursor {
    Line(LinePos),
    Index(usize),
}

/// Spec 0235 S2: what a sweep found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SweepHit {
    pub(super) at: SweepCursor,
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
}

/// Spec 0235 S2: a search in progress.
pub(super) struct SearchSweep {
    pub(super) pattern: SearchPattern,
    dir: SearchDir,
    origin: SearchOrigin,
    /// The next candidate to test. `None` once the walk has finished,
    /// either by finding a match or by seeing the whole document.
    at: Option<SweepCursor>,
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
        let (scroll, pan, at) = match scope {
            SearchScope::Main => (
                self.scroll,
                self.pan_offset,
                SweepCursor::Line(LinePos {
                    node: self.cursor,
                    line_in_node: self.cursor_line_in_node,
                }),
            ),
            SearchScope::Override => (
                self.override_scroll,
                self.override_pan_offset,
                SweepCursor::Index(self.override_highlight),
            ),
            SearchScope::Manage => (
                self.manage_scroll,
                self.manage_pan_offset,
                SweepCursor::Index(self.manage_highlight),
            ),
        };
        SearchOrigin {
            scope,
            scroll,
            pan,
            at,
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

    /// A sweep of `pattern` over `origin`'s pane, starting one candidate
    /// past the origin so that the origin's own row comes *last* — which
    /// is what makes a wrapped search land back where it started rather
    /// than report "not found". `None` for an empty pattern or an empty
    /// pane, neither of which has anything to walk.
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
            at: Some(self.next_candidate(origin.at, dir, n)?),
            found: None,
            remaining: n,
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
    fn sweep_test(&self, sweep: &mut SearchSweep, at: SweepCursor) -> Option<SweepHit> {
        let haystack: Cow<'_, str> = match (sweep.origin.scope, at) {
            (SearchScope::Main, SweepCursor::Line(pos)) => {
                // A closing `}` draws no content of its own, and a
                // search has never matched one.
                if self.is_footer(pos) {
                    return None;
                }
                let text = self.line_text(pos);
                if let Some(range) = sweep.pattern.find_range(&text) {
                    return Some(SweepHit {
                        at,
                        column: text[..range.start].chars().count(),
                        width: text[range].chars().count(),
                        on_path: false,
                    });
                }
                // The row's first non-blank, which is where a path match
                // lands (S20) — measured here, while its text is in
                // hand, rather than resolved again at `Enter`.
                let indent = text.len() - text.trim_start().len();
                drop(text);
                self.write_positional_path(&mut sweep.path, pos.node);
                return sweep
                    .pattern
                    .is_match(sweep.path.as_str())
                    .then_some(SweepHit {
                        at,
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
        sweep.pattern.find_range(&haystack).map(|range| SweepHit {
            at,
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
            let Some(at) = sweep.at else { return };
            if let Some(hit) = self.sweep_test(sweep, at) {
                sweep.found = Some(hit);
                sweep.at = None;
                return;
            }
            sweep.remaining -= 1;
            sweep.at = (sweep.remaining > 0)
                .then(|| self.next_candidate(at, sweep.dir, n))
                .flatten();
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
    }

    /// Spec 0235 S7: any change to the pattern replaces the sweep —
    /// unconditionally, with no exception for "the pattern only grew".
    /// That symmetry is the entire reason `Backspace` reads as an undo.
    pub(super) fn restart_search_sweep(&mut self) {
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
