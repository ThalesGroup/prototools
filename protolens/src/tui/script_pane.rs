// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0271: applying a script step to a live session.
//!
//! The format lives in `crate::script`; this is the half that touches
//! `App`. The rule the whole module is built around is spec 0271 S6:
//! **a step declares a view, it does not describe a change.** Applying
//! step *n* resets the state a step can name and then re-derives all of
//! it from the step and the current document, so re-applying a step
//! always produces the same view and stepping backward needs no undo
//! stack.
//!
//! The reset is what makes that true, so it is deliberately the first
//! thing `apply` does and deliberately covers *everything* a step can
//! set — a directive family that gains a new member and forgets to
//! reset it is how this becomes a delta language by accident.

use std::fmt::Write as _;

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::pane_scroll::{AnchorLine, WireAnchor, WireSpan};
use super::search::SearchScope;
use super::{App, CursorPos, SearchDir};
use crate::script::{FoldEntry, Position, Predicate, Script, Step, Wire};
use crate::theme;
use crate::tui::heat_cue::HeatCueMode;

/// Spec 0271 S4: the script pane's share of the terminal, and the two
/// absolute bounds on it.
///
/// Below `MIN` there is no room for a sentence; above `MAX` the blob —
/// the thing being explained — stops dominating the screen.
const PANE_PERCENT: u16 = 25;
const PANE_MIN: u16 = 3;
const PANE_MAX: u16 = 12;

/// A loaded script and where the session is in it.
pub(crate) struct ScriptState {
    pub(super) script: Script,
    /// 0-based index into `script.steps`.
    pub(super) current: usize,
    /// Whether `,`/`;`/`?`/`.` step and scroll the script rather than
    /// meaning what they otherwise mean (spec 0271 S7). `space` toggles
    /// it.
    ///
    /// Starts **on** (amended 2026-08-10). S8 had it start off because
    /// the pane "arrives unasked for", which stopped being true: a
    /// script is on screen only because `--script` named one, and a
    /// reader who asked for a walkthrough should not have to find a key
    /// before the walkthrough answers to anything.
    pub(super) active: bool,
    /// First line of the step's text drawn in the pane.
    pub(super) scroll: u16,
    /// What the last application of a step could not do (spec 0271 S13),
    /// shown on the message row.
    pub(super) diagnostics: Vec<String>,
}

impl App {
    /// Attach `script` and apply its first step (spec 0271 S8).
    pub(crate) fn set_script(&mut self, script: Script) {
        self.script = Some(ScriptState {
            script,
            current: 0,
            active: true,
            scroll: 0,
            diagnostics: Vec::new(),
        });
        // A script has its own first-frame story to tell; the splash
        // screen competes with it and is not useful here.
        self.splash = false;
        self.script_apply();
    }

    pub(super) fn script_active(&self) -> bool {
        self.script.as_ref().is_some_and(|s| s.active)
    }

    /// `Tab` — turn script navigation on or off (spec 0355 S3).
    ///
    /// Turning it *on* re-applies the current step, so the gesture that
    /// puts the session back under the script also puts the view back
    /// where the script left it. Wandering off between steps is free
    /// (spec 0271 G3); coming back is one key.
    pub(super) fn script_toggle(&mut self) {
        self.script_focus = false;
        let Some(state) = self.script.as_mut() else {
            return;
        };
        state.active = !state.active;
        if state.active {
            self.script_apply();
        }
    }

    /// `space` / `PageDown` — scroll step text down one page, then
    /// advance to the next step (spec 0355 S1).
    pub(super) fn script_space(&mut self) {
        let max = self.script_max_scroll();
        let scroll = self.script.as_ref().map_or(0, |s| s.scroll);
        if scroll < max {
            self.script_scroll_page(true);
        } else {
            self.script_advance(true);
        }
    }

    /// `Backspace` / `PageUp` — scroll step text up one page, then go
    /// to the previous step (spec 0355 S2).
    pub(super) fn script_backspace(&mut self) {
        let scroll = self.script.as_ref().map_or(0, |s| s.scroll);
        if scroll > 0 {
            self.script_scroll_page(false);
        } else {
            self.script_advance(false);
        }
    }

