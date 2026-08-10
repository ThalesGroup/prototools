// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::command_line::{next_word_boundary, prev_word_boundary};
use super::*;

/// Spec 0235 S21: the two buffers `write_positional_path` reuses across
/// a whole sweep — the formatted path and the leaf-to-root segment list
/// it is built from.
#[derive(Default)]
pub(super) struct PathScratch {
    text: String,
    segments: Vec<usize>,
}

impl PathScratch {
    pub(super) fn as_str(&self) -> &str {
        &self.text
    }
}

impl App {
    /// What every fold toggle does once the line counts agree again:
    /// the pan re-clamp and the prefetch invalidation.
    pub(super) fn folds_changed(&mut self) {
        self.clamp_pan_offset();
        // Spec 0164 G7: any fold/unfold or content-shape change can
        // shift rendered line numbers or invalidate prefetch
        // eligibility — bumping this makes `App::prefetch_step` notice
        // and restart its walk from scratch.
        self.structural_version += 1;
    }

    /// Spec 0268 S1: the shown run, resolved to visible rows.
    ///
    /// `None` when nothing is shown, and also when the span's own nodes
    /// have been rendered away — an `FqdnField` override can flatten a
    /// parent to `bytes`, leaving the anchored node with no row at all,
    /// and the honest answer to "which rows show their bytes" is then
    /// none.
    pub(super) fn wire_rows(&self) -> Option<std::ops::Range<usize>> {
        let span = self.wire?;
        if let Some(cache) = self.wire_rows.borrow().as_ref() {
            if cache.version == self.structural_version {
                return cache.rows.clone();
            }
        }
        let rows = self
            .wire_anchor_row(span.first)
            .zip(self.wire_anchor_row(span.last))
            .map(|(a, b)| a.min(b)..a.max(b) + 1);
        *self.wire_rows.borrow_mut() = Some(WireRowCache {
            version: self.structural_version,
            rows: rows.clone(),
        });
        rows
    }

    /// One end of the span, as a visible row.
    fn wire_anchor_row(&self, anchor: WireAnchor) -> Option<usize> {
        let total = self.tree.get(anchor.node)?.lines_total as usize;
        let last = total.checked_sub(1)?;
        let line_in_node = match anchor.line {
            AnchorLine::Footer => last,
            AnchorLine::FromStart(k) => (k as usize).min(last),
        };
        self.visible_row_of_line(self.absolute_start(anchor.node) + line_in_node)
    }

    /// Spec 0268 S4: how tall each of the main pane's rows is.
    pub(super) fn row_heights(&self) -> RowHeights {
        match self.wire_rows() {
            Some(rows) => RowHeights::new(rows),
            None => FLAT_ROWS,
        }
    }

    /// How many *document* lines the main pane can show.
    ///
    /// Spec 0268 S5: the rows from the current top that fit in the
    /// pane's terminal rows, which is no longer a division — the ones
    /// showing their bytes cost two rows and the rest cost one. Every
    /// scroll computation is stated in document lines, so they all come
    /// here rather than reading `main_area.height` themselves. The side
    /// panes are unaffected and keep reading their own heights.
    pub(super) fn document_pane_height(&self) -> usize {
        let height = self.main_area.height as usize;
        self.row_heights()
            .rows_fitting(self.scroll.index, height)
            .max(1)
    }

    /// The top edge of the viewport, in terminal rows counted from the
    /// top of document line 0. Negative means blank rows above the
    /// document's first line. See `App::scroll`.
    pub(super) fn scroll_top(&self) -> isize {
        self.scroll.top(&self.row_heights())
    }

    /// Puts the viewport's top edge at `top`, splitting it back into the
    /// whole line and the remainder. The only writer of the pair.
    pub(super) fn set_scroll_top(&mut self, top: isize) {
        let heights = self.row_heights();
        self.scroll.set_top(top, &heights);
    }

    /// The terminal row `row` is drawn on, relative to the top of the
    /// main area. Negative or `>= main_area.height` means off screen.
    pub(super) fn terminal_row_of(&self, row: usize) -> isize {
        self.row_heights().offset(row) as isize - self.scroll_top()
    }

    /// Scrolls just far enough that `row`'s own line is on screen.
    ///
    /// Its wire row, if any, is deliberately not required to be: a
    /// cursor on the pane's last terminal row is where the user put it,
    /// and insisting on the byte row under it would scroll the document
    /// out from under them to show something they can already toggle off.
    pub(super) fn clamp_scroll_to_cursor(&mut self, row: usize) {
        let pane = self.main_area.height as isize;
        if pane <= 0 {
            return;
        }
        let pos = self.row_heights().offset(row) as isize;
        let top = self.scroll_top();
        if pos < top {
            self.set_scroll_top(pos);
        } else if pos >= top + pane {
            self.set_scroll_top(pos + 1 - pane);
        }
    }

    /// Clamps `pan_offset` to the current content's valid range — the
    /// same `max_pan_offset` bound `pan_horizontal`'s right branch
    /// enforces, but applied proactively rather than only when the user
    /// happens to pan right again.
    ///
    /// A fold/unfold or an override splice can shrink the visible
    /// content out from under a `pan_offset` that was valid for the
    /// *previous* shape. Left unclamped, every visible row then renders
    /// shorter than `pan_offset`, so `pan_spans` yields nothing for any
    /// of them and the main pane goes blank.
    ///
    /// First re-syncs `scroll_offset` to the cursor's row, mirroring
    /// `render()`'s own auto-pan-into-view guard: this runs
    /// mid-`splice_override`, well before the next `render()` pass would
    /// refresh `scroll_offset` for the new content shape. Computing
    /// `max_pan_offset` against that stale window — rather than the one
    /// the next render will show around the (possibly moved) cursor —
    /// clamps against the wrong rows, panning further left than the true
    /// content width allows.
    pub(super) fn clamp_pan_offset(&mut self) {
        if !self.tree.is_empty() {
            let cursor_row = self.cursor_display_row();
            if self.last_cursor_row != Some(cursor_row) {
                self.clamp_scroll_to_cursor(cursor_row);
                self.last_cursor_row = Some(cursor_row);
            }
        }
        // An unpanned document has nothing to clamp, and `0.min(x)` is
        // `0` for every `x` — so the guard changes no outcome, only what
        // it costs to reach it. `max_pan_offset` resolves the entire
        // visible window through `build_window`, against a window cache
        // the `structural_version` bump immediately above has just
        // emptied, which is O(pane height) *per splice*.
        //
        // That is a real bill and it predates the bake: measured over a
        // full drain of googleapis.desc, it was 19.9 ms per pane row —
        // 1.0 s of a 6.55 s drain on a 50-row terminal, and 97 s of
        // 102 s on a 5000-row one. Every fold and every commit pays the
        // same rate; the bake only made it visible by splicing 70 894
        // times in a row.
        //
        // `pan_offset` is zero unless the reader has actually panned
        // right, so this is the common path, not an optimization for a
        // corner. When it is *not* zero the bill is still owed, because
        // the clamp's bound genuinely depends on which rows are visible.
        if self.pan_offset > 0 {
            self.pan_offset = self.pan_offset.min(self.max_pan_offset());
        }
    }

