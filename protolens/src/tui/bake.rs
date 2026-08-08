// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0255: finishing a bounded confirm's document in the background.
//!
//! A confirm renders a screenful and folds everything it did not reach
//! (spec 0249 S1/S3). Those folds are a debt: until they are paid the
//! line-count footer, the scrollbar, `G` and every search describe a
//! document that is shorter than the one the user asked for. The bake
//! pays it, one subtree per idle iteration, drawing nothing.
//!
//! Except where the user is looking. Spec 0249 S8's other half — a stop
//! that scrolls into view is expanded on arrival — is the same work
//! re-ordered: the queue is document order, and a reader who jumps is
//! not in document order.

use ratatui::style::Style;

use super::{theme, App, DisplayRow};
use crate::node_status::Status;

/// What one call to [`App::bake_step`] did.
///
/// Three-valued where `prefetch_step` and `search_sweep_step` are two,
/// because a bake step is the only one of the three that can change a
/// row the user is *reading*. Those rows are normally final by spec 0249
/// S1's depth-first rule, which is what lets spec 0255 S6 defer the
/// frame by half a second; expanding a stop that is on screen is exactly
/// the case where that premise does not hold.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum BakeStep {
    /// A stop that was on screen. The frame is owed *now*.
    Visible,
    /// A stop somewhere else in the document. Only the total height
    /// changed, so the frame is owed within `BAKE_REPAINT_INTERVAL`.
    Progressed,
    Idle,
}

impl App {
    /// Spec 0255 S1: how many rows one bake step renders.
    ///
    /// Larger than a confirm's screenful, deliberately. A bounded render
    /// re-emits its whole right frontier as folded rows (spec 0249 S1:
    /// output size = budget + breadth) and the step that expands one of
    /// them renders those rows again, so a small budget pays for the
    /// frontier over and over. Measured over a full drain of
    /// googleapis.desc, the bake's own render is 3.3x the unbounded
    /// render at a budget of 50 and 1.6x at 5000, over 419 723 steps
    /// against 70 797.
    ///
    /// The ceiling is the event loop: at 5000 the worst single step is
    /// ~22 ms, with two steps over 8 ms out of 70 797 and none over 50.
    pub(super) const BAKE_ROW_BUDGET: usize = 5000;

    /// Render at most one subtree a bounded confirm stopped at.
    ///
    /// Spec 0255 S3: `auto_folded` is the queue and `bake_queue` is only
    /// the order to walk it in, so an entry that has left the set — a
    /// duplicate, a node the user opened by hand (spec 0249 S8), a node
    /// whose ancestor was re-overridden underneath it — is discarded
    /// here rather than guarded against everywhere else.
    ///
    /// One `expand_auto_fold` per call, never a loop: the caller
    /// re-checks its channel between steps, and that is what bounds a
    /// keystroke's wait to a single step.
    pub(super) fn bake_step(&mut self) -> BakeStep {
        // Spec 0249 S8: what is on screen first, whatever the queue
        // says. The queue is document order and drains from the top, so
        // a reader who presses `G` or lands on a search match is sitting
        // in front of rows the bake will not reach for seconds. Those
        // rows are the only ones anybody is actually reading.
        while let Some(idx) = self.visible_stops.pop_front() {
            if !self.auto_folded.contains(&idx) {
                continue;
            }
            self.expand_auto_fold(idx, Self::BAKE_ROW_BUDGET);
            return BakeStep::Visible;
        }
        while let Some(idx) = self.bake_queue.pop_front() {
            if !self.auto_folded.contains(&idx) {
                continue;
            }
            self.expand_auto_fold(idx, Self::BAKE_ROW_BUDGET);
            return BakeStep::Progressed;
        }
        BakeStep::Idle
    }

    /// Record which of the rows a frame is about to draw are stops, so
    /// the next bake step pays those off first (spec 0249 S8).
    ///
    /// Called from `render_main_pane` with the window it has already
    /// built, so this costs one hash lookup per drawn row and no extra
    /// descent. The list is replaced rather than appended to: it
    /// describes one frame, and the frame after a jump describes
    /// somewhere else entirely.
    ///
    /// This terminates. Expanding the topmost visible stop renders
    /// `BAKE_ROW_BUDGET` rows depth-first from it, which is a hundred
    /// times a pane, so the rows that replace it are final and the stops
    /// it leaves behind fall below the fold rather than into it.
    /// Spec 0249 S13: the ambient half of "the document is still
    /// filling in" — the activity dot, in `Unbaked`'s violet, whenever
    /// anything still owes a body.
    ///
    /// This settles spec 0249's open question 4: the bake's cue lands on
    /// the dot spec 0190 already put in that cell, and this spec
    /// therefore supersedes specs 0204/0205/0209's claim to invent one.
    /// A second such cue is a second thing to keep consistent, and the
    /// user would have to learn which corner meant which job.
    ///
    /// The heat subsystem keeps the cell when it is using it. That costs
    /// almost nothing in practice — read-ahead is deferred while a bake
    /// runs (spec 0255 S5), so the two collide only on the `Visible` and
    /// `User` tiers, which are about a row the user is looking at and
    /// outrank an ambient one either way.
    ///
    /// Read straight off `auto_folded`, which is the truth about what is
    /// owed (spec 0255 S3) — so this is steady for exactly as long as
    /// the bake runs, and needs no flag anybody has to remember to
    /// clear. Steady, not flashing: a blink would be a timer-driven
    /// redraw every ~500 ms for the whole bake, reopening spec 0245's
    /// rule that a frame is drawn only when something changed.
    pub(super) fn bake_dot_style(&self) -> Option<Style> {
        if self.auto_folded.is_empty() {
            return None;
        }
        theme::status_color(Status::Unbaked, self.theme).map(|c| Style::default().fg(c))
    }

    pub(super) fn note_visible_stops(&mut self, window: &[DisplayRow]) {
        self.visible_stops.clear();
        if self.auto_folded.is_empty() {
            return;
        }
        for row in window {
            if let DisplayRow::Committed(c) = row {
                if self.auto_folded.contains(&c.pos.node) {
                    self.visible_stops.push_back(c.pos.node);
                }
            }
        }
    }
}
