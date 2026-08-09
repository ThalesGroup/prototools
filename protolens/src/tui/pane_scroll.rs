// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0244 S1: one vertical viewport, shared by the three pannable
//! panes (main, override, manage).

use std::ops::Range;

use super::{App, DisplayRow};

/// How tall each content row is, in terminal rows (spec 0268 S4).
///
/// Every row is one terminal row except those in `tall`, which are two —
/// the wire row (spec 0225) is drawn under them. Spec 0268 N1's
/// single-span rule is what keeps this three numbers instead of a set:
/// the map from a content row to the terminal row it starts on is
/// arithmetic, and invertible in the same form.
///
/// The side panes have no wire rows, so they pass `RowHeights::default()`
/// and every method below degenerates to the identity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct RowHeights {
    /// The content rows drawn two terminal rows tall. Empty means none.
    tall: Range<usize>,
}

impl RowHeights {
    /// The rows in `tall` are two terminal rows; everything else is one.
    pub(super) fn new(tall: Range<usize>) -> Self {
        Self {
            tall: if tall.is_empty() { 0..0 } else { tall },
        }
    }

    /// The terminal row that content row `row` starts on, counted from
    /// the top of content row 0.
    pub(super) fn offset(&self, row: usize) -> usize {
        row + (row.clamp(self.tall.start, self.tall.end) - self.tall.start)
    }

    /// How many terminal rows content row `row` occupies.
    pub(super) fn height(&self, row: usize) -> usize {
        if self.tall.contains(&row) {
            2
        } else {
            1
        }
    }

    /// `offset`'s inverse: the content row containing terminal row
    /// `offset`, and how far into it that row is.
    pub(super) fn row_at(&self, offset: usize) -> (usize, usize) {
        if offset < self.tall.start {
            return (offset, 0);
        }
        let past = offset - self.tall.start;
        let span = self.tall.len() * 2;
        if past < span {
            (self.tall.start + past / 2, past % 2)
        } else {
            (self.tall.end + (past - span), 0)
        }
    }

    /// How many content rows starting at `start` it takes to *cover*
    /// `terminal_rows` — a partial row at the end counts, because it is
    /// still drawn.
    pub(super) fn rows_to_cover(&self, start: usize, terminal_rows: usize) -> usize {
        let (row, part) = self.row_at(self.offset(start) + terminal_rows);
        (row + usize::from(part > 0)).saturating_sub(start)
    }

    /// How many content rows starting at `start` fit *whole* within
    /// `terminal_rows`.
    pub(super) fn rows_fitting(&self, start: usize, terminal_rows: usize) -> usize {
        self.row_at(self.offset(start) + terminal_rows)
            .0
            .saturating_sub(start)
    }
}

/// A pane's vertical viewport: the first content row it draws, plus the
/// signed remainder in terminal rows.
///
/// Spec 0230 introduced the pair for the main pane, where a content row
/// is a *document line* and in wire mode is two terminal rows thick —
/// too coarse a unit to hold a row still across a `w` toggle. `skip` is
/// the remainder: positive means that many terminal rows of the row at
/// `index` are cut off the top of the pane, negative means that many
/// blank rows are drawn above the content's first row.
///
/// The pair is always normalized so that the two together name the same
/// terminal row (`top`), which leaves `skip` in `0..row_height` except at
/// the very top, where there is no whole row left to borrow from and it
/// goes negative.
///
/// The side panes' rows are one terminal row tall, so for them the pair
/// degenerates to "row index, plus blank rows above" — but it is the same
/// value, with the same one writer, so the resets and the window
/// arithmetic are shared rather than mirrored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PaneScroll {
    /// The first content row drawn, in the pane's own row units.
    pub(super) index: usize,
    /// The signed remainder, in terminal rows. See the type's docs.
    pub(super) skip: isize,
}

impl PaneScroll {
    /// The top edge of the viewport, in terminal rows counted from the
    /// top of content row 0. Negative means blank rows above it.
    pub(super) fn top(&self, heights: &RowHeights) -> isize {
        heights.offset(self.index) as isize + self.skip
    }

    /// Puts the viewport's top edge at `top`, splitting it back into the
    /// whole row and the remainder. The only writer of the pair.
    pub(super) fn set_top(&mut self, top: isize, heights: &RowHeights) {
        if top < 0 {
            self.index = 0;
            self.skip = top;
        } else {
            let (index, skip) = heights.row_at(top as usize);
            self.index = index;
            self.skip = skip as isize;
        }
    }

    /// What a renderer draws: how many blank rows to prepend, and which
    /// content rows to draw after them.
    ///
    /// `pane_height` is in terminal rows. The range is netted against
    /// `skip` — a viewport that starts half way down a row needs one more
    /// row to fill the same pane, and one that starts above the content
    /// needs fewer — and rounded *up*, because a partial row at either
    /// end is still drawn.
    pub(super) fn window(
        &self,
        pane_height: usize,
        heights: &RowHeights,
        total: usize,
    ) -> (usize, Range<usize>) {
        let terminal_rows = (pane_height as isize + self.skip).max(0) as usize;
        let rows = heights.rows_to_cover(self.index, terminal_rows);
        let start = self.index.min(total);
        let end = (self.index + rows).min(total);
        (self.skip.min(0).unsigned_abs(), start..end)
    }
}