    /// Spec 0268 S2: `w` — toggle the caret's own line, and carry the
    /// rest of the selection with it.
    ///
    /// The caret's line is what *decides* the new state, and the
    /// selection only widens what that decision applies to. So `w`
    /// always does to the line under the caret what the reader can see
    /// they asked for, whatever else is selected.
    ///
    /// "Unless the caret is outside the selection" needs no code: spec
    /// 0242 made the caret the selection's own moving end, so it is
    /// always one of the two lines the span is built from.
    pub(super) fn wire_lines(&mut self) {
        let caret = self.cursor_line();
        let (first, last) = match self.selection_span() {
            Some((lo, _, hi, _)) => (lo, hi),
            None => (caret, caret),
        };
        let target = self.wire_span_of_lines(first, last);
        self.set_wire_span(target, self.cursor_display_row());
    }

    /// Spec 0268 S3: `W` — the same for the whole subtree containing the
    /// selection, or the caret's own node when nothing is selected.
    pub(super) fn wire_subtree(&mut self) {
        let (lo, hi) = match self.selection_span() {
            Some((lo, _, hi, _)) => (lo, hi),
            None => {
                let caret = self.cursor_line();
                (caret, caret)
            }
        };
        let (Some(a), Some(b)) = (self.line_pos(lo), self.line_pos(hi)) else {
            return;
        };

        // A packed record's elements are drawn as the message's own
        // fields — `vals: 5` sits at the same indent as `a: 42`, with no
        // header and no brace of its own (spec 0216 S7) — even though
        // the arena collapses the whole run into one node. The reader is
        // not looking at that node, so for `W` an element *line* is the
        // unit: one of them is a leaf, and two of them are siblings.
        //
        // Both ends in one unbracketed node of several lines is exactly
        // that case and no other.
        if a.node == b.node && !self.has_children(a.node) && self.tree[a.node].lines_total > 1 {
            if lo == hi {
                // A leaf, so `W` on it is `w` on it — spec 0268 S3's
                // literal case.
                return self.wire_lines();
            }
            // Siblings, so the subtree they name is the message holding
            // them: the run's other elements, the other fields, and any
            // second packed record beside this one.
            let parent = self.parent(a.node).unwrap_or(a.node);
            return self.wire_whole_node(parent);
        }
        self.wire_whole_node(self.common_ancestor(a.node, b.node));
    }

    /// Shows the bytes of every line `node` draws, its descendants'
    /// included.
    ///
    /// A node's lines are contiguous and its descendants' lie between
    /// its header and its footer, so its whole subtree is one span.
    fn wire_whole_node(&mut self, node: usize) {
        let span = WireSpan {
            first: WireAnchor {
                node,
                line: AnchorLine::FromStart(0),
            },
            last: WireAnchor {
                node,
                line: AnchorLine::Footer,
            },
        };
        let probe = self.wire_anchor_row(span.first).unwrap_or(0);
        self.set_wire_span(Some(span), probe);
    }

    /// The span covering absolute lines `first..=last`, or `None` if
    /// either end has no node to anchor to.
    pub(super) fn wire_span_of_lines(&self, first: usize, last: usize) -> Option<WireSpan> {
        let anchor = |line| {
            let pos = self.line_pos(line)?;
            Some(WireAnchor {
                node: pos.node,
                line: if self.is_footer(pos) {
                    AnchorLine::Footer
                } else {
                    AnchorLine::FromStart(pos.line_in_node)
                },
            })
        };
        Some(WireSpan {
            first: anchor(first)?,
            last: anchor(last)?,
        })
    }

    /// The deepest node containing both `a` and `b`, found by climbing
    /// until the two root paths meet — O(depth), which is 13 on the
    /// reference corpus.
    fn common_ancestor(&self, a: usize, b: usize) -> usize {
        let mut path = vec![a];
        let mut cur = a;
        while let Some(p) = self.parent(cur) {
            path.push(p);
            cur = p;
        }
        let mut cur = b;
        loop {
            if path.contains(&cur) {
                return cur;
            }
            match self.parent(cur) {
                Some(p) => cur = p,
                // Separate roots, which only the multi-record test
                // fixtures have: neither contains the other, so there is
                // nothing above them to aim at.
                None => return b,
            }
        }
    }

    /// Spec 0268 S2: turn `target` on, unless `probe_row` already shows
    /// its bytes — in which case the gesture is asking for them off, and
    /// since one `w` *replaces* the run, off means `None`.
    ///
    /// `probe_row` is the row the gesture is pointing at: the caret's
    /// own for `w`, the subtree's header for `W`. There are only ever
    /// these two outcomes, which is what makes the state describable
    /// without reference to its history.
    ///
    /// Spec 0225 S8: the document does not change and neither does the
    /// cursor; what changes is that a document line starts or stops
    /// costing two terminal rows. Left alone that moves the cursor's row
    /// down or up the screen by however far it sat from the top, which
    /// on a full pane is most of it — the user asked to see the bytes,
    /// not to be scrolled. So the terminal row the cursor is drawn on is
    /// measured before the change and restored after it, by moving the
    /// viewport's top edge, which spec 0230 made a signed count of
    /// terminal rows precisely so that it can answer here. The two cases
    /// a whole-line offset could not serve both work out of it: turning
    /// the bytes on from an odd row leaves the pane starting mid-line,
    /// and turning them off near the top leaves blank rows above the
    /// document's first line.
    pub(super) fn set_wire_span(&mut self, target: Option<WireSpan>, probe_row: usize) {
        let shown = self
            .wire_rows()
            .is_some_and(|rows| rows.contains(&probe_row));
        let cursor_row = self.cursor_display_row();
        let drawn = self.terminal_row_of(cursor_row);
        self.wire = if shown { None } else { target };
        *self.wire_rows.borrow_mut() = None;
        let pos = self.row_heights().offset(cursor_row) as isize;
        self.set_scroll_top(pos - drawn);
        // `clamp_pan_offset` re-syncs the scroll only when the cursor has
        // moved since the last clamp, and it has not — the pane changed
        // capacity under it instead. Forgetting the remembered row is how
        // that guard is told the clamp is owed for a new geometry rather
        // than a new cursor, which matters when the restored row falls
        // outside a pane that just got shorter in document lines.
        self.last_cursor_row = None;
        self.clamp_pan_offset();
    }

