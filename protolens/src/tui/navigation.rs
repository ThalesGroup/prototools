// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::command_line::{next_word_boundary, prev_word_boundary};
use super::*;

/// Moved to `crate::decode` by spec 0192 S1, which made `build_tree` a
/// fourth caller — the ordinal it is counted over is now derived there.
/// Re-exported here so this module's name stays the one the other tui
/// call sites use.
pub(super) use crate::decode::same_packed_record;

impl App {
    /// Whether `idx` is a bracketed node — has its own distinct header
    /// *and* footer line, so it's foldable and carries a fold marker.
    /// Not the same as `self.tree[idx].first_child.is_some()` (spec
    /// 0142 fix, 2026-07-18 feedback): an empty-but-bracketed message
    /// (decoded with zero populated fields, still rendered as `Name {`
    /// then `}` on the next line) has no children yet is still a real,
    /// two-line bracketed node — foldable (folding it just hides its
    /// own footer line, same as any node with an empty body) and
    /// entitled to a fold marker/handle like any other message node.
    pub(super) fn has_children(&self, idx: usize) -> bool {
        let span = &self.tree[idx].span;
        span.text_range.end - 1 > span.text_range.start
    }

    /// Recompute `visible_rows` from current fold state: a folded node
    /// hides its body (`text_range.start + 1 .. text_range.end`), keeping
    /// its own opening line visible with a fold indicator.
    ///
    /// Also re-clamps `pan_offset` (2026-07-24 bug report: a fold/unfold
    /// or an override splice — e.g. deactivating an override that turns
    /// a collapsed scalar back into a wide expanded message — can shrink
    /// the currently-visible content out from under a `pan_offset` that
    /// was valid for the *previous* shape. Left unclamped, every visible
    /// row then renders shorter than `pan_offset`, so `pan_spans` yields
    /// nothing for any of them and the main pane goes blank — recoverable
    /// only by panning right again, since only `pan_horizontal`'s own
    /// right-branch re-derives the clamp). Called on every caller of this
    /// method, so the pane is never left stuck blank regardless of which
    /// direction shrank the content.
    pub(super) fn rebuild_visible_rows(&mut self) {
        self.rebuild_visible_rows_from(0);
    }

    /// Spec 0186 S4: `rebuild_visible_rows` restricted to lines at or
    /// after `from`. Lines below it keep both their content and their
    /// index, so their visibility cannot have changed and the prefix of
    /// `visible_rows` describing them can simply be kept.
    ///
    /// Only `finalize_override_batch` passes a non-zero `from`; the fold
    /// path deliberately still passes `0` (spec 0186 N4).
    pub(super) fn rebuild_visible_rows_from(&mut self, from: usize) {
        let total = self.lines.len();
        // A splice can shrink the document below `from`'s own value in a
        // degenerate batch; clamping keeps the slicing below in bounds
        // and turns the whole call into "truncate to what still exists".
        let from = from.min(total);

        // `visible_rows` is sorted ascending by construction — spec 0185's
        // overlay anchor relies on that too — so the surviving prefix is
        // just a `partition_point` away, with no allocation and no move.
        let keep = self.visible_rows.partition_point(|&l| l < from);
        self.visible_rows.truncate(keep);

        // Taken out of `self` so the `self.folded`/`self.tree` reads
        // below can borrow immutably, and put back at the end. Reusing
        // the buffer across calls is what removes the per-call
        // `vec![false; total]` (193 kB on the 1.1 MB fixture).
        let mut hidden = std::mem::take(&mut self.hidden_mask);
        hidden.resize(total, false);
        // Clear exactly the range about to be marked, not the whole
        // buffer: below `from` nothing is read, and above it a stale
        // `true` left by a previous call would hide a line that is now
        // visible.
        hidden[from..total].fill(false);
        for &idx in &self.folded {
            let r = &self.tree[idx].span.text_range;
            // Clamping to the tail keeps the marking cost proportional
            // to `total - from` rather than to the folded ranges' full
            // extents, however far back they start.
            let start = (r.start + 1).max(from);
            let end = r.end.min(total);
            if start < end {
                hidden[start..end].fill(true);
            }
        }
        self.visible_rows
            .extend((from..total).filter(|&l| !hidden[l]));
        self.hidden_mask = hidden;

        self.clamp_pan_offset();
        // Spec 0164 G7: any fold/unfold or content-shape change can
        // shift rendered line numbers or invalidate prefetch
        // eligibility — bumping this makes `App::prefetch_step` notice
        // and restart its walk from scratch. A *partial* rebuild is
        // still a structural change, so this stays unconditional.
        self.structural_version += 1;
    }