    /// Step forward or backward unconditionally.
    pub(super) fn script_advance(&mut self, forward: bool) {
        let Some(state) = self.script.as_mut() else {
            return;
        };
        let last = state.script.steps.len() - 1;
        let next = if forward {
            state.current + 1
        } else {
            match state.current.checked_sub(1) {
                Some(n) => n,
                None => {
                    self.message = "script: at the first step".to_string();
                    return;
                }
            }
        };
        if next > last {
            self.message = "script: at the last step".to_string();
            return;
        }
        state.current = next;
        self.script_apply();
    }

    /// Scroll the pane by one pane-height, clamped at both ends.
    fn script_scroll_page(&mut self, down: bool) {
        let max = self.script_max_scroll();
        let pane_height = self.script_area.height;
        let Some(state) = self.script.as_mut() else {
            return;
        };
        state.scroll = if down {
            state.scroll.saturating_add(pane_height).min(max)
        } else {
            state.scroll.saturating_sub(pane_height)
        };
    }

    /// Scroll the pane by one line, clamped at both ends.
    /// Used by the mouse wheel (spec 0355 S5).
    pub(super) fn script_scroll_by(&mut self, down: bool) {
        let max = self.script_max_scroll();
        let Some(state) = self.script.as_mut() else {
            return;
        };
        state.scroll = if down {
            state.scroll.saturating_add(1).min(max)
        } else {
            state.scroll.saturating_sub(1)
        };
    }

    /// The last row the commentary may be scrolled to: one paneful short
    /// of the step's wrapped end, and zero when the step already fits
    /// (2026-08-12).
    ///
    /// A step is a paragraph, not a document — panning past either end
    /// of it shows blank rows and loses the only thing the pane is for.
    ///
    /// The count comes from the `Paragraph` the pane will actually draw,
    /// through ratatui's own `line_count`. A word-wrapper of our own
    /// would be a second answer to a question the widget already
    /// answers, and the two would disagree the moment either changed.
    fn script_max_scroll(&self) -> u16 {
        let area = self.script_area;
        let Some(state) = self.script.as_ref() else {
            return 0;
        };
        let rows = script_paragraph(state, theme::script_pane_style(self.theme))
            .line_count(area.width)
            .try_into()
            .unwrap_or(u16::MAX);
        rows.saturating_sub(area.height)
    }

    /// Spec 0271 S6: put the session into the view the current step
    /// declares.
    pub(super) fn script_apply(&mut self) {
        let Some(state) = self.script.as_ref() else {
            return;
        };
        // Cloned so the rest of this borrows `self` mutably. A step is a
        // handful of short strings; the alternative is threading an
        // index through every helper below for no measurable gain.
        let step: Step = state.script.steps[state.current].clone();

        self.script_reset();
        let mut errors = Vec::new();
        // Spec 0356 S8: apply mode directives before cursor/fold/wire so
        // that heat-cue visibility is correct when the view is composed.
        if let Some(on) = step.set_annotations {
            self.annotations = on;
        }
        if let Some(mode) = step.set_heat_cues {
            self.heat_cues = mode;
        }
        self.script_apply_folds(&step, &mut errors);
        self.script_apply_cursor(&step, &mut errors);
        self.script_apply_wire(&step, &mut errors);
        self.script_apply_search(&step, &mut errors);
        self.script_apply_select(&step);
        self.script_focus(&step);
        if let Some(prefill) = &step.prefill {
            // Spec 0271 S11: typed, not run. The command line reports
            // its own errors when the reader presses Enter.
            self.open_command_line(super::CommandLineKind::Command, prefill.clone());
        }

        if let Some(state) = self.script.as_mut() {
            state.scroll = 0;
            state.diagnostics = errors;
        }
        let diagnostics = self
            .script
            .as_ref()
            .map(|s| s.diagnostics.join("; "))
            .unwrap_or_default();
        if !diagnostics.is_empty() {
            self.message = format!("script: {diagnostics}");
        }

        // Spec 0356 G3: evaluate advance_when immediately after apply.
        // A step whose own directives already satisfy its exit condition
        // skips forward without waiting for user input.
        if self.script_advance_when_satisfied() {
            self.script_advance(true);
        }
    }

    /// Spec 0356 S7: true iff every predicate in the current step's
    /// `advance_when` list holds.
    pub(super) fn script_advance_when_satisfied(&mut self) -> bool {
        let Some(state) = self.script.as_ref() else {
            return false;
        };
        if !state.active {
            return false;
        }
        let predicates = state.script.steps[state.current].advance_when.clone();
        if predicates.is_empty() {
            return false;
        }
        predicates.iter().all(|p| self.script_eval_predicate(p))
    }