/// Every content row one terminal row tall: the side panes always, and
/// the main pane whenever no bytes are shown.
pub(super) const FLAT_ROWS: RowHeights = RowHeights { tall: 0..0 };

/// Which of a node's own rows an anchor names, in a form that survives
/// the node's row count changing under it (spec 0259 S2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AnchorLine {
    /// Counted from the node's header: a bracketed node's header, or any
    /// row of a flat node, whose count a splice does not change.
    FromStart(u32),
    /// A bracketed node's closing brace. It lives at `lines_total - 1`,
    /// so its `line_in_node` is stale the moment the body it closes is
    /// baked in — stored as "the last one" instead, and resolved against
    /// whatever the count has become.
    Footer,
}

/// Spec 0268 S1: the one run of rows currently showing their bytes.
///
/// Held as a pair of nodes rather than a pair of line numbers for the
/// same reason [`ScrollAnchor`] is: a splice renumbers every line below
/// it, so a span kept as numbers slides off the rows it was pointing at.
/// [`AnchorLine::Footer`] does the other half — it means "the node's
/// last line, whatever the count has become", which is what lets a `W`
/// at the root keep covering the lines a bake has not produced yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WireSpan {
    pub(super) first: WireAnchor,
    pub(super) last: WireAnchor,
}

/// One end of a [`WireSpan`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WireAnchor {
    pub(super) node: usize,
    pub(super) line: AnchorLine,
}

/// A [`WireSpan`] resolved to visible rows, and the structure it was
/// resolved against.
///
/// Resolving costs two descents and the geometry asks for the answer
/// several times a frame, so `App` keeps the last one. The two questions
/// this answers are deliberately separate fields rather than nested
/// `Option`s: whether an answer exists at all is the *cache's* business,
/// while `rows` being `None` is a real answer — the span's ends are
/// currently folded away, and no row shows its bytes.
#[derive(Clone, Debug)]
pub(super) struct WireRowCache {
    pub(super) version: u64,
    pub(super) rows: Option<Range<usize>>,
}

/// The main pane's viewport, expressed against the document's structure
/// rather than against its row numbering (spec 0259 S1).
///
/// A splice renumbers every row below it, so a viewport held as a row
/// number moves the content out from under the reader. Held as a node it
/// does not: the arena is immutable (spec 0216), so a slot index stays
/// valid for the life of the document and nothing has to invalidate this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ScrollAnchor {
    node: usize,
    line: AnchorLine,
    /// `PaneScroll.skip` as it was, so a wire-mode viewport that starts
    /// half way down a row lands back half way down it.
    skip: isize,
}

impl App {
    /// Spec 0259 S1: remember which node the pane's top row belongs to,
    /// so that the next splice can put that row back where it was.
    ///
    /// Read off the window the frame has already built — `build_window`
    /// resolves every row to its owning node (spec 0222 S3), so the top
    /// one is in hand and this costs no descent.
    ///
    /// Spec 0259 S5: not while a preview overlay is held. With an
    /// overlay a display row and a committed visible row are different
    /// numbers, and the restore answers in the second. No anchor leaves
    /// the pre-spec behavior, which is what the overlay had anyway.
    pub(super) fn capture_scroll_anchor(&mut self, window: &[DisplayRow]) {
        self.scroll_anchor = None;
        if self.preview_overlay.is_some() {
            return;
        }
        let Some(&DisplayRow::Committed(c)) = window.first() else {
            return;
        };
        self.scroll_anchor = Some(ScrollAnchor {
            node: c.pos.node,
            line: if self.is_footer(c.pos) {
                AnchorLine::Footer
            } else {
                AnchorLine::FromStart(c.pos.line_in_node)
            },
            skip: self.scroll.skip,
        });
    }

