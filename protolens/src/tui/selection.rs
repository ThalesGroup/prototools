// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0242: the main-pane selection.
//!
//! One model behind both the mouse and the `Shift`-motions: a fixed end
//! in `select_anchor` and a moving end that *is* the caret. Everything
//! here follows from that — the mouse cannot select something the keys
//! could not, because both of them select by moving the caret.

use super::*;

/// The two ends of a selection, resolved to absolute lines and ordered.
/// `(start_line, start_column, end_line, end_column)`, half-open at the
/// end, so the span is never empty: the narrowest one is a single
/// character, and "nothing selected" is [`App::selection_span`]
/// returning `None`.
pub(super) type SelectionSpan = (usize, usize, usize, usize);

impl App {
    /// Spec 0242 S1/S2: the selection's two ends, in document order, or
    /// `None` when no selection is engaged.
    ///
    /// **Both endpoint cells are inside the span**, which is why the end
    /// column comes back one past the caret's. The caret here is vim's
    /// block, resting *on* a character rather than between two of them
    /// (`caret_bounds` stops it at `len - 1`), so a span that excluded
    /// its cell would be describing something the user cannot see — and
    /// the document's very last character would be unselectable, there
    /// being no column past it to move the caret to.
    ///
    /// The caret resting *on* the anchor is therefore a one-character
    /// selection, not an empty one — `Shift-Right` `Shift-Left` is how
    /// the keyboard asks for a single character, and dragging out and
    /// back is how the mouse does. What distinguishes it from "nothing
    /// selected" is `select_engaged`: a bare click arms the anchor
    /// without engaging, so that a click still selects nothing.
    pub(super) fn selection_span(&self) -> Option<SelectionSpan> {
        let anchor = self.select_anchor.filter(|_| self.select_engaged)?;
        let a = (
            self.absolute_start(anchor.node) + anchor.line_in_node as usize,
            anchor.column,
        );
        let b = (self.cursor_line(), self.cursor_column);
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        Some((lo.0, lo.1, hi.0, hi.1 + 1))
    }

    /// Which columns of absolute line `line` are selected, if any. The
    /// answer for a row strictly inside the span is "all of it", which
    /// the caller renders or copies as the row's full width.
    ///
    /// `text_chars` is that row's own character count, used only to
    /// bound an end that is past it — the caret track extends into the
    /// heat suffix (spec 0194 S1), and a selection stops at the text.
    pub(super) fn selected_columns(
        &self,
        span: SelectionSpan,
        line: usize,
        text_chars: usize,
    ) -> Option<std::ops::Range<usize>> {
        let (lo_line, lo_col, hi_line, hi_col) = span;
        if line < lo_line || line > hi_line {
            return None;
        }
        let start = if line == lo_line { lo_col } else { 0 };
        let end = if line == hi_line { hi_col } else { text_chars };
        let (start, end) = (start.min(text_chars), end.min(text_chars));
        (start < end).then_some(start..end)
    }

    /// Spec 0242 S3: forget the selection. Called by every main-pane key
    /// that is not one of the four selection keys, so that a plain
    /// motion does not drag the selection along behind the caret.
    pub(super) fn clear_selection(&mut self) {
        self.select_anchor = None;
        self.select_engaged = false;
    }

    /// Spec 0242 S4's "anchor if unanchored": the first `Shift`-motion
    /// pins the selection where the caret already was; later ones only
    /// move the caret. Either way the motion engages the selection — a
    /// `Shift`-key is the user saying so, unlike the bare click that
    /// arms an anchor and selects nothing.
    fn anchor_selection(&mut self) {
        if self.select_anchor.is_none() {
            self.select_anchor = Some(self.cursor_pos());
        }
        self.select_engaged = true;
    }

    /// Spec 0242 S4/S5: extend the selection by running `motion`, then
    /// bring the caret back on screen (S7).
    fn extend_selection(&mut self, motion: impl FnOnce(&mut Self)) {
        self.anchor_selection();
        motion(self);
        self.show_caret();
    }

    pub(super) fn select_up(&mut self) {
        self.extend_selection(Self::move_up);
    }

    pub(super) fn select_down(&mut self) {
        self.extend_selection(Self::move_down);
    }

    pub(super) fn select_left(&mut self) {
        self.extend_selection(Self::selection_caret_left);
    }

    pub(super) fn select_right(&mut self) {
        self.extend_selection(Self::selection_caret_right);
    }

    /// Spec 0242 S5's horizontal motion: one character, wrapping onto
    /// the neighboring row at either end.
    ///
    /// Deliberately not `caret_left`, which at a *voluntary* `Home`
    /// folds the cursor node before climbing to its parent (spec 0199
    /// S5). A selection motion changes nothing but the caret (S6): a
    /// fold would hide rows the user has already selected, and an
    /// unfold would reveal rows they had not.
    fn selection_caret_left(&mut self) {
        self.clamp_caret_column();
        let first = self.caret_bounds().0;
        if self.cursor_column > first {
            self.cursor_column -= 1;
        } else if self.step_up() {
            self.caret_to_line_end();
        }
        self.desired_column = self.cursor_column;
        self.caret_anchor = CaretAnchor::Free;
    }

    /// The mirror of [`Self::selection_caret_left`]. A folded node is
    /// one visible row like any other: the caret steps over it onto the
    /// next visible row rather than opening it (S6).
    fn selection_caret_right(&mut self) {
        self.clamp_caret_column();
        let last = self.caret_bounds().1;
        if self.cursor_column < last {
            self.cursor_column += 1;
        } else if self.step_down() {
            self.cursor_column = self.caret_bounds().0;
        }
        self.desired_column = self.cursor_column;
        self.caret_anchor = CaretAnchor::Free;
    }