    /// Evaluate one predicate against the current session state.
    fn script_eval_predicate(&mut self, predicate: &Predicate) -> bool {
        match predicate {
            Predicate::Visible { position } => {
                let Some(idx) = self.script_resolve(position) else {
                    return false;
                };
                // Visible iff no ancestor is folded.
                let mut cur = idx;
                while let Some(parent) = self.parent(cur) {
                    if self.is_folded(parent) {
                        return false;
                    }
                    cur = parent;
                }
                true
            }
            Predicate::Folded { position } => {
                let Some(idx) = self.script_resolve(position) else {
                    return false;
                };
                self.is_folded(idx) && self.has_children(idx)
            }
            Predicate::Wire { position } => {
                let Some(idx) = self.script_resolve(position) else {
                    return false;
                };
                let Some(span) = self.wire else {
                    return false;
                };
                // Resolve anchors to absolute document lines (not visible
                // rows) so the predicate is independent of scroll position.
                let anchor_line = |anchor: WireAnchor| -> usize {
                    let node = anchor.node;
                    let line_in_node = match anchor.line {
                        AnchorLine::Footer => {
                            self.tree[node].lines_total.saturating_sub(1) as usize
                        }
                        AnchorLine::FromStart(k) => k as usize,
                    };
                    self.absolute_start(node) + line_in_node
                };
                let first = anchor_line(span.first);
                let last = anchor_line(span.last);
                let node_line = self.absolute_start(idx);
                (first.min(last)..=first.max(last)).contains(&node_line)
            }
            Predicate::Type { position, fqdn } => {
                let Some(idx) = self.script_resolve(position) else {
                    return false;
                };
                self.effective_type(idx).as_deref() == Some(fqdn.as_str())
            }
            Predicate::FieldName { position, name } => {
                let Some(idx) = self.script_resolve(position) else {
                    return false;
                };
                self.field_name_for(idx) == *name
            }
            Predicate::FileExists { path } => std::path::Path::new(path).exists(),
            Predicate::Caret { position } => {
                let Some(idx) = self.script_resolve(position) else {
                    return false;
                };
                self.cursor == idx
            }
            Predicate::Annotations { on } => self.annotations == *on,
            Predicate::HeatCues { mode } => self.heat_cues == *mode,
            Predicate::Not { inner } => !inner.iter().all(|p| self.script_eval_predicate(p)),
        }
    }

    /// Everything a step can set, put back to its default.
    ///
    /// Not an undo: nothing here reads what the previous step did. That
    /// is the whole of spec 0271 N1.
    fn script_reset(&mut self) {
        self.script_reset_folds();
        // `set_wire_span(None, _)` clears whatever the probe row says —
        // the toggle only ever chooses between `None` and the target,
        // and the target is `None`.
        if self.wire.is_some() {
            self.set_wire_span(None, 0);
        }
        self.clear_selection();
        // `clear_search_highlight` rather than `cancel_search`: the
        // latter restores a saved scroll position that was set at prompt
        // open time, which has no meaning here and would fight the step's
        // `node:` placement.
        self.clear_search_highlight();
        if self.command_buffer.is_some() {
            self.command_buffer = None;
            self.command_cursor = 0;
            self.cancel_search();
        }
        // Reset modes to their defaults so each step starts from a
        // known baseline.  Step directives (`set_annotations`,
        // `set_heat_cues`) override these immediately after.
        self.annotations = true;
        self.heat_cues = HeatCueMode::Off;
    }

    /// Drop every *user* fold, deepest-first.
    ///
    /// Deepest-first because `refresh_line_counts` climbs upward from
    /// the node it is given and stops as soon as a level's difference is
    /// zero — refreshing a parent before its children would propagate
    /// counts that are about to move again. Level order (spec 0216) puts
    /// a parent at a lower slot than its children, so descending slot
    /// order *is* deepest-first.
    ///
    /// `auto_folded` is deliberately untouched: it is not a fold the
    /// reader or the script chose, it is rendering that has not happened
    /// yet, and the way to clear it is to bake (S12).
    fn script_reset_folds(&mut self) {
        // A descending sweep of the arena rather than of the set, for
        // the reason the `Fold::All` arm below sweeps the same way: since
        // spec 0323 every bracketed slot is folded by default, so
        // collecting the set would allocate one `usize` per node of the
        // document to walk it in an order the arena already gives.
        //
        // Every slot is a member since spec 0338 S1, so every clear
        // reports a move and every slot gets a refresh. On a scalar that
        // refresh recomputes the counts the node already holds and stops
        // before its first ancestor.
        for idx in (0..self.tree.len()).rev() {
            if self.set_folded(idx, false) {
                self.refresh_line_counts(idx);
            }
        }
        self.folds_changed();
    }