    /// Spec 0271 S10: show `span`, whatever is showing now.
    ///
    /// [`App::set_wire_span`] answers a *gesture*, and a gesture aimed
    /// at a row that already shows its bytes means "off" — so a script
    /// step, which declares a state rather than making one, cannot use
    /// it directly. Clearing first makes the outcome a function of the
    /// span alone. A step's own reset (spec 0271 S6) has normally
    /// already done this; stating the dependency here is what keeps that
    /// from being a silent premise.
    pub(super) fn show_wire_span(&mut self, span: WireSpan) {
        self.wire = None;
        *self.wire_rows.borrow_mut() = None;
        let probe = self.wire_anchor_row(span.first).unwrap_or(0);
        self.set_wire_span(Some(span), probe);
    }

    /// Whether `idx` is drawn collapsed — because the user folded it,
    /// or because a row-bounded render never emitted its body
    /// (spec 0249 S3). Every *read* of fold state goes through here:
    /// on screen the two are the same row, and every operation that
    /// acts on a folded node acts on both kinds. Only writes
    /// distinguish them.
    pub(super) fn is_folded(&self, idx: usize) -> bool {
        self.folded.contains(&idx) || self.auto_folded.contains(&idx)
    }

    /// Open `idx` whichever set folded it, reporting whether it moved.
    ///
    /// Non-short-circuiting `|`: a node can be in both sets — the user
    /// can fold a node that was already auto-folded — and leaving it in
    /// one of them would draw it collapsed after an unfold gesture.
    pub(super) fn unfold(&mut self, idx: usize) -> bool {
        self.folded.remove(&idx) | self.auto_folded.remove(&idx)
    }

    /// Open `idx` on the user's behalf, rendering its body first if a
    /// bounded render never did (spec 0249 S8).
    ///
    /// This is the difference between the two fold sets that a *reader*
    /// still cannot see: a user fold hides rows that exist, so opening
    /// it is a set removal; an auto-fold stands where no rows were ever
    /// produced, so opening it is a render. Plain [`App::unfold`] does
    /// only the removal and is for the paths that are not a gesture —
    /// the descendant scrub, which is vacating those slots anyway, and
    /// `unfold_ancestors`, which cannot meet an auto-fold because a
    /// stop has no rendered descendants to climb from.
    pub(super) fn open(&mut self, idx: usize) -> bool {
        if self.auto_folded.contains(&idx) {
            // Removed first: the splice below takes `idx` out of
            // `auto_folded` itself, and a node in both sets would
            // otherwise stay drawn collapsed after an open gesture.
            self.folded.remove(&idx);
            self.expand_auto_fold(idx, self.document_pane_height());
            return true;
        }
        self.unfold(idx)
    }

    /// Unfold every ancestor of `idx`, so it becomes visible.
    pub(super) fn unfold_ancestors(&mut self, idx: usize) {
        let mut p = self.parent(idx);
        let mut changed = false;
        while let Some(pi) = p {
            if self.unfold(pi) {
                changed = true;
                // Innermost first, so each level's recomputation reads
                // children that are already up to date. The climb inside
                // `refresh_line_counts` stops at the next still-folded
                // ancestor, which the next iteration then unfolds.
                self.refresh_line_counts(pi);
            }
            p = self.parent(pi);
        }
        if changed {
            self.folds_changed();
        }
    }

    /// Sets `self.cursor` and bumps `cursor_moves` — the sole mutation
    /// path for `self.cursor`, so every real cursor change (even a
    /// round trip that lands back on the same node, e.g. Down then Up)
    /// is observable via `cursor_moves`, unlike comparing `self.
    /// cursor`'s value alone against a stashed old value. Always resets
    /// `cursor_line_in_node` to `0` (spec 0142) — every caller targets a
    /// node's own header row.
    ///
    /// Spec 0194 S1: also resets the caret to the new row's first
    /// reachable column, so a node-level jump lands at the start of the
    /// row's text. Vertical movement wants the opposite rule and so
    /// deliberately does not come through here — see `carry_caret`.
    pub(crate) fn set_cursor(&mut self, idx: usize) {
        self.cursor = idx;
        self.cursor_line_in_node = 0;
        self.cursor_moves += 1;
        self.reset_caret_column();
    }

    /// Spec 0194 S3: the columns of the cursor row's caret track the
    /// caret may rest on, as an inclusive `(first, last)` pair.
    ///
    /// The rule both ends follow: **the caret may rest on anything that
    /// carries information, and not on a control.** Leftward that stops
    /// it at the row's first non-blank character (vim's `^`) — the fold
    /// margin left of it is a click target, not text. Rightward it runs
    /// to the end of the heat suffix when the row has one, since the
    /// suffix is a rendered fact about the node.
    ///
    /// An all-blank row has no track at all and gets a single column,
    /// drawn on a synthetic trailing space (S2).
    ///
    /// Spec 0215 S7: the owner is handed to `row_text_of` rather than
    /// looked up. This is exact, not an approximation:
    /// `node_at_header_line` yields `None` for a footer line and the
    /// owning node for any other, and `cursor_line()` is by definition
    /// one of the cursor's own lines, so
    /// `(!cursor_on_footer()).then_some(cursor)` *is* the value that
    /// lookup would have walked the document to produce.
    pub(super) fn caret_bounds(&self) -> (usize, usize) {
        if self.tree.is_empty() {
            return (0, 0);
        }
        let text = self.row_text_of(
            self.cursor_row(),
            (!self.cursor_on_footer()).then_some(self.cursor),
        );
        let trimmed = text.trim_start();
        if trimmed.is_empty() {
            return (0, 0);
        }
        let len = text.chars().count();
        (
            len - trimmed.chars().count(),
            len - 1 + self.caret_suffix_len,
        )
    }

    /// Spec 0194 S1: put the caret at the start of the row's text and
    /// make that the desired column too — what a node-level jump does.
    ///
    /// Spec 0199 S1 rule 1: a node-level jump *declares* the anchor. The
    /// user asked to be at this node, and the first non-blank is where
    /// its content begins, so arriving there is as deliberate as `^`.
    pub(super) fn reset_caret_column(&mut self) {
        let (first, _) = self.caret_bounds();
        self.cursor_column = first;
        self.desired_column = first;
        self.caret_anchor = CaretAnchor::Home;
    }

    /// Spec 0199 S1 rule 2: derive the anchor from the column a caret
    /// motion just reached — an end reached by walking there is as
    /// deliberate as one reached by `^`/`$`.
    ///
    /// On a one-column row `first == last` and `Home` wins: such a row
    /// is a `}` footer or a blank, where folding is a plausible intent
    /// and descending is not.
    fn settle_anchor(&mut self) {
        let (first, last) = self.caret_bounds();
        self.caret_anchor = if self.cursor_column == first {
            CaretAnchor::Home
        } else if self.cursor_column == last {
            CaretAnchor::End
        } else {
            CaretAnchor::Free
        };
    }