    /// Spec 0242 S7: bring the caret on screen, vertically and
    /// horizontally, moving the view as little as possible.
    pub(super) fn show_caret(&mut self) {
        let row = self.cursor_display_row();
        self.clamp_scroll_to_cursor(row);
        self.pan_to_caret();
    }

    /// The horizontal half of S7. Minimum movement, not a centering:
    /// `center_columns` (`search.rs`) centers because a search jump
    /// lands somewhere the reader has not been, whereas extending a
    /// selection by one character should move the view by one column.
    pub(super) fn pan_to_caret(&mut self) {
        let usable = (self.main_area.width as usize)
            .saturating_sub(render::HEAT_FIELD_WIDTH)
            .saturating_sub(render::FOLD_FIELD_WIDTH);
        if usable == 0 {
            return;
        }
        let column = self.cursor_column;
        if column < self.pan_offset {
            self.pan_offset = column;
        } else if column >= self.pan_offset + usable {
            let want = column + 1 - usable;
            self.pan_offset = want.min(self.max_pan_offset());
        }
    }

    /// Spec 0242 S12: the selected characters — the first row from its
    /// column to its end, whole rows in between, the last row up to its
    /// column — joined with `\n`, alongside how many lines they span.
    /// `None` if there is no active selection.
    ///
    /// **The walk is fold-blind**, and deliberately so (S6): an absolute
    /// line is a line of the *whole* document — `absolute_start` sums
    /// `lines_total`, not `lines_visible` — so stepping from one
    /// endpoint to the other with `next_line` visits the rows a fold is
    /// hiding as well as the ones on screen. A selection dragged across
    /// a collapsed message copies the message, not a placeholder.
    ///
    /// Which is why the text comes from `row_text_of(row, None)` rather
    /// than `row_text`: the latter splices spec 0193's `" ... }"`
    /// collapse summary into a folded node's header, and here the hidden
    /// body follows on the next lines, so the summary would both
    /// duplicate it and put a `...` in the clipboard. Passing no owner
    /// says "this row is document text, not a drawn row", which is
    /// exactly the case.
    ///
    /// `row_text_of`, not `row_content`: spec 0193 S1's fold margin is
    /// gutter furniture, and a `▼` (or the two blank columns that stand
    /// in for one) pasted into a `.textproto` would not parse.
    ///
    /// One descent to enter the document and then a step per row (spec
    /// 0222 S3), rather than a descent per selected line.
    pub(super) fn selected_text(&mut self) -> Option<(usize, String)> {
        let span = self.selection_span()?;
        let (lo, _, hi, _) = span;
        let mut rows = Vec::with_capacity(hi - lo + 1);
        let mut at = self.line_pos(lo);
        let mut cursor = None;
        let mut line = lo;
        while line <= hi {
            let Some(pos) = at else { break };
            let offset = self.advance_offset(&mut cursor, pos);
            let row = DisplayRow::Committed(CommittedRow { line, pos, offset });
            let chars: Vec<char> = self.row_text_of(row, None).chars().collect();
            // Only the two endpoint rows are cut; everything between
            // them is whole, hidden rows included.
            match self.selected_columns(span, line, chars.len()) {
                Some(range) => rows.push(chars[range].iter().collect::<String>()),
                None => rows.push(String::new()),
            }
            at = self.next_line(pos);
            line += 1;
        }
        Some((hi - lo + 1, rows.join("\n")))
    }

    /// Spec 0129 §G2/0131 §G2: copy the selection to the OS clipboard.
    /// No-op if there is no active selection. `copy_to_clipboard` always
    /// attempts an OSC 52 fallback when `arboard` fails (no reliable ack
    /// from the terminal either way), so a failure here still reports an
    /// (optimistic) success message rather than "clipboard unavailable"
    /// — spec 0131 §G2's "safest default."
    ///
    /// Spec 0242 S13: a selection inside one line is reported in
    /// characters. "1 line(s) copied" is a lie about half a line, and
    /// this message is the only sign the copy happened at all.
    pub(super) fn copy_selection_to_clipboard(&mut self) {
        let Some((count, text)) = self.selected_text() else {
            return;
        };
        let what = match count {
            1 => format!("{} character(s)", text.chars().count()),
            n => format!("{n} line(s)"),
        };
        self.message = match copy_to_clipboard(&text) {
            Ok(()) => format!("{what} copied to clipboard"),
            Err(_) => format!("{what} copied to clipboard (OSC 52 fallback)"),
        };
    }

    /// Spec 0131 §G1: `Ctrl-C` — copies the active selection if one
    /// exists, else the cursor's own whole current line.
    ///
    /// The test is `selection_span`, not `select_anchor`: a bare click
    /// arms an anchor without engaging a selection (S10), and a click
    /// followed by `Ctrl-c` copies the clicked line — the same thing
    /// `Ctrl-c` does when no click happened at all.
    pub(super) fn copy_current_selection_or_line(&mut self) {
        if self.selection_span().is_some() {
            self.copy_selection_to_clipboard();
            return;
        }
        let line = self.cursor_line();
        let Some(pos) = self.line_pos(line) else {
            return;
        };
        let mut cursor = None;
        let offset = self.advance_offset(&mut cursor, pos);
        let text = self.row_text(DisplayRow::Committed(CommittedRow { line, pos, offset }));
        self.message = match copy_to_clipboard(&text) {
            Ok(()) => "1 line(s) copied to clipboard".to_string(),
            Err(_) => "1 line(s) copied to clipboard (OSC 52 fallback)".to_string(),
        };
    }
}