    fn script_apply_folds(&mut self, step: &Step, errors: &mut Vec<String>) {
        for entry in &step.fold {
            self.script_apply_fold_entry(entry, errors);
        }
        self.script_settle_cursor();
    }

    /// Spec 0359 S3: resolve `entry.position` and apply `set_fold_depth`.
    fn script_apply_fold_entry(&mut self, entry: &FoldEntry, errors: &mut Vec<String>) {
        match self.script_resolve(&entry.position) {
            Some(idx) => self.set_fold_depth(idx, entry.depth),
            None => errors.push(unresolved(&entry.position)),
        }
    }

    /// Move the cursor out of any region the step just folded.
    ///
    /// A `fold: all` with no `node:` would otherwise leave the cursor on
    /// a node that is no longer drawn, and every viewport calculation
    /// downstream is about a row the cursor has.
    fn script_settle_cursor(&mut self) {
        let mut target = self.cursor;
        let mut cur = self.cursor;
        while let Some(parent) = self.parent(cur) {
            if self.is_folded(parent) {
                target = parent;
            }
            cur = parent;
        }
        if target != self.cursor {
            self.set_cursor(target);
        }
    }

    fn script_apply_cursor(&mut self, step: &Step, errors: &mut Vec<String>) {
        let Some(position) = &step.node else {
            return;
        };
        match self.script_resolve(position) {
            Some(idx) => {
                self.unfold_ancestors(idx);
                self.set_cursor(idx);
            }
            None => errors.push(unresolved(position)),
        }
    }

    /// Spec 0357: engage the selection background on the caret's current
    /// line when the step declares `select_line: true`.
    ///
    /// Runs after `script_apply_search`, so if the search moved the
    /// caret to a match, the selection lands on that line. The anchor is
    /// fixed at column 0; the caret is moved to the last column
    /// (including any `; shadowed_scalar` suffix) to make the span cover
    /// the full line, then immediately restored to wherever the
    /// node/search directives left it. Because the selection is
    /// persistent it survives the restore. `script_focus` runs after
    /// this, so scroll positioning has the final word.
    fn script_apply_select(&mut self, step: &Step) {
        if !step.select_line {
            return;
        }
        let saved_column = self.cursor_column;
        let (_, last) = self.caret_bounds();
        self.select_anchor = Some(CursorPos {
            node: self.cursor,
            line_in_node: self.cursor_line_in_node,
            column: 0,
        });
        // Record the moving end at the last column — this is what makes
        // the span cover the full line including any `; shadowed_scalar`
        // suffix. `select_caret` is independent of `cursor_column`, so
        // the span stays full-line even after the caret is restored.
        self.select_caret = Some(CursorPos {
            node: self.cursor,
            line_in_node: self.cursor_line_in_node,
            column: last,
        });
        self.select_engaged = true;
        // Restore the caret to where node/search left it. The selection
        // span is unaffected because it reads `select_caret`, not
        // `cursor_column`. scroll positioning is left to `script_focus`,
        // which runs after this.
        self.cursor_column = saved_column;
    }

    /// Spec 0357: fire the search highlight for the step's `search:`
    /// pattern, as if `/pattern Enter` had been typed from column 0 of
    /// the caret node's header line.
    fn script_apply_search(&mut self, step: &Step, errors: &mut Vec<String>) {
        let Some(pattern) = step.search.clone() else {
            return;
        };
        // Build the origin from column 0 of the node header, regardless
        // of where `select_line:` may have left `cursor_column`.
        let saved_line = self.cursor_line_in_node;
        let saved_col = self.cursor_column;
        self.cursor_line_in_node = 0;
        self.cursor_column = 0;
        let origin = self.search_origin_for(SearchScope::Main);
        self.cursor_line_in_node = saved_line;
        self.cursor_column = saved_col;
        self.set_last_search_for(SearchScope::Main, (SearchDir::Forward, pattern.clone()));
        self.search_origin = Some(origin);
        self.search_highlight = true;
        self.commit_search(SearchDir::Forward, &pattern);
        // If the pattern did not compile, `search_sweep` stays `None`
        // and `commit_search` sets `self.message`. Demote that to a
        // diagnostic so the step's own commentary can still appear on
        // `self.message` (spec 0271 S13).
        if self.search_sweep.is_none() {
            self.search_highlight = false;
            errors.push(format!("search: pattern {pattern:?} did not compile"));
        }
    }