    /// Spec 0194 S11: pull the caret back into the row's reachable range
    /// without disturbing `desired_column`. Needed wherever the row's
    /// text changes under a caret that did not move — a fold toggle
    /// splices in a collapse summary, an override resplices the subtree —
    /// and once per frame, so no other mutation of `lines` can leave the
    /// caret pointing past a row's end.
    pub(super) fn clamp_caret_column(&mut self) {
        let (first, last) = self.caret_bounds();
        self.cursor_column = self.cursor_column.clamp(first, last);
    }

    /// Spec 0194 S5 / spec 0199 S3: carry the caret across a *vertical*
    /// move. A `Free` caret keeps its desired column, clamped into the
    /// new row's range without being forgotten, so crossing a short row
    /// and coming back restores it — vim's rule.
    ///
    /// An *anchored* caret keeps the anchor instead of the column, which
    /// is what makes `^`/`j` walk down the first non-blank of each row
    /// (vim's `'startofline'`) and `$`/`j` walk down the last column of
    /// each row (vim's `curswant = MAXCOL`). The anchor is deliberately
    /// left untouched here: a vertical move relocates the caret but
    /// expresses nothing about whether the user meant to be at an end,
    /// and that distinction is the whole point of the field.
    fn carry_caret(&mut self) {
        let (first, last) = self.caret_bounds();
        self.cursor_column = match self.caret_anchor {
            CaretAnchor::Home => first,
            CaretAnchor::End => last,
            CaretAnchor::Free => self.desired_column.clamp(first, last),
        };
    }

    /// The cursor node is foldable and currently open — the state in
    /// which a leftward key folds instead of moving (spec 0199 G3).
    ///
    /// `has_children` is the *bracketed-node* test rather than the
    /// has-descendants test (see its doc comment, spec 0142), which is
    /// exactly what is wanted here: an empty-but-bracketed message is
    /// foldable and must fold.
    fn cursor_expanded(&self) -> bool {
        self.has_children(self.cursor) && !self.is_folded(self.cursor)
    }

    /// The cursor node is currently folded — the state in which a
    /// rightward key unfolds instead of moving (spec 0199 G4).
    fn cursor_folded(&self) -> bool {
        self.is_folded(self.cursor)
    }

    /// Spec 0194 S6 / spec 0199 S5: `h`, `Left`, `Backspace`.
    ///
    /// Three regimes, in the order the code tests them. Away from the
    /// row's first column it is plain caret motion. At the first column
    /// with the anchor not yet `Home`, the caret was pushed here by a
    /// vertical move's clamp, so the press *adopts* the position rather
    /// than acting on it (G2) — not a wasted keystroke, since it also
    /// collapses `desired_column` onto the real column, which is vim's
    /// own rule for a horizontal motion. At a voluntary `Home` it
    /// delegates to `parent_move`, i.e. to the whole fold-then-parent
    /// sequence of S7 including its `no parent` message, not to a bare
    /// fold.
    ///
    /// `clamp_caret_column` runs first, so the comparison is against the
    /// row's real current bounds rather than a stale column (0194 S11).
    pub(super) fn caret_left(&mut self) {
        self.clamp_caret_column();
        let (first, _) = self.caret_bounds();
        if self.cursor_column > first {
            self.cursor_column -= 1;
            self.desired_column = self.cursor_column;
            self.settle_anchor();
            return;
        }
        if self.caret_anchor != CaretAnchor::Home {
            self.caret_anchor = CaretAnchor::Home;
            self.desired_column = self.cursor_column;
            return;
        }
        self.parent_move();
    }

    /// Spec 0194 S6 / spec 0199 S6: `l`, `Right`. Stops at the last
    /// character of the heat suffix when the row has one.
    ///
    /// The unfold branch is tested first and gated on all three of
    /// anchor, column and fold state. The fold state, because on an
    /// expanded node there is nothing to unfold and the key is plain
    /// motion. The column, because a folded node's header row still
    /// carries a whole line of text (`Name { ... }`) that an
    /// unconditional unfold would make unreachable by caret. The anchor,
    /// because an involuntary `Home` means the user was passing over
    /// this row, not aiming at it.
    ///
    /// Then the mirror of `caret_left`: plain motion while there is row
    /// left, one press to adopt an involuntary `End`, and a descent from
    /// a voluntary one — the opening brace *is* the row's last
    /// character, so continuing right through it is where the tree
    /// actually lies on screen.
    ///
    /// Order matters on a one-column row (`first == last`): the unfold
    /// branch runs first, and an unfolded such row still falls through
    /// to the `End`-adopting branch.
    pub(super) fn caret_right(&mut self) {
        self.clamp_caret_column();
        let (first, last) = self.caret_bounds();
        if self.caret_anchor == CaretAnchor::Home
            && self.cursor_column == first
            && self.cursor_folded()
        {
            self.toggle_fold(self.cursor);
            return;
        }
        if self.cursor_column < last {
            self.cursor_column += 1;
            self.desired_column = self.cursor_column;
            self.settle_anchor();
            return;
        }
        if self.caret_anchor != CaretAnchor::End {
            self.caret_anchor = CaretAnchor::End;
            self.desired_column = self.cursor_column;
            return;
        }
        // A voluntary `End`. Descend if there is anywhere to descend to
        // — a folded node to open, or a child to enter.
        if self.cursor_folded() || self.first_child(self.cursor).is_some() {
            self.first_child_move();
            return;
        }
        // Otherwise carry on to the first character of the next row.
        // Rightward motion at the end of a leaf's text used to stop dead
        // with "no children", which is the one place the caret could be
        // blocked while the document plainly continued below. Reading
        // right off the end of a line and arriving at the start of the
        // next is what every other text surface does, and it makes `l`
        // alone enough to walk the whole document.
        //
        // Only `step_down` moves, not `move_down`: `carry_caret` would
        // place the caret by `desired_column`, and the column being
        // aimed at here is the new row's first, not the old row's last.
        if self.step_down() {
            self.caret_to_line_start();
        }
    }

    /// Spec 0194 S6: `0`/`^`. The two are one key here — column zero is
    /// the fold gutter and unreachable (S3), so vim's two motions have
    /// the same destination. Declares the anchor (spec 0199 S1 rule 1).
    pub(super) fn caret_to_line_start(&mut self) {
        self.cursor_column = self.caret_bounds().0;
        self.desired_column = self.cursor_column;
        self.caret_anchor = CaretAnchor::Home;
    }

    /// Spec 0194 S6: `$`. Declares the anchor (spec 0199 S1 rule 1).
    pub(super) fn caret_to_line_end(&mut self) {
        self.cursor_column = self.caret_bounds().1;
        self.desired_column = self.cursor_column;
        self.caret_anchor = CaretAnchor::End;
    }

