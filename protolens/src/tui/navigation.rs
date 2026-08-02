// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::command_line::{next_word_boundary, prev_word_boundary};
use super::*;

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
    /// How many *document* lines the main pane can show.
    ///
    /// Spec 0225 S8: in wire mode each window entry draws two terminal
    /// rows, so the pane holds half as many lines. Every scroll
    /// computation is stated in document lines, so they all come here
    /// rather than reading `main_area.height` themselves. The side
    /// panes are unaffected and keep reading their own heights.
    pub(super) fn document_pane_height(&self) -> usize {
        let height = self.main_area.height as usize;
        if self.wire {
            (height / 2).max(1)
        } else {
            height
        }
    }

    pub(super) fn clamp_pan_offset(&mut self) {
        if !self.tree.is_empty() {
            let pane_height = self.document_pane_height();
            let cursor_row = self.cursor_display_row();
            if self.last_cursor_row != Some(cursor_row) {
                clamp_scroll_to_visible(&mut self.scroll_offset, cursor_row, pane_height);
                self.last_cursor_row = Some(cursor_row);
            }
        }
        self.pan_offset = self.pan_offset.min(self.max_pan_offset());
    }

    /// Unfold every ancestor of `idx`, so it becomes visible.
    pub(super) fn unfold_ancestors(&mut self, idx: usize) {
        let mut p = self.parent(idx);
        let mut changed = false;
        while let Some(pi) = p {
            if self.folded.remove(&pi) {
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
        self.has_children(self.cursor) && !self.folded.contains(&self.cursor)
    }

    /// The cursor node is currently folded — the state in which a
    /// rightward key unfolds instead of moving (spec 0199 G4).
    fn cursor_folded(&self) -> bool {
        self.folded.contains(&self.cursor)
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
        self.first_child_move();
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
    fn step_down(&mut self) -> bool {
        let Some((next, _)) = self.next_visible(self.cursor_line_pos()) else {
            return false;
        };
        self.cursor = next.node;
        self.cursor_line_in_node = next.line_in_node;
        self.cursor_moves += 1;
        true
    }

    /// Spec 0215 S1: the stepping half of `move_up`. See `step_down`.
    fn step_up(&mut self) -> bool {
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
    /// `clamp_pan_offset` calls this on every fold and every commit,
    /// right after the `structural_version` bump that empties the window
    /// cache, and each row's content is then asked for its owner several
    /// times over, so resolving per row would cost a handful of full
    /// descents per row of the pane.
    pub(super) fn max_visible_line_len(&mut self) -> usize {
        let pane_height = self.document_pane_height();
        let total = self.composed_row_count();
        let start = self.scroll_offset.min(total);
        let end = (self.scroll_offset + pane_height).min(total);
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
    fn max_pan_offset(&mut self) -> usize {
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
        if left {
            self.pan_offset = self.pan_offset.saturating_sub(step);
        } else {
            self.pan_offset = (self.pan_offset + step).min(self.max_pan_offset());
        }
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
    fn pan_vertical(&mut self, step: usize, up: bool) {
        let height = self.document_pane_height();
        let max_scroll = self.composed_row_count().saturating_sub(height);
        pan_by_step_clamped(&mut self.scroll_offset, max_scroll, step, up);
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

    /// Folds/unfolds `idx`. Folding hides `idx`'s whole body, including
    /// its own footer line, so a cursor resting there snaps back to
    /// `idx`'s header (spec 0142 G6.2). More generally a cursor on any
    /// strict descendant of `idx` — reachable via a fold-marker click,
    /// not just the cursor's own node — snaps up to `idx`, the nearest
    /// still-visible ancestor, rather than being stranded on a hidden
    /// node until the fold is reopened.
    pub(super) fn toggle_fold(&mut self, idx: usize) {
        if !self.folded.remove(&idx) {
            self.folded.insert(idx);
            if idx == self.cursor {
                self.cursor_line_in_node = 0;
            } else if self.is_strict_descendant(self.cursor, idx) {
                self.cursor = idx;
                self.cursor_line_in_node = 0;
            }
        }
        self.refresh_line_counts(idx);
        self.folds_changed();
        // Spec 0194 S11: a fold toggle rewrites the cursor row's text
        // (the `{ ... }` collapse summary is spliced into `row_text`),
        // so a caret that did not move may now point past the row's end.
        // `desired_column` is deliberately left alone.
        self.clamp_caret_column();
    }

    /// Spec 0194 S6: `za`/`Space` — toggle the cursor node's own fold.
    pub(super) fn toggle_cursor_fold(&mut self) {
        if self.has_children(self.cursor) {
            self.toggle_fold(self.cursor);
        } else {
            self.message = "not foldable".to_string();
        }
    }

    /// Spec 0194 S6: `zc` — close the cursor node's fold, a no-op when
    /// it is already closed (vim's `zc` errors there; a reading tool has
    /// nothing useful to say about it).
    pub(super) fn fold_cursor(&mut self) {
        if !self.has_children(self.cursor) {
            self.message = "not foldable".to_string();
        } else if !self.folded.contains(&self.cursor) {
            self.toggle_fold(self.cursor);
        }
    }

    /// Spec 0194 S6: `zo` — open the cursor node's fold.
    pub(super) fn unfold_cursor(&mut self) {
        if !self.has_children(self.cursor) {
            self.message = "not foldable".to_string();
        } else if self.folded.contains(&self.cursor) {
            self.toggle_fold(self.cursor);
        }
    }

    /// Spec 0194 S6: `zA` — the sibling-wide counterpart of `za`.
    /// Follows the cursor node's own state so the two stay predictable:
    /// whatever `za` would do here, `zA` does to the whole level.
    pub(super) fn toggle_all_siblings(&mut self) {
        if self.folded.contains(&self.cursor) {
            self.unfold_all_siblings();
        } else {
            self.fold_all_siblings();
        }
    }

    /// True if `idx` is a strict ancestor of `descendant` (i.e.
    /// `descendant` != `idx` but is reachable by following `parent`
    /// links from `descendant`).
    fn is_strict_descendant(&self, descendant: usize, idx: usize) -> bool {
        let mut p = self.parent(descendant);
        while let Some(pi) = p {
            if pi == idx {
                return true;
            }
            p = self.parent(pi);
        }
        false
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
                self.folded.remove(&i)
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
        let mut segments = Vec::new();
        let mut cur = Some(idx);
        while let Some(i) = cur {
            segments.push(self.sibling_position(i));
            cur = self.parent(i);
        }
        segments.reverse();
        segments.remove(0);
        let mut path = String::from("/");
        for (i, seg) in segments.iter().enumerate() {
            if i > 0 {
                path.push('/');
            }
            path.push_str(&seg.to_string());
        }
        path
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