    /// Spec 0279 S5: start the view at the outermost thing enclosing the
    /// step's node that still fits in the pane.
    ///
    /// Not `clamp_scroll_to_cursor`, which is the *reader's* rule: it
    /// moves only far enough to bring a row on screen, so a step whose
    /// node is below the previous step's view lands it on the pane's
    /// **last** row — with everything the step is about, its subtree and
    /// its wire rows, off the bottom. A step declares a view (spec 0271
    /// S6), and where the view starts is part of it.
    ///
    /// The climb is what makes the rule useful rather than merely
    /// correct. A step's node is a field inside a submessage inside a
    /// section, and under `fold: all` the section is a dozen rows: the
    /// view then opens on the section's own header, so the row that
    /// *names* the anomaly is on screen with it. It also holds two
    /// consecutive steps aimed at neighboring fields of one section on
    /// the same view, which is what makes an anomaly and its canonical
    /// twin comparable across a keypress.
    ///
    /// **Amended 2026-08-12: the preceding sibling comes too, if it
    /// fits.** A caption need not be an ancestor of what it captions. In
    /// `tests/fixtures/anomalies.pb` it is the opposite — the heading is a
    /// top-level `name` line and the example is the submessage *beside*
    /// it, precisely so that the heading survives folding, which a
    /// caption written inside the wrapper does not. The climb alone then
    /// puts the wrapper's own first row at the top of the pane and
    /// leaves the row that names the section one row above it, off
    /// screen. So after the climb the view reaches back over the fitting
    /// ancestor's previous sibling as well, on the one condition that
    /// the two together still fit.
    ///
    /// This is structural rather than a margin: a fixed number of
    /// context rows was tried and is wrong, because it moves the same
    /// subtree to a different place depending on what happens to sit
    /// above it, and two steps aimed at the twin halves of one anomaly
    /// must land on the same view. A sibling is the same sibling for
    /// both of them.
    ///
    /// Runs after the wire span is set, because a wire row makes its
    /// document row two terminal rows tall and every extent here is in
    /// terminal rows.
    fn script_focus(&mut self, step: &Step) {
        if step.node.is_none() || self.tree.is_empty() {
            return;
        }
        let pane = self.main_area.height as usize;
        if pane == 0 {
            return;
        }
        let heights = self.row_heights();
        // `extent` is `None` for a node drawn nowhere — a folded-away
        // ancestor cannot be aimed at, and the climb stops below it.
        let extent = |app: &Self, idx: usize| {
            let row = app.visible_row_of_line(app.absolute_start(idx))?;
            let rows = app.tree[idx].lines_visible as usize;
            Some((row, heights.offset(row + rows) - heights.offset(row)))
        };
        let mut node = self.cursor;
        let mut top = match extent(self, node) {
            Some((row, _)) => row,
            None => return,
        };
        let mut bottom = top + self.tree[node].lines_visible as usize;
        while let Some(parent) = self.parent(node) {
            match extent(self, parent) {
                Some((row, height)) if height <= pane => {
                    top = row;
                    bottom = row + self.tree[parent].lines_visible as usize;
                }
                // Too tall, or not drawn: nothing above it is shorter,
                // so this is as far out as the view can open.
                _ => break,
            }
            node = parent;
        }
        // The caption, when there is one, is the sibling above.
        if let Some(prev) = self.prev_sibling(node) {
            if let Some((row, _)) = extent(self, prev) {
                if heights.offset(bottom) - heights.offset(row) <= pane {
                    top = row;
                }
            }
        }
        self.set_scroll_top(heights.offset(top) as isize);
    }