    /// Spec 0199 S8: `Alt-Left`, the command line's `Alt-b` applied to
    /// the cursor row. The word definition is shared with
    /// `command_line.rs` rather than restated — a main pane that broke
    /// words differently from the `:` prompt on the same screen would be
    /// a defect, not a feature.
    pub(super) fn caret_word_left(&mut self) {
        self.clamp_caret_column();
        let chars = self.caret_row_chars();
        self.cursor_column = prev_word_boundary(&chars, self.cursor_column);
        self.clamp_caret_column();
        self.desired_column = self.cursor_column;
        self.settle_anchor();
    }

    /// Spec 0199 S8: `Alt-Right`, the command line's `Alt-f`.
    pub(super) fn caret_word_right(&mut self) {
        self.clamp_caret_column();
        let chars = self.caret_row_chars();
        self.cursor_column = next_word_boundary(&chars, self.cursor_column);
        self.clamp_caret_column();
        self.desired_column = self.cursor_column;
        self.settle_anchor();
    }

    /// The cursor row's own text, as `char`s — the same text
    /// `caret_bounds` measures its first component from. Columns beyond
    /// it belong to the heat suffix (spec 0194 S3) and are reached by
    /// clamping, not by scanning.
    fn caret_row_chars(&self) -> Vec<char> {
        self.row_text(self.cursor_row()).chars().collect()
    }

    /// The cursor's own line as a display row. Free of any lookup: the
    /// cursor already names its node and which of that node's lines it
    /// rests on, which is exactly what spec 0222 S3 has a row carry.
    fn cursor_row(&self) -> DisplayRow {
        self.committed_row_at(self.cursor_line(), self.cursor_line_pos())
    }

    /// `self.cursor`'s own currently-displayed line — spec 0142.
    ///
    /// A node's lines are consecutive from its header, footer included
    /// (the body between them belongs to the subtree, but it is still
    /// part of the same run), so this is one addition.
    pub(super) fn cursor_line(&self) -> usize {
        self.absolute_start(self.cursor) + self.cursor_line_in_node as usize
    }

    /// The cursor as a `LinePos`, in the form `next_visible` /
    /// `prev_visible` step over.
    fn cursor_line_pos(&self) -> LinePos {
        LinePos {
            node: self.cursor,
            line_in_node: self.cursor_line_in_node,
        }
    }

    /// Whether the cursor rests on its node's own closing `}` rather
    /// than on a row the node draws its content on.
    pub(super) fn cursor_on_footer(&self) -> bool {
        self.is_footer(self.cursor_line_pos())
    }

    /// Whether `pos` names a bracketed node's closing `}` line.
    ///
    /// A flat node has no brace at all, so every one of its rows — the
    /// single row of a scalar, each element of a packed run — is a row
    /// it draws content on (spec 0216 S7).
    pub(super) fn is_footer(&self, pos: LinePos) -> bool {
        pos.line_in_node > 0 && self.tree[pos.node].is_bracketed()
    }

    /// The node whose text is drawn on `line`, if any — everything but
    /// a closing brace, which draws no content of its own.
    ///
    /// Spec 0222 S3: a drawn row already names its own line's owner, so
    /// nothing in the draw path asks a *line number* who owns it any
    /// more. What is left is the tests' own convenience.
    #[cfg(test)]
    pub(super) fn node_at_header_line(&self, line: usize) -> Option<usize> {
        let pos = self.line_pos(line)?;
        (!self.is_footer(pos)).then_some(pos.node)
    }

    /// The node whose own closing `}` sits on `line`, if any.
    #[cfg(test)]
    pub(super) fn node_at_footer_line(&self, line: usize) -> Option<usize> {
        let pos = self.line_pos(line)?;
        self.is_footer(pos).then_some(pos.node)
    }

    /// Moves the cursor to the next visible *line* (spec 0142) — a
    /// node's own closing `}` line is a distinct stop, right after its
    /// last visible descendant and right before its next sibling (or
    /// ancestor's own footer). Every visible line is a cursor stop
    /// (spec 0210 S1), so this is a single O(1) step along the
    /// visible-line sequence.
    ///
    /// Spec 0215 S1: the stepping half of `move_down` — everything
    /// except the caret fix-up. Returns whether the cursor moved.
    ///
    /// Split out so a page key pays for `carry_caret` once (S2) instead
    /// of once per row. Nothing between the steps may read
    /// `cursor_column`: `carry_caret` overwrites it from
    /// `desired_column` and `caret_anchor`, neither of which stepping
    /// touches, so the intermediate values are unobservable (S3).
    pub(super) fn step_down(&mut self) -> bool {
        let Some((next, _)) = self.next_visible(self.cursor_line_pos()) else {
            return false;
        };
        self.cursor = next.node;
        self.cursor_line_in_node = next.line_in_node;
        self.cursor_moves += 1;
        true
    }

    /// Spec 0215 S1: the stepping half of `move_up`. See `step_down`.
    pub(super) fn step_up(&mut self) -> bool {
        let Some((prev, _)) = self.prev_visible(self.cursor_line_pos()) else {
            return false;
        };
        self.cursor = prev.node;
        self.cursor_line_in_node = prev.line_in_node;
        self.cursor_moves += 1;
        true
    }

    pub(super) fn move_down(&mut self) {
        if self.step_down() {
            self.carry_caret();
        }
    }

    pub(super) fn move_up(&mut self) {
        if self.step_up() {
            self.carry_caret();
        }
    }

    /// Sibling-skip move (`J` / Shift-Down, spec 0126 G2): moves to the
    /// cursor's next sibling, or leaves it in place with a message if
    /// there isn't one.
    pub(super) fn next_sibling_move(&mut self) {
        if let Some(next) = self.next_sibling(self.cursor) {
            self.record_jump();
            self.set_cursor(next);
        } else {
            self.message = "no next sibling".to_string();
        }
    }

    /// Sibling-skip move (`K` / Shift-Up, spec 0126 G2): moves to the
    /// cursor's previous sibling, or leaves it in place with a message if
    /// there isn't one.
    pub(super) fn prev_sibling_move(&mut self) {
        if let Some(prev) = self.prev_sibling(self.cursor) {
            self.record_jump();
            self.set_cursor(prev);
        } else {
            self.message = "no previous sibling".to_string();
        }
    }

    /// Spec 0215 S2: `page` steps, then **one** `carry_caret`. One
    /// fix-up per row would walk the document from its start each time:
    /// near the end of a blob with a wide root that is the whole cost
    /// of the key, 13-23 ms against 47 µs at the top of the same
    /// document.
    ///
    /// `carry_caret` runs only if at least one step succeeded, so a page
    /// key already at a document end is an exact no-op: `carry_caret` is
    /// not idempotent on `cursor_column` when the caret is `Free` and
    /// the row is narrower than `desired_column`.
    pub(super) fn move_page_down(&mut self) {
        let page = self.document_pane_height().max(1);
        let mut moved = false;
        for _ in 0..page {
            moved |= self.step_down();
        }
        if moved {
            self.carry_caret();
        }
    }