    /// Spec 0259 S3: put the anchored row back on the terminal row it was
    /// drawn on.
    ///
    /// Called at the top of `finalize_override_batch`, before the pan
    /// clamp — with the viewport already restored the clamp finds the
    /// caret inside the pane and moves nothing, so the two compose
    /// instead of fighting. The other order undoes this and leaves the
    /// caret off screen.
    pub(super) fn restore_scroll_anchor(&mut self) {
        let Some(anchor) = self.scroll_anchor else {
            return;
        };
        // Spec 0259 S4: an `FqdnField` override can flatten this node's
        // parent to `bytes`, leaving the node with no row of its own.
        // The root is always rendered, so the climb terminates.
        let mut node = anchor.node;
        let mut climbed = false;
        while self.tree[node].lines_total == 0 {
            match self.parent(node) {
                Some(p) => {
                    node = p;
                    climbed = true;
                }
                None => return,
            }
        }
        // A climb lands on the node that swallowed the anchor, and the
        // row within it that the anchor named is gone with the rendering
        // it was part of — so the ancestor's own header is what is left
        // to aim at.
        let line_in_node = match anchor.line {
            _ if climbed => 0,
            AnchorLine::Footer => self.tree[node].lines_total.saturating_sub(1),
            AnchorLine::FromStart(k) => k.min(self.tree[node].lines_total.saturating_sub(1)),
        };
        let line = self.absolute_start(node) + line_in_node as usize;
        let Some(row) = self.visible_row_of_line(line) else {
            return;
        };
        let top = self.row_heights().offset(row) as isize + anchor.skip;
        self.set_scroll_top(top);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No wire rows anywhere: every content row is one terminal row.
    fn flat() -> RowHeights {
        RowHeights::default()
    }

    /// Wire rows under the whole document, which is what the pre-0268
    /// `row_height() == 2` meant.
    fn all_tall() -> RowHeights {
        RowHeights::new(0..1000)
    }

    /// `set_top` and `top` are inverses at every offset, in both row
    /// heights, on both sides of zero — the invariant every other method
    /// here assumes.
    #[test]
    fn set_top_and_top_round_trip() {
        for heights in [flat(), all_tall(), RowHeights::new(3..6)] {
            for top in -8isize..=16 {
                let mut scroll = PaneScroll::default();
                scroll.set_top(top, &heights);
                assert_eq!(scroll.top(&heights), top, "{heights:?}, top {top}");
                if top >= 0 {
                    assert!(scroll.skip >= 0, "only a negative top borrows: {scroll:?}");
                    assert!(
                        scroll.skip < heights.height(scroll.index) as isize,
                        "skip must stay under its own row: {scroll:?} in {heights:?}",
                    );
                } else {
                    assert_eq!(scroll.index, 0, "nothing above row 0 to name: {scroll:?}");
                }
            }
        }
    }

    /// Spec 0268 S4: the map is a bijection on terminal rows at every
    /// span position, the empty span included.
    #[test]
    fn offset_and_row_at_round_trip() {
        let spans = [
            0..0,
            0..1,
            0..12,
            3..4,
            3..7,
            7..8,
            11..12,
            11..40,
            39..40,
            40..41,
        ];
        for span in spans {
            let heights = RowHeights::new(span.clone());
            let mut expected = 0;
            for row in 0..24 {
                assert_eq!(heights.offset(row), expected, "{span:?} row {row}");
                for part in 0..heights.height(row) {
                    assert_eq!(
                        heights.row_at(expected + part),
                        (row, part),
                        "{span:?} row {row} part {part}",
                    );
                }
                expected += heights.height(row);
            }
        }
    }

    /// A viewport sitting exactly on a row draws no blank rows and
    /// exactly as many content rows as fit.
    #[test]
    fn window_on_a_whole_row() {
        let scroll = PaneScroll { index: 3, skip: 0 };
        assert_eq!(scroll.window(5, &flat(), 100), (0, 3..8));
        assert_eq!(scroll.window(6, &all_tall(), 100), (0, 3..6));
    }

    /// A positive skip cuts the top row in half, so one more content row
    /// is needed to fill the same pane.
    #[test]
    fn window_with_a_partial_row_at_the_top() {
        let scroll = PaneScroll { index: 3, skip: 1 };
        assert_eq!(scroll.window(6, &all_tall(), 100), (0, 3..7));
    }

    /// A negative skip is blank rows above the content, and that many
    /// fewer content rows drawn.
    #[test]
    fn window_with_blank_rows_above() {
        let scroll = PaneScroll { index: 0, skip: -3 };
        assert_eq!(scroll.window(5, &flat(), 100), (3, 0..2));
        assert_eq!(scroll.window(5, &all_tall(), 100), (3, 0..1));
    }

    /// Spec 0268 G3: only the rows in the span cost two terminal rows,
    /// so a pane whose span is over by row 5 keeps drawing one row per
    /// row after it.
    #[test]
    fn window_spends_two_rows_only_inside_the_span() {
        let scroll = PaneScroll { index: 2, skip: 0 };
        // Rows 2 and 3 are two rows each, then 4..8 are one: 4 + 4 = 8.
        assert_eq!(scroll.window(8, &RowHeights::new(2..4), 100), (0, 2..8));
        // Ahead of the span it is flat until row 6, which is cut in half.
        assert_eq!(scroll.window(5, &RowHeights::new(6..9), 100), (0, 2..7));
    }

    /// Content shorter than the pane, and a pane with no room at all,
    /// both yield an empty-or-short range rather than an out-of-bounds
    /// one — every caller slices with it.
    #[test]
    fn window_is_clamped_to_the_content() {
        let scroll = PaneScroll { index: 4, skip: 0 };
        assert_eq!(scroll.window(10, &flat(), 6), (0, 4..6));
        assert_eq!(scroll.window(10, &flat(), 2), (0, 2..2));
        assert_eq!(scroll.window(0, &flat(), 6), (0, 4..4));

        let scroll = PaneScroll { index: 0, skip: -9 };
        assert_eq!(scroll.window(5, &flat(), 6), (9, 0..0));
    }
}