    fn script_apply_wire(&mut self, step: &Step, errors: &mut Vec<String>) {
        let Some(wire) = &step.wire else {
            return;
        };
        match wire {
            Wire::Line(position) => match self.script_resolve(position) {
                Some(idx) => {
                    let line = self.absolute_start(idx);
                    match self.wire_span_of_lines(line, line) {
                        Some(span) => self.show_wire_span(span),
                        None => errors.push(format!("{} has no line", position.as_written())),
                    }
                }
                None => errors.push(unresolved(position)),
            },
            Wire::Lines { from, to } => {
                let (Some(a), Some(b)) = (self.script_resolve(from), self.script_resolve(to))
                else {
                    for position in [from, to] {
                        if self.script_resolve(position).is_none() {
                            errors.push(unresolved(position));
                        }
                    }
                    return;
                };
                let (first, last) = (self.absolute_start(a), self.absolute_start(b));
                if first > last {
                    errors.push(format!(
                        "wire-lines: {} comes after {}",
                        from.as_written(),
                        to.as_written()
                    ));
                    return;
                }
                match self.wire_span_of_lines(first, last) {
                    Some(span) => self.show_wire_span(span),
                    None => errors.push("wire-lines: no lines there".to_string()),
                }
            }
            Wire::Node(position) => match self.script_resolve(position) {
                Some(idx) => {
                    let span = WireSpan {
                        first: WireAnchor {
                            node: idx,
                            line: AnchorLine::FromStart(0),
                        },
                        last: WireAnchor {
                            node: idx,
                            line: AnchorLine::Footer,
                        },
                    };
                    // The footer anchor is a row of the *subtree*, so
                    // everything under `idx` has to have been rendered.
                    self.bake_subtree(idx);
                    self.show_wire_span(span);
                }
                None => errors.push(unresolved(position)),
            },
        }
    }

    // -----------------------------------------------------------------
    // Resolving a position (spec 0271 S3), baking what it needs (S12)
    // -----------------------------------------------------------------

    fn script_resolve(&mut self, position: &Position) -> Option<usize> {
        match position {
            Position::Path(path) => self.script_resolve_path(path),
            Position::Search(text) => {
                self.script_bake_document();
                self.script_find_text(text)
            }
        }
    }

    /// `App::resolve_path`, baking each level on the way down.
    ///
    /// The plain resolver cannot reach into unbaked territory:
    /// `child_slots` reports no children for a node whose first child
    /// slot was never rendered (spec 0216), which is exactly what a
    /// bounded open leaves behind (spec 0257). So each level is expanded
    /// before its children are counted.
    ///
    /// Holding `cur` across `expand_auto_fold` is sound because a splice
    /// allocates, abandons and renumbers no slot (spec 0216 S12) — the
    /// arena is a function of the bytes and does not move when the
    /// interpretation changes.
    fn script_resolve_path(&mut self, path: &str) -> Option<usize> {
        let mut cur = self.first_node;
        self.script_bake_node(cur);
        if path == "/" {
            return Some(cur);
        }
        for segment in path.trim_start_matches('/').split('/') {
            let position: usize = segment.parse().ok()?;
            cur = self.nth_child(cur, position.checked_sub(1)?)?;
            self.script_bake_node(cur);
        }
        Some(cur)
    }

    fn script_bake_node(&mut self, idx: usize) {
        if self.auto_folded.contains(idx) {
            self.expand_auto_fold(idx, usize::MAX);
        }
    }

    /// Spec 0271 S12: a search reads rendered text, so there has to be
    /// some.
    ///
    /// Charged once per session rather than once per step — the bake is
    /// a debt against the whole document and paying it leaves nothing
    /// owed. A script written entirely in positional paths never comes
    /// here at all.
    fn script_bake_document(&mut self) {
        if self.auto_folded.is_empty() {
            return;
        }
        let root = self.first_node;
        self.bake_subtree(root);
    }

    /// The first node in document order whose own text contains `needle`.
    ///
    /// A plain substring test over each node's own lines, not the `/`
    /// prompt's sweep: a script position wants an answer, not a
    /// highlight, a history entry or a resumable cursor, and going
    /// through the prompt's machinery would leave all three behind.
    fn script_find_text(&self, needle: &str) -> Option<usize> {
        let mut cur = Some(self.first_node);
        while let Some(idx) = cur {
            if self.node_text[idx]
                .as_deref()
                .is_some_and(|text| text.contains(needle))
            {
                return Some(idx);
            }
            cur = self.doc_next(idx);
        }
        None
    }

    // -----------------------------------------------------------------
    // Drawing
    // -----------------------------------------------------------------

    /// Spec 0271 S4: how many rows the commentary takes, excluding its
    /// separator.
    ///
    /// Zero when no script is loaded, and zero while navigation is off
    /// (amended 2026-08-10): commentary the reader has stepped out of is
    /// a quarter of the screen spent on a paragraph about wherever the
    /// script last was, which is not where the reader now is. The
    /// separator stays — see [`Self::script_separator_rows`] — so the
    /// script is still visibly there, and `space` brings the text back.
    pub(super) fn script_rows(&self, total: u16) -> u16 {
        if !self.script_active() {
            return 0;
        }
        match self.script_height {
            Some(rows) => rows.min(total.saturating_sub(2)),
            None => (total * PANE_PERCENT / 100).clamp(PANE_MIN, PANE_MAX),
        }
    }