    pub(super) fn move_page_up(&mut self) {
        let page = self.document_pane_height().max(1);
        let mut moved = false;
        for _ in 0..page {
            moved |= self.step_up();
        }
        if moved {
            self.carry_caret();
        }
    }

    /// Longest rendered line (in characters, gutter included) among the
    /// currently visible window — the basis for `pan_right`'s clamping
    /// bound (spec 0113 D24: "recomputed as the cursor/scroll position
    /// changes").
    ///
    /// Spec 0185 G4: measured over the *composed* window, so a preview
    /// overlay wider than the committed rows it stands in for can still
    /// be panned to its own right edge — the case a preview exists for,
    /// since a structurally wrong candidate is what renders wide.
    ///
    /// Resolved through `build_window`, exactly as a frame is, rather
    /// than one `display_row` per row (spec 0210 S3).
    /// `clamp_pan_offset` calls this on a fold or a commit that finds a
    /// non-zero pan, right after the `structural_version` bump that
    /// empties the window cache, and each row's content is then asked
    /// for its owner several times over, so resolving per row would cost
    /// a handful of full descents per row of the pane.
    pub(super) fn max_visible_line_len(&mut self) -> usize {
        let pane_height = self.document_pane_height();
        let total = self.composed_row_count();
        let start = self.scroll.index.min(total);
        let end = (self.scroll.index + pane_height).min(total);
        self.build_window(start, end - start)
            .into_iter()
            .map(|row| self.row_content(row).chars().count())
            .max()
            .unwrap_or(0)
    }

    /// Upper bound for `pan_offset`: the widest currently-visible row's
    /// last character stays shown, never further. Column 0 of
    /// `main_area` is always the heat-cue gutter (spec 0138 N1),
    /// reserved but never panned — only `width - 1` columns actually
    /// show line text, so the bound must leave room for that extra
    /// column or panning stops one character short of the line's true
    /// end.
    pub(super) fn max_pan_offset(&mut self) -> usize {
        let width = (self.main_area.width as usize).saturating_sub(1);
        self.max_visible_line_len().saturating_sub(width)
    }

    /// Shared horizontal-pan arithmetic behind the main pane's Ctrl-Left/
    /// Ctrl-Right (`pan_left`/`pan_right`, `PAN_STEP`) and Shift+wheel/
    /// native horizontal scroll (`wheel_pan_left`/`wheel_pan_right`,
    /// `WHEEL_PAN_STEP`) — bounded on the right by `max_pan_offset` so
    /// it stops once the rightmost character of the widest
    /// currently-visible row would be shown, never further.
    fn pan_horizontal(&mut self, step: usize, left: bool) {
        let before = self.pan_offset;
        if left {
            self.pan_offset = self.pan_offset.saturating_sub(step);
        } else {
            self.pan_offset = (self.pan_offset + step).min(self.max_pan_offset());
        }
        self.event_changed_nothing = self.pan_offset == before;
    }

    pub(super) fn pan_left(&mut self) {
        self.pan_horizontal(PAN_STEP, true);
    }

    pub(super) fn pan_right(&mut self) {
        self.pan_horizontal(PAN_STEP, false);
    }

    /// Shift+wheel/native horizontal-scroll pan over the main pane: same
    /// as `pan_left`/`pan_right` but at `WHEEL_PAN_STEP`'s finer
    /// granularity.
    pub(super) fn wheel_pan_left(&mut self) {
        self.pan_horizontal(WHEEL_PAN_STEP, true);
    }

    pub(super) fn wheel_pan_right(&mut self) {
        self.pan_horizontal(WHEEL_PAN_STEP, false);
    }

    /// Shared vertical-pan arithmetic behind the main pane's Ctrl-Up/
    /// Ctrl-Down (`pan_vertical_up`/`pan_vertical_down`, `PAN_STEP`) and
    /// plain mouse wheel (`wheel_pan_up`/`wheel_pan_down`,
    /// `WHEEL_PAN_STEP`) — scrolls the viewport without moving the
    /// cursor, bounded only by the content itself and deliberately not
    /// by the cursor's own row: the cursor may leave the view.
    ///
    /// Spec 0185 S2: bounded by the *composed* row count, so a preview
    /// overlay taller than the block it stands in for can be scrolled
    /// through in full.
    ///
    /// Spec 0230: stated in terminal rows, so that a pan started from a
    /// half-line scroll left over from a `w` toggle lands back on whole
    /// lines rather than carrying the offset forever.
    ///
    /// Spec 0244 S6: bounded by `pan_top_bounds`, which lets the pane's
    /// top edge run past either end of the content — up until one
    /// terminal row of it is left on screen.
    ///
    /// Spec 0245 S2: a pan already sitting on its bound moves nothing,
    /// and says so — a held wheel at the top of the document is the
    /// commonest way to fill the event queue with no-ops.
    fn pan_vertical(&mut self, step: usize, up: bool) {
        let heights = self.row_heights();
        let content_rows = heights.offset(self.composed_row_count());
        let (min_top, max_top) = pan_top_bounds(content_rows, self.main_area.height as usize);
        // The step is a count of *document* rows, so it is measured in
        // terminal rows over the rows it would actually cross — spec
        // 0268 S4 having made those two different numbers.
        let from = self.scroll.index;
        let step = if up {
            // Panning up past row 0 is what leaves blank rows above the
            // document (spec 0244 S6), and there are no rows there to
            // measure — they stand in for the first one, so they are
            // counted at its height.
            let missing = step.saturating_sub(from);
            heights.offset(from) - heights.offset(from - (step - missing))
                + missing * heights.height(0)
        } else {
            heights.offset(from + step) - heights.offset(from)
        } as isize;
        let top = self.scroll_top();
        let moved = if up { top - step } else { top + step };
        let landed = moved.clamp(min_top, max_top);
        self.event_changed_nothing = landed == top;
        self.set_scroll_top(landed);
    }

    pub(super) fn pan_vertical_up(&mut self) {
        self.pan_vertical(PAN_STEP, true);
    }

    pub(super) fn pan_vertical_down(&mut self) {
        self.pan_vertical(PAN_STEP, false);
    }

    /// Plain mouse-wheel vertical pan: pans the viewport, same as
    /// Ctrl-Up/Ctrl-Down but at `WHEEL_PAN_STEP`'s finer granularity.
    /// It does not move the cursor.
    pub(super) fn wheel_pan_up(&mut self) {
        self.pan_vertical(WHEEL_PAN_STEP, true);
    }