    /// Clamps `pan_offset` to the current content's valid range — the
    /// same `max_pan_offset` bound `pan_horizontal`'s right-branch
    /// enforces, but applied proactively (see `rebuild_visible_rows`'s
    /// doc comment) rather than only when the user happens to pan right
    /// again.
    ///
    /// First re-syncs `scroll_offset` to the cursor's row, mirroring
    /// `render()`'s own auto-pan-into-view guard (2026-07-24 follow-up):
    /// `rebuild_visible_rows` runs mid-`splice_override`, well before
    /// the next `render()` pass would normally refresh `scroll_offset`
    /// for the new content shape. Computing `max_pan_offset` against
    /// that stale, pre-splice `scroll_offset` window — rather than the
    /// window the next render will actually show around the (possibly
    /// moved) cursor — clamps against the wrong rows, panning further
    /// left than the true content width allows.
    pub(super) fn clamp_pan_offset(&mut self) {
        if !self.tree.is_empty() {
            let pane_height = self.main_area.height as usize;
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
        let mut p = self.tree[idx].parent;
        let mut changed = false;
        while let Some(pi) = p {
            if self.folded.remove(&pi) {
                changed = true;
            }
            p = self.tree[pi].parent;
        }
        if changed {
            self.rebuild_visible_rows();
        }
    }

    /// Sets `self.cursor` and bumps `cursor_moves` — the sole mutation
    /// path for `self.cursor`, so every real cursor change (even a
    /// round trip that lands back on the same node, e.g. Down then Up)
    /// is observable via `cursor_moves`, unlike comparing `self.
    /// cursor`'s value alone against a stashed old value. Always resets
    /// `cursor_footer` to `false` (spec 0142) — every caller of this
    /// method targets a node's own header row.
    ///
    /// Spec 0194 S1: also resets the caret to the new row's first
    /// reachable column, so a node-level jump lands at the start of the
    /// row's text. Vertical movement wants the opposite rule and so
    /// deliberately does not come through here — see `carry_caret`.
    pub(crate) fn set_cursor(&mut self, idx: usize) {
        self.cursor = idx;
        self.cursor_footer = false;
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
    pub(super) fn caret_bounds(&self) -> (usize, usize) {
        if self.tree.is_empty() {
            return (0, 0);
        }
        let text = self.row_text(DisplayRow::Committed(self.cursor_line()));
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
        self.row_text(DisplayRow::Committed(self.cursor_line()))
            .chars()
            .collect()
    }

    /// `self.cursor`'s own currently-displayed line: its footer line
    /// (`text_range.end - 1`) if `cursor_footer`, else its header line
    /// (`text_range.start`) — spec 0142.
    pub(super) fn cursor_line(&self) -> usize {
        let span = &self.tree[self.cursor].span;
        if self.cursor_footer {
            span.text_range.end - 1
        } else {
            span.text_range.start
        }
    }

    /// The node whose text *starts* on `line`, if any.
    ///
    /// Spec 0188 S1: the maps are `Vec<Option<u32>>`, so a lookup is an
    /// in-bounds test and a widening. These two accessors exist so the
    /// `u32` does not leak to the half-dozen call sites that only ever
    /// want an arena index.
    pub(super) fn node_at_header_line(&self, line: usize) -> Option<usize> {
        self.line_to_node
            .get(line)
            .copied()
            .flatten()
            .map(|n| n as usize)
    }

    /// The node whose own closing `}` sits on `line`, if any.
    pub(super) fn node_at_footer_line(&self, line: usize) -> Option<usize> {
        self.footer_line_to_node
            .get(line)
            .copied()
            .flatten()
            .map(|n| n as usize)
    }

    /// Resolve a visible line back to a `(node, is_footer)` cursor stop
    /// (spec 0142) — `line_to_node` (header) checked first,
    /// `footer_line_to_node` (footer) as fallback; the two never
    /// overlap for the same line (a footer line only exists for a node
    /// with a nonempty body, so its closing line always differs from
    /// its own header line).
    fn resolve_cursor_line(&self, line: usize) -> Option<(usize, bool)> {
        if let Some(idx) = self.node_at_header_line(line) {
            return Some((idx, false));
        }
        self.node_at_footer_line(line).map(|idx| (idx, true))
    }

    /// Moves the cursor to the next/previous visible *line* (spec
    /// 0142) — a node's own closing `}` line is now a distinct stop,
    /// right after its last visible descendant and right before its
    /// next sibling (or ancestor's own footer). Walks `visible_rows`
    /// directly rather than `doc_next`/`doc_prev` node links, since
    /// footer lines aren't nodes in their own right.
    ///
    /// `visible_rows` holds every unfolded *rendered* line, and not all
    /// of them are cursor stops: a malformed record's line
    /// (`IndexSink::malformed` delegates to the text sink without
    /// pushing a `NodeSpan`) and a virtual scalar's line (Any's
    /// `type_url`, MessageSet's `type_id` — likewise span-less by spec
    /// 0110 §3) are display-only. Scan past them for the next line that
    /// does resolve, rather than testing only the immediately adjacent
    /// row: giving up after one made any such line an absorbing
    /// barrier the cursor could never cross (2026-07-25 bug, reported
    /// against an `INVALID_TAG_TYPE` line inside a mistyped preview).
    pub(super) fn move_down(&mut self) {
        let cur = self.cursor_line();
        let Ok(pos) = self.visible_rows.binary_search(&cur) else {
            return;
        };
        let next = self.visible_rows[pos + 1..]
            .iter()
            .find_map(|&line| self.resolve_cursor_line(line));
        if let Some((idx, footer)) = next {
            self.cursor = idx;
            self.cursor_footer = footer;
            self.cursor_moves += 1;
            self.carry_caret();
        }
    }

    pub(super) fn move_up(&mut self) {
        let cur = self.cursor_line();
        let Ok(pos) = self.visible_rows.binary_search(&cur) else {
            return;
        };
        let prev = self.visible_rows[..pos]
            .iter()
            .rev()
            .find_map(|&line| self.resolve_cursor_line(line));
        if let Some((idx, footer)) = prev {
            self.cursor = idx;
            self.cursor_footer = footer;
            self.cursor_moves += 1;
            self.carry_caret();
        }
    }

    /// Sibling-skip move (`J` / Shift-Down, spec 0126 G2): moves to the
    /// cursor's next sibling, or leaves it in place with a message if
    /// there isn't one.
    pub(super) fn next_sibling_move(&mut self) {
        if let Some(next) = self.tree[self.cursor].next_sibling {
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
        if let Some(prev) = self.tree[self.cursor].prev_sibling {
            self.record_jump();
            self.set_cursor(prev);
        } else {
            self.message = "no previous sibling".to_string();
        }
    }

    pub(super) fn move_page_down(&mut self) {
        let page = (self.main_area.height as usize).max(1);
        for _ in 0..page {
            self.move_down();
        }
    }

    pub(super) fn move_page_up(&mut self) {
        let page = (self.main_area.height as usize).max(1);
        for _ in 0..page {
            self.move_up();
        }
    }

    /// Longest rendered line (in characters, gutter included) among the
    /// currently visible window — the basis for `pan_right`'s clamping
    /// bound (spec 0113 D24: "recomputed as the cursor/scroll position
    /// changes").
    /// Spec 0185 G4: measured over the *composed* window, so a preview
    /// overlay whose lines are wider than the committed rows they stand
    /// in for can still be panned all the way to its own right edge —
    /// which is exactly the case a preview exists for, since a
    /// structurally wrong candidate is what renders wide.
    pub(super) fn max_visible_line_len(&self) -> usize {
        let pane_height = self.main_area.height as usize;
        let total = self.composed_row_count();
        let start = self.scroll_offset.min(total);
        let end = (self.scroll_offset + pane_height).min(total);
        (start..end)
            .filter_map(|d| self.display_row(d))
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
    fn max_pan_offset(&self) -> usize {
        let width = (self.main_area.width as usize).saturating_sub(1);
        self.max_visible_line_len().saturating_sub(width)
    }

    /// Shared horizontal-pan arithmetic behind the main pane's Ctrl-Left/
    /// Ctrl-Right (`pan_left`/`pan_right`, `PAN_STEP`) and Shift+wheel/
    /// native horizontal scroll (`wheel_pan_left`/`wheel_pan_right`,
    /// `WHEEL_PAN_STEP`, 2026-07-19 feedback) — bounded on the right by
    /// `max_pan_offset` so it stops once the rightmost character of
    /// the widest currently-visible row would be shown, never further.
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

    /// Shift+wheel/native horizontal-scroll pan over the main pane
    /// (2026-07-19 feedback): same as `pan_left`/`pan_right` but at
    /// `WHEEL_PAN_STEP`'s finer granularity.
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
    /// cursor, bounded only by the content itself, no longer by the
    /// cursor's own row (2026-07-19 feedback item 1, supersedes the
    /// 2026-07-18 "cursor must never leave view" bound).
    ///
    /// Spec 0185 S2: bounded by the *composed* row count, so a preview
    /// overlay taller than the block it stands in for can be scrolled
    /// through in full.
    fn pan_vertical(&mut self, step: usize, up: bool) {
        let height = self.main_area.height as usize;
        let max_scroll = self.composed_row_count().saturating_sub(height);
        pan_vertical_by_step(&mut self.scroll_offset, max_scroll, step, up);
    }

    pub(super) fn pan_vertical_up(&mut self) {
        self.pan_vertical(PAN_STEP, true);
    }

    pub(super) fn pan_vertical_down(&mut self) {
        self.pan_vertical(PAN_STEP, false);
    }

    /// Plain mouse-wheel vertical pan (2026-07-19 feedback item 2): the
    /// wheel now pans the viewport, same as Ctrl-Up/Ctrl-Down but at
    /// `WHEEL_PAN_STEP`'s finer granularity — it no longer moves the
    /// cursor (that was the pre-item-2 behavior, `move_up`/`move_down`).
    pub(super) fn wheel_pan_up(&mut self) {
        self.pan_vertical(WHEEL_PAN_STEP, true);
    }

    pub(super) fn wheel_pan_down(&mut self) {
        self.pan_vertical(WHEEL_PAN_STEP, false);
    }

    /// Absolute last node in document order (regardless of visibility).
    pub(super) fn last_node(&self) -> usize {
        let mut cur = self.first_node;
        while let Some(n) = self.tree[cur].doc_next {
            cur = n;
        }
        cur
    }

    /// Jump to the document-order first node (`Home`/`gg`). Must also
    /// fire when the cursor already sits on `first_node` but on its
    /// *footer* line (e.g. the root node's own closing `}`, which is
    /// `first_node`'s footer, not a distinct node) — otherwise the
    /// `self.cursor != self.first_node` check alone is falsely
    /// satisfied and the cursor stays stuck on the last line.
    pub(super) fn move_home(&mut self) {
        if self.cursor != self.first_node || self.cursor_footer {
            self.record_jump();
            self.set_cursor(self.first_node);
        }
    }

    /// Jump to the document's true last visible line (`End`/`G`, spec
    /// 0142) — `visible_rows`'s own last entry, which may be a node's
    /// footer line (e.g. the virtual encompassing wrapper's own final
    /// `}`), not just the last content node's header as before.
    pub(super) fn move_end(&mut self) {
        let Some(&last_line) = self.visible_rows.last() else {
            return;
        };
        let Some((idx, footer)) = self.resolve_cursor_line(last_line) else {
            return;
        };
        if self.cursor != idx || self.cursor_footer != footer {
            self.record_jump();
            self.cursor = idx;
            self.cursor_footer = footer;
            self.cursor_moves += 1;
            // Spec 0194 S1: a node-level jump, so the caret resets — the
            // assignment above bypasses `set_cursor` only because that
            // one always clears `cursor_footer`, and this stop may be a
            // footer line.
            self.reset_caret_column();
        }
    }

    /// Spec 0194 S6/S4: `%`. Both braces belong to the cursor node — the
    /// `{` on its header line, the `}` on its footer — so this is
    /// exactly "flip `cursor_footer`, and put the column on the brace".
    ///
    /// A *folded* node draws both on one row (spec 0193's `{ ... }`
    /// collapse summary) and has no visible footer line at all, so there
    /// the column moves and `cursor_footer` stays put — which falls out
    /// of deriving it from the target line rather than negating it.
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
        self.cursor_footer = line != self.tree[self.cursor].span.text_range.start;
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
    /// `Home`. Since spec 0199 S9 gave `H`/`Shift-Left` to sibling
    /// folding, this has no key of its own — it is reachable only
    /// through `h`/`Left`/`Backspace`, which is why the fold-first
    /// branch lives here rather than in the caller.
    ///
    /// Folds an expanded node first, and only moves to the parent once
    /// it is closed — the nvim-tree pattern spec 0194 S6 had removed.
    ///
    /// At the root with the fold already closed there is nothing left to
    /// do; it reports `no parent` rather than folding the root's
    /// siblings, which is what the pre-caret code did and is `H`/`zC`
    /// now (N2).
    pub(super) fn parent_move(&mut self) {
        if self.cursor_expanded() {
            self.toggle_fold(self.cursor);
            return;
        }
        if let Some(parent) = self.tree[self.cursor].parent {
            self.record_jump();
            self.set_cursor(parent);
        } else {
            self.message = "no parent".to_string();
        }
    }

    /// Spec 0199 S7: what `caret_right` delegates to at a voluntary
    /// `End`. Like `parent_move` it has no key of its own since S9 gave
    /// `L`/`Shift-Right` to sibling unfolding.
    ///
    /// Unfolds a closed node and *stops*, so a second press descends.
    /// This used to unfold and move in the same press, on the reasoning
    /// that a folded node's children are not on screen and there would
    /// otherwise be nothing to move to — which is answered by the fact
    /// that after the first press there is. What the split buys is that
    /// `l` at `End` and `h` at `Home` become inverses: closing a node
    /// and reopening it returns the tree *and* the cursor to where they
    /// were, which the one-press form made impossible.
    ///
    /// The descent goes through `set_cursor`, so the caret lands on the
    /// child row's first non-blank with the anchor `Home` — the child's
    /// name, which is what the user descended to read (spec 0199 Q2).
    pub(super) fn first_child_move(&mut self) {
        if self.cursor_folded() {
            self.toggle_fold(self.cursor);
            return;
        }
        let Some(child) = self.tree[self.cursor].first_child else {
            self.message = "no children".to_string();
            return;
        };
        self.record_jump();
        self.set_cursor(child);
    }

    /// Folds/unfolds `idx`. Folding hides `idx`'s whole body, including
    /// its own footer line — if the cursor was resting there
    /// (`cursor_footer`) at the moment `idx` itself gets folded, snap
    /// it back to `idx`'s header (spec 0142 G6.2), since that line is
    /// no longer visible. More generally, if the cursor was resting on
    /// any strict descendant of `idx` (reachable via a fold-marker
    /// click, not just the cursor's own node), that row also just
    /// disappeared from `visible_rows` — snap the cursor up to `idx`
    /// itself, the nearest still-visible ancestor, rather than leaving
    /// it stuck on a now-hidden node until the fold is reopened.
    pub(super) fn toggle_fold(&mut self, idx: usize) {
        if !self.folded.remove(&idx) {
            self.folded.insert(idx);
            if idx == self.cursor && self.cursor_footer {
                self.cursor_footer = false;
            } else if self.is_strict_descendant(self.cursor, idx) {
                self.cursor = idx;
                self.cursor_footer = false;
            }
        }
        self.rebuild_visible_rows();
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
        let mut p = self.tree[descendant].parent;
        while let Some(pi) = p {
            if pi == idx {
                return true;
            }
            p = self.tree[pi].parent;
        }
        false
    }

    /// All siblings of `idx` (including `idx` itself), in document order —
    /// walks to the first sibling via `prev_sibling`, then follows
    /// `next_sibling`. Works uniformly at any level, including root-level
    /// nodes (which share sibling links despite having no `parent`).
    pub(super) fn sibling_range(&self, idx: usize) -> Vec<usize> {
        let mut first = idx;
        while let Some(p) = self.tree[first].prev_sibling {
            first = p;
        }
        let mut v = Vec::new();
        let mut cur = Some(first);
        while let Some(i) = cur {
            v.push(i);
            cur = self.tree[i].next_sibling;
        }
        v
    }

    pub(super) fn fold_all_siblings(&mut self) {
        let siblings = self.sibling_range(self.cursor);
        let mut changed = false;
        for i in siblings {
            if self.has_children(i) && self.folded.insert(i) {
                changed = true;
            }
        }
        if changed {
            self.rebuild_visible_rows();
        }
    }

    pub(super) fn unfold_all_siblings(&mut self) {
        let siblings = self.sibling_range(self.cursor);
        let mut changed = false;
        for i in siblings {
            if self.folded.remove(&i) {
                changed = true;
            }
        }
        if changed {
            self.rebuild_visible_rows();
        }
    }

    /// 1-based ordinal position of `idx` among its own parent's direct
    /// children (or among root-level siblings, if `idx` has no parent —
    /// root-level nodes are sibling-linked despite having no `parent`, see
    /// D16), in document order (spec 0113 D25).
    ///
    /// Ordinals count wire *records*, not nodes (spec 0184 S2): a packed
    /// run's N element `NodeSpan`s (spec 0115) are one record, so they
    /// all share one ordinal and the sibling after the run is numbered
    /// one past it, whatever N is. Without this, applying an override to
    /// a run — which collapses it to a single node
    /// (`splice_override`'s `siblings[0]` merge) — would renumber every
    /// later sibling and silently re-point paths recorded beforehand.
    ///
    /// Spec 0192 S1: this used to walk the `prev_sibling` chain, making
    /// it O(position among siblings) and `positional_path` O(document)
    /// deep in a wide message — which `render` paid once per drawn row.
    /// The count is now derived once, where the chain is built.
    pub(super) fn sibling_position(&self, idx: usize) -> usize {
        self.tree[idx].sibling_ordinal as usize
    }

    /// Slash-separated positional path from the root to `idx` (spec 0113
    /// D25) — e.g. `/1/2/3`, each segment a `sibling_position`. No schema
    /// knowledge required, purely structural.
    ///
    /// The underlying tree's actual root is the virtual encompassing
    /// wrapper (spec 0114 §1.1); every real node's true internal path
    /// therefore has a leading `/1` leg (descent into the wrapper's sole
    /// field) that isn't part of the caller-visible protobuf. Drop it here
    /// so displayed paths match exactly what they were before the wrapper
    /// existed; the wrapper's own node (internal path `/1`) displays as
    /// bare `/`.
    pub(super) fn positional_path(&self, idx: usize) -> String {
        let mut segments = Vec::new();
        let mut cur = Some(idx);
        while let Some(i) = cur {
            segments.push(self.sibling_position(i));
            cur = self.tree[i].parent;
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
        let raw =
            extract::message_payload_range(&self.blob, &span.raw_range, span.packed_record_start);
        (raw.start - self.wrapper_offset)..(raw.end - self.wrapper_offset)
    }
}