    /// The separator's own height, which is *not* a function of the
    /// pane's.
    ///
    /// One row for as long as a script is loaded, whether or not
    /// navigation is on. With the commentary at zero rows this rule and
    /// its legend are the whole of what says a script is attached and
    /// how to get back into it, so deriving it from `script_rows` — as
    /// the layout did until 2026-08-10 — hides exactly the row that has
    /// to survive.
    pub(super) fn script_separator_rows(&self) -> u16 {
        u16::from(self.script.is_some())
    }

    pub(super) fn render_script_pane(&mut self, frame: &mut Frame, area: Rect) {
        // Recorded before the early return so that `script_max_scroll`
        // is answering about the pane as it was last drawn.
        self.script_area = area;
        let Some(state) = self.script.as_ref() else {
            return;
        };
        let pane =
            script_paragraph(state, theme::script_pane_style(self.theme)).scroll((state.scroll, 0));
        frame.render_widget(pane, area);
    }

    pub(super) fn render_script_separator(&self, frame: &mut Frame, area: Rect) {
        let Some(state) = self.script.as_ref() else {
            return;
        };
        let style = theme::script_rule_style(self.theme);
        let width = area.width as usize;
        let legend = script_legend(state, width);
        let mut text = String::new();
        if legend.is_empty() {
            text.push_str(&"─".repeat(width));
        } else {
            // Flushed right, with two rule characters of run-out so the
            // legend reads as sitting *on* the rule rather than ending
            // it. Left-aligned it began in the same column the document
            // starts in one row below, and at a glance it read as a line
            // of the blob — which is the one thing the separator exists
            // to rule out.
            let tail = format!(" {legend} ──");
            let lead = width.saturating_sub(tail.chars().count());
            text.push_str(&"─".repeat(lead));
            text.push_str(&tail);
        }
        frame.render_widget(Paragraph::new(Line::from(Span::styled(text, style))), area);
    }

    // -----------------------------------------------------------------
    // Transcript (spec 0271 S14)
    // -----------------------------------------------------------------

    /// Apply every step in order and describe what each one produced.
    ///
    /// The test vehicle for a script: it needs no terminal, and it
    /// reports the resolved outcome of each directive rather than the
    /// directive itself, so a script that has drifted out of sync with
    /// its blob shows up as a diff.
    pub(crate) fn script_transcript(&mut self) -> String {
        let mut out = String::new();
        let Some(state) = self.script.as_ref() else {
            return out;
        };
        let total = state.script.steps.len();
        // The file name rather than the path: the transcript is a golden
        // test's expected output, and where the fixture happens to sit is
        // not part of what the script says.
        if let Some(name) = state.script.path.file_name() {
            let _ = writeln!(out, "script: {}", name.to_string_lossy());
        }
        if let Some(title) = &state.script.title {
            let _ = writeln!(out, "title: {title}");
        }
        for index in 0..total {
            if let Some(state) = self.script.as_mut() {
                state.current = index;
            }
            self.script_apply();
            let _ = writeln!(out, "step {}/{}", index + 1, total);
            let _ = writeln!(out, "  node: {}", self.positional_path(self.cursor));
            // Filtered, not `user_folded.len()`: spec 0338 S1 makes the
            // set total, so its length counts every scalar in the
            // document. What a beat is pinned against is the number of
            // nodes a reader sees collapsed.
            let folded = self
                .user_folded
                .iter()
                .filter(|&idx| self.is_foldable(idx))
                .count();
            let _ = writeln!(out, "  folded: {folded}");
            match self.wire_rows() {
                Some(rows) => {
                    let _ = writeln!(out, "  wire: rows {}..{}", rows.start, rows.end);
                }
                None => {
                    let _ = writeln!(out, "  wire: none");
                }
            }
            if let Some(prefill) = &self.command_buffer {
                let _ = writeln!(out, "  prefill: {prefill}");
            }
            let diagnostics = self
                .script
                .as_ref()
                .map(|s| s.diagnostics.clone())
                .unwrap_or_default();
            for diagnostic in diagnostics {
                let _ = writeln!(out, "  error: {diagnostic}");
            }
            let first = self
                .script
                .as_ref()
                .and_then(|s| s.script.steps[index].text.lines().next())
                .unwrap_or("");
            let _ = writeln!(out, "  text: {first}");
        }
        out
    }
}