    pub(super) fn wheel_pan_down(&mut self) {
        self.pan_vertical(WHEEL_PAN_STEP, false);
    }

    /// Jump to the document-order first node (`Home`/`gg`). Must also
    /// fire when the cursor already sits on `first_node` but on its
    /// *footer* line (e.g. the root node's own closing `}`, which is
    /// `first_node`'s footer, not a distinct node) — otherwise the
    /// `self.cursor != self.first_node` check alone is falsely
    /// satisfied and the cursor stays stuck on the last line.
    pub(super) fn move_home(&mut self) {
        if self.cursor != self.first_node || self.cursor_line_in_node != 0 {
            self.record_jump();
            self.set_cursor(self.first_node);
        }
    }

    /// Jump to the document's true last visible line (`End`/`G`, spec
    /// 0142) — which may be a node's footer line, e.g. the virtual
    /// encompassing wrapper's own final `}`.
    pub(super) fn move_end(&mut self) {
        let last_row = self.visible_row_count().checked_sub(1);
        let Some((pos, _)) = last_row.and_then(|row| self.visible_row_pos(row)) else {
            return;
        };
        if self.cursor != pos.node || self.cursor_line_in_node != pos.line_in_node {
            self.record_jump();
            self.cursor = pos.node;
            self.cursor_line_in_node = pos.line_in_node;
            self.cursor_moves += 1;
            // Spec 0194 S1: a node-level jump, so the caret resets — the
            // assignment above bypasses `set_cursor` only because that
            // one always returns to the header row, and this stop may be
            // a footer line.
            self.reset_caret_column();
        }
    }

    /// Spec 0194 S6/S4: `%`. Both braces belong to the cursor node — the
    /// `{` on its header line, the `}` on its footer — so this is
    /// exactly "move to the other of the node's two rows, and put the
    /// column on the brace".
    ///
    /// A *folded* node draws both on one row (spec 0193's `{ ... }`
    /// collapse summary) and has no visible footer line at all, so there
    /// the column moves and the row stays put — which falls out of
    /// deriving the row from the target line rather than negating it.
    ///
    /// From anywhere other than the closing brace the jump goes to it,
    /// so `%` is useful from the middle of a header line and not only
    /// from the brace itself.
    pub(super) fn jump_matching_brace(&mut self) {
        let Some((open, close)) = self.cursor_brace_pair() else {
            self.message = "no matching brace here".to_string();
            return;
        };
        let (line, column) = if (self.cursor_line(), self.cursor_column) == close {
            open
        } else {
            close
        };
        self.cursor_line_in_node = (line - self.absolute_start(self.cursor)) as u32;
        self.cursor_column = column;
        self.desired_column = column;
        // Spec 0199 S10: `%` lands on a brace, and a header row's
        // opening brace *is* its last column. Anchoring `End` there
        // would make the next `l` descend, which is not what `%`
        // promised.
        self.caret_anchor = CaretAnchor::Free;
        self.cursor_moves += 1;
    }

    /// Spec 0199 S7: what `caret_left` delegates to at a voluntary
    /// `Home`. `H`/`Shift-Left` belongs to sibling folding (S9), so this
    /// has no key of its own and is reachable only through
    /// `h`/`Left`/`Backspace` — which is why the fold-first branch lives
    /// here rather than in the caller.
    ///
    /// Folds an expanded node first, and only moves to the parent once
    /// it is closed — the nvim-tree pattern.
    ///
    /// At the root with the fold already closed there is nothing left to
    /// do; it reports `no parent` rather than folding the root's
    /// siblings, which is `H`/`zC`'s job (N2).
    pub(super) fn parent_move(&mut self) {
        if self.cursor_expanded() {
            self.toggle_fold(self.cursor);
            return;
        }
        if let Some(parent) = self.parent(self.cursor) {
            self.record_jump();
            self.set_cursor(parent);
        } else {
            self.message = "no parent".to_string();
        }
    }

    /// Spec 0199 S7: what `caret_right` delegates to at a voluntary
    /// `End`. Like `parent_move` it has no key of its own, since
    /// `L`/`Shift-Right` belongs to sibling unfolding (S9).
    ///
    /// Unfolds a closed node and *stops*, so a second press descends.
    /// Unfolding and descending in one press is deliberately rejected:
    /// the split is what makes `l` at `End` and `h` at `Home` inverses,
    /// so closing a node and reopening it returns the tree *and* the
    /// cursor to where they were.
    ///
    /// The descent goes through `set_cursor`, so the caret lands on the
    /// child row's first non-blank with the anchor `Home` — the child's
    /// name, which is what the user descended to read (spec 0199 Q2).
    pub(super) fn first_child_move(&mut self) {
        if self.cursor_folded() {
            self.toggle_fold(self.cursor);
            return;
        }
        let Some(child) = self.first_child(self.cursor) else {
            self.message = "no children".to_string();
            return;
        };
        self.record_jump();
        self.set_cursor(child);
    }

    /// Folds/unfolds `idx`, and puts the cursor on it.
    ///
    /// The cursor follows because a fold is an edit to the shape of the
    /// document made *at* one node: leaving the cursor where it was
    /// makes the next keystroke act somewhere the user is not looking,
    /// and the reachable ways in — a fold-marker click, `h` at a node's
    /// `Home` — are all gestures aimed at `idx`. It subsumes spec 0142
    /// G6.2's narrower rule, which moved the cursor only when folding
    /// would have stranded it on a line about to be hidden: `idx` is
    /// the nearest still-visible ancestor of every such line, so the
    /// same move happens, for a reason that also covers the rest.
    ///
    /// Nothing pins the view, because nothing needs to. A node's own
    /// row is a count of the visible rows *before* it, and folding
    /// `idx` only ever hides lines after `idx`'s header — so `idx` is
    /// drawn on the row it was already on, and `folds_changed`'s scroll
    /// clamp finds the new cursor already in view and leaves
    /// `scroll_offset` alone.
    pub(super) fn toggle_fold(&mut self, idx: usize) {
        if !self.open(idx) {
            self.folded.insert(idx);
        }
        if idx == self.cursor {
            self.cursor_line_in_node = 0;
        } else {
            self.set_cursor(idx);
        }
        self.refresh_line_counts(idx);
        self.folds_changed();
        // Spec 0194 S11: a fold toggle rewrites the cursor row's text
        // (the `{ ... }` collapse summary is spliced into `row_text`),
        // so a caret that did not move may now point past the row's end.
        // `desired_column` is deliberately left alone.
        self.clamp_caret_column();
    }

    /// `z` — toggle the cursor node's own fold.
    ///
    /// Spec 0249 S8 needs no arm of its own here: `has_children` is
    /// `is_bracketed`, and a node a bounded render stopped at is
    /// bracketed — it emitted its header and footer, just nothing
    /// between. So a stop is foldable by the same test as any other
    /// message, and `open` is what makes the gesture render.
    pub(super) fn toggle_cursor_fold(&mut self) {
        if self.has_children(self.cursor) {
            self.toggle_fold(self.cursor);
        } else {
            self.message = "not foldable".to_string();
        }
    }

