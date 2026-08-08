// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0244 S1: one vertical viewport, shared by the three pannable
//! panes (main, override, manage).

use std::ops::Range;

use super::{App, DisplayRow};

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
    pub(super) fn top(&self, row_height: usize) -> isize {
        (self.index * row_height) as isize + self.skip
    }

    /// Puts the viewport's top edge at `top`, splitting it back into the
    /// whole row and the remainder. The only writer of the pair.
    pub(super) fn set_top(&mut self, top: isize, row_height: usize) {
        let row_height = row_height as isize;
        if top < 0 {
            self.index = 0;
            self.skip = top;
        } else {
            self.index = (top / row_height) as usize;
            self.skip = top % row_height;
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
        row_height: usize,
        total: usize,
    ) -> (usize, Range<usize>) {
        let terminal_rows = (pane_height as isize + self.skip).max(0) as usize;
        let rows = terminal_rows.div_ceil(row_height);
        let start = self.index.min(total);
        let end = (self.index + rows).min(total);
        (self.skip.min(0).unsigned_abs(), start..end)
    }
}

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
        let top = (row * self.row_height()) as isize + anchor.skip;
        self.set_scroll_top(top);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `set_top` and `top` are inverses at every offset, in both row
    /// heights, on both sides of zero — the invariant every other method
    /// here assumes.
    #[test]
    fn set_top_and_top_round_trip() {
        for row_height in [1usize, 2] {
            for top in -8isize..=8 {
                let mut scroll = PaneScroll::default();
                scroll.set_top(top, row_height);
                assert_eq!(
                    scroll.top(row_height),
                    top,
                    "row_height {row_height}, top {top}"
                );
                assert!(
                    scroll.skip < row_height as isize,
                    "skip must stay under one row: {scroll:?}"
                );
                if top >= 0 {
                    assert!(scroll.skip >= 0, "only a negative top borrows: {scroll:?}");
                } else {
                    assert_eq!(scroll.index, 0, "nothing above row 0 to name: {scroll:?}");
                }
            }
        }
    }

    /// A viewport sitting exactly on a row draws no blank rows and
    /// exactly `pane_height / row_height` of them.
    #[test]
    fn window_on_a_whole_row() {
        let scroll = PaneScroll { index: 3, skip: 0 };
        assert_eq!(scroll.window(5, 1, 100), (0, 3..8));
        assert_eq!(scroll.window(6, 2, 100), (0, 3..6));
    }

    /// A positive skip cuts the top row in half, so one more content row
    /// is needed to fill the same pane.
    #[test]
    fn window_with_a_partial_row_at_the_top() {
        let scroll = PaneScroll { index: 3, skip: 1 };
        assert_eq!(scroll.window(6, 2, 100), (0, 3..7));
    }

    /// A negative skip is blank rows above the content, and that many
    /// fewer content rows drawn.
    #[test]
    fn window_with_blank_rows_above() {
        let scroll = PaneScroll { index: 0, skip: -3 };
        assert_eq!(scroll.window(5, 1, 100), (3, 0..2));
        assert_eq!(scroll.window(5, 2, 100), (3, 0..1));
    }

    /// Content shorter than the pane, and a pane with no room at all,
    /// both yield an empty-or-short range rather than an out-of-bounds
    /// one — every caller slices with it.
    #[test]
    fn window_is_clamped_to_the_content() {
        let scroll = PaneScroll { index: 4, skip: 0 };
        assert_eq!(scroll.window(10, 1, 6), (0, 4..6));
        assert_eq!(scroll.window(10, 1, 2), (0, 2..2));
        assert_eq!(scroll.window(0, 1, 6), (0, 4..4));

        let scroll = PaneScroll { index: 0, skip: -9 };
        assert_eq!(scroll.window(5, 1, 6), (9, 0..0));
    }
}