fn unresolved(position: &Position) -> String {
    match position {
        Position::Path(path) => format!("no node at {path}"),
        Position::Search(text) => format!("no match for {text:?}"),
    }
}

/// The current step's commentary, wrapped the way the pane draws it.
///
/// One construction shared by the renderer and [`App::script_max_scroll`]
/// — the scroll bound is a fact about this exact paragraph, so deriving
/// it from a differently-built one would be how the two drift apart.
fn script_paragraph(state: &ScriptState, style: Style) -> Paragraph<'static> {
    let text: Vec<Line> = state.script.steps[state.current]
        .text
        .lines()
        .map(|l| Line::styled(l.to_string(), style))
        .collect();
    Paragraph::new(text).style(style).wrap(Wrap { trim: false })
}

/// Spec 0355 S7: the micro-help on the separator.
///
/// Two states: navigation on (step counter + advance hint) and off
/// (toggle hint). The step counter goes last when narrowing because it
/// is the thing a presenter reads out loud — the key reminder can be
/// learned once, but "where am I" changes every step.
fn script_legend(state: &ScriptState, width: usize) -> String {
    if !state.active {
        for candidate in ["Tab to resume script navigation", "Tab to resume"] {
            if fits(candidate, width) {
                return candidate.to_string();
            }
        }
        return String::new();
    }
    let counter = format!("step {}/{}", state.current + 1, state.script.steps.len());
    let ladder = [
        format!("Tab to pause  space/Backspace step  {counter}"),
        format!("space/Backspace step  {counter}"),
        counter,
    ];
    for candidate in ladder {
        if fits(&candidate, width) {
            return candidate;
        }
    }
    String::new()
}

/// A legend needs the two rule characters, the two spaces around it, and
/// at least one rule character after it.
fn fits(legend: &str, width: usize) -> bool {
    legend.chars().count() + 6 <= width
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(active: bool, current: usize, steps: usize) -> ScriptState {
        let text = (0..steps)
            .map(|i| format!("- text: step {i}\n"))
            .collect::<String>();
        ScriptState {
            script: Script::parse(&format!("steps:\n{text}"), "t.script".into())
                .expect("must parse"),
            current,
            active,
            scroll: 0,
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn the_legend_names_what_the_toggle_does_while_navigation_is_off() {
        let state = state(false, 0, 3);
        assert_eq!(script_legend(&state, 80), "Tab to resume script navigation");
        // Too narrow for the sentence: the short spelling, which still
        // says what the key is for.
        assert_eq!(script_legend(&state, 30), "Tab to resume");
        assert_eq!(script_legend(&state, 18), "");
    }

    /// The three rungs narrow gracefully: toggle first, then keys, then
    /// just the counter.
    #[test]
    fn the_legend_shortens_gracefully() {
        let state = state(true, 2, 23);
        assert_eq!(
            script_legend(&state, 80),
            "Tab to pause  space/Backspace step  step 3/23"
        );
        assert_eq!(script_legend(&state, 42), "space/Backspace step  step 3/23");
        assert_eq!(script_legend(&state, 20), "step 3/23");
        // Narrower than the counter plus its rule: the rule alone.
        assert_eq!(script_legend(&state, 14), "");
    }

    /// Each rung is claimed to need a particular width, and `fits`
    /// charges six columns of rule on top of the text. Asserted as a
    /// boundary pair so a change to either the wording or the six shows
    /// up here rather than as a legend that silently stopped appearing.
    #[test]
    fn each_rung_appears_at_exactly_the_width_it_needs() {
        let on = state(true, 2, 23);
        let off = state(false, 0, 3);
        for (state, legend, width) in [
            (&on, "Tab to pause  space/Backspace step  step 3/23", 51),
            (&on, "space/Backspace step  step 3/23", 37),
            (&on, "step 3/23", 15),
            (&off, "Tab to resume script navigation", 37),
            (&off, "Tab to resume", 19),
        ] {
            assert_eq!(script_legend(state, width), legend, "at {width} columns");
            assert_ne!(
                script_legend(state, width - 1),
                legend,
                "at {} columns it must have given way to the next rung",
                width - 1
            );
        }
    }
}