    /// `Z` — toggle the cursor node and force its whole subtree into the
    /// state the cursor node just took, so one keystroke opens or closes
    /// a message all the way down.
    ///
    /// The cursor node decides the target state for everyone, rather than
    /// each descendant toggling its own: a subtree in mixed states has no
    /// meaningful "opposite", and following the one node the user is
    /// looking at makes `Z` agree with the `z` they can see the result
    /// of.
    ///
    /// Descendants are visited deepest-first (the reverse of
    /// `collect_descendants`' pre-order, in which a parent always
    /// precedes its own descendants), because `refresh_line_counts`
    /// walks *upward* recomputing each node from its children and stops
    /// as soon as one is unchanged. Refreshing a parent before its
    /// children would propagate counts that are about to move again.
    ///
    /// The `has_children` guard is on the folding side only, for the
    /// reason `set_all_siblings_folded` gives: a leaf must never enter
    /// `folded`, since nothing would take it back out.
    pub(super) fn toggle_cursor_fold_recursive(&mut self) {
        if !self.has_children(self.cursor) {
            self.message = "not foldable".to_string();
            return;
        }
        let fold = !self.is_folded(self.cursor);
        let mut nodes = vec![self.cursor];
        self.collect_descendants(self.cursor, &mut nodes);

        let mut changed = false;
        for i in nodes.into_iter().rev() {
            let moved = if fold {
                self.has_children(i) && self.folded.insert(i)
            } else {
                // Spec 0249 S8: opening a stop renders it, one screenful
                // deep. The stops that render then leaves behind are not
                // in `nodes` and stay folded — `Z` opens what the
                // document currently has, not what it could be made to
                // have, which is the same promise it always made.
                self.open(i)
            };
            if moved {
                changed = true;
                self.refresh_line_counts(i);
            }
        }
        if changed {
            self.folds_changed();
        }
        // The cursor is already on the node that was toggled, so unlike
        // `toggle_fold` there is nobody to move — but its row's text has
        // been rewritten underneath the caret just the same (spec 0194
        // S11).
        self.cursor_line_in_node = 0;
        self.clamp_caret_column();
    }

    /// All siblings of `idx` (including `idx` itself), in document order —
    /// walks to the first sibling via `prev_sibling`, then follows
    /// `next_sibling`. Works uniformly at any level, including root-level
    /// nodes (which share sibling links despite having no `parent`).
    pub(super) fn sibling_range(&self, idx: usize) -> Vec<usize> {
        let mut first = idx;
        while let Some(p) = self.prev_sibling(first) {
            first = p;
        }
        let mut v = Vec::new();
        let mut cur = Some(first);
        while let Some(i) = cur {
            v.push(i);
            cur = self.next_sibling(i);
        }
        v
    }

    pub(super) fn fold_all_siblings(&mut self) {
        self.set_all_siblings_folded(true);
    }

    pub(super) fn unfold_all_siblings(&mut self) {
        self.set_all_siblings_folded(false);
    }

    /// Folds or unfolds every sibling of the cursor node, re-deriving
    /// line counts for each one that actually moved and re-rendering
    /// once at the end if any did.
    ///
    /// The `has_children` guard is on the folding side only, and not by
    /// oversight: a leaf must not enter `folded` (nothing would ever
    /// take it back out), while a node already *in* `folded` has
    /// children by construction, so unfolding needs no such test.
    fn set_all_siblings_folded(&mut self, fold: bool) {
        let mut changed = false;
        for i in self.sibling_range(self.cursor) {
            let moved = if fold {
                self.has_children(i) && self.folded.insert(i)
            } else {
                self.open(i)
            };
            if moved {
                changed = true;
                self.refresh_line_counts(i);
            }
        }
        if changed {
            self.folds_changed();
        }
    }

    /// Slash-separated positional path from the root to `idx` (spec 0113
    /// D25) — e.g. `/1/2/3`, each segment a `sibling_position`. No schema
    /// knowledge required, purely structural.
    ///
    /// The underlying tree's actual root is the virtual encompassing
    /// wrapper (spec 0114 §1.1); every real node's true internal path
    /// therefore has a leading `/1` leg (descent into the wrapper's sole
    /// field) that isn't part of the caller-visible protobuf. Drop it
    /// here, so the wrapper stays invisible in displayed paths; the
    /// wrapper's own node (internal path `/1`) displays as bare `/`.
    pub(super) fn positional_path(&self, idx: usize) -> String {
        let mut scratch = PathScratch::default();
        self.write_positional_path(&mut scratch, idx);
        scratch.text
    }

    /// `positional_path` into a caller-owned buffer (spec 0235 S21).
    ///
    /// A sweep tests the path of every candidate line, so at the
    /// reference corpus's 5.28 M lines a `String` and a `Vec` allocated
    /// per candidate would cost more than the matching they exist to
    /// serve. Both are cleared and rewritten instead.
    pub(super) fn write_positional_path(&self, out: &mut PathScratch, idx: usize) {
        use std::fmt::Write as _;

        out.segments.clear();
        let mut cur = Some(idx);
        while let Some(i) = cur {
            out.segments.push(self.sibling_position(i));
            cur = self.parent(i);
        }
        // The segments run leaf-to-root, so the *last* of them is the
        // virtual encompassing wrapper's own leg — the one this drops.
        out.segments.pop();
        out.text.clear();
        out.text.push('/');
        for (i, seg) in out.segments.iter().rev().enumerate() {
            if i > 0 {
                out.text.push('/');
            }
            let _ = write!(out.text, "{seg}");
        }
    }

    /// Node `idx`'s displayed byte range, half-open `[start, end)`, in the
    /// caller's original (pre-wrap) blob's numbering (spec 0114 §1.1):
    /// every node — message/group *and* scalar alike — is shown
    /// payload-only, tag (and, for length-delimited fields, the length
    /// prefix — strings, bytes, and packed-repeated scalars are all
    /// wire-type LEN, same as messages/groups) stripped via
    /// `extract::message_payload_range`, which strips generically by wire
    /// type rather than by node kind. Every coordinate also has
    /// `wrapper_offset` subtracted to undo the virtual encompassing
    /// wrapper's own tag+length prefix. The wrapper's own node displays
    /// as `[0, n)`.
    pub(super) fn display_range(&self, idx: usize) -> Range<usize> {
        let span = &self.tree[idx].span;
        let raw = extract::message_payload_range(&self.blob, &span.raw_range);
        (raw.start - self.wrapper_offset)..(raw.end - self.wrapper_offset)
    }
}
